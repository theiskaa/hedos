import argparse
import hashlib
import json
import os
import struct
import sys
import time
import types

real_stdout = os.dup(1)
os.dup2(2, 1)


def send(frame_type, payload):
    os.write(real_stdout, struct.pack("<I", len(payload) + 1) + bytes([frame_type]) + payload)


def send_json(obj):
    send(1, json.dumps(obj).encode())


def read_exact(count):
    buffer = b""
    while len(buffer) < count:
        chunk = os.read(0, count - len(buffer))
        if not chunk:
            return None
        buffer += chunk
    return buffer


def read_frame():
    header = read_exact(4)
    if header is None:
        return None
    (length,) = struct.unpack("<I", header)
    body = read_exact(length)
    if body is None:
        return None
    return body[0], body[1:]


def read_request():
    frame = read_frame()
    if frame is None:
        return None
    _, payload = frame
    try:
        return json.loads(payload)
    except ValueError:
        return {}


USAGE = (
    'send the question as JSON in the user message: {"state": ..., "questions": '
    '{"<id>": {"type": "choice" | "score" | "noul", "instructions": "...", "criteria": ...}}}'
)


def message_text(message):
    content = message.get("content", "")
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        return "".join(
            part.get("text", "")
            for part in content
            if isinstance(part, dict) and part.get("type", "text") == "text"
        )
    return ""


def last_user_text(messages):
    for message in reversed(messages):
        if isinstance(message, dict) and message.get("role") == "user":
            return message_text(message)
    return ""


def parse_question(text):
    try:
        body = json.loads(text)
    except ValueError:
        raise ValueError(f"laya answers typed questions, not prose; {USAGE}") from None
    if not isinstance(body, dict) or "state" not in body:
        raise ValueError(f'the request has no "state"; {USAGE}')
    questions = body.get("questions")
    if questions is None and isinstance(body.get("question"), dict):
        questions = {"question": body["question"]}
    if not isinstance(questions, dict) or not questions:
        raise ValueError(f'the request has no "questions"; {USAGE}')
    for key, question in questions.items():
        if not isinstance(question, dict):
            raise ValueError(f"question {key!r} is not an object; {USAGE}")
        if question.get("type") not in ("choice", "score", "noul"):
            raise ValueError(f"question {key!r} has no type of choice, score, or noul")
        if "instructions" not in question:
            raise ValueError(f"question {key!r} has no instructions")
    return body["state"], questions


# The decision head is the model's own code, shipped beside its weights, and
# running it keeps its calibration and option ordering exactly as trained. It is
# also code from a model repository, and approving this runtime approves only
# the files in this directory. So the two modules are pinned here by content:
# a snapshot whose code differs is refused, and accepting new code means
# changing these lines, which changes the runtime's consent hash and asks the
# user again. Nothing else in the snapshot is importable.
PINNED_SOURCES = (
    (
        "rl_common",
        "rl_common.py",
        "8d83611d480c971d640a7b7d3aa2f2219c5e8455e9cc2329fd073681bd8be23e",
    ),
    (
        "rl_agent_api",
        "rl_agent_api.py",
        "be3b46819c9999c3ef88e0f2ecf6d3ab1cdfed1d9a8b466fc89811e34d44031b",
    ),
)


def load_pinned(model_dir, pinned=PINNED_SOURCES):
    # Each module runs from the very bytes that were hashed, so the file cannot
    # change between the check and the import.
    for name, filename, expected in pinned:
        path = os.path.join(model_dir, filename)
        with open(path, "rb") as handle:
            source = handle.read()
        if hashlib.sha256(source).hexdigest() != expected:
            raise RuntimeError(
                f"{filename} in this snapshot is not the code this runtime was reviewed against, "
                "so it will not be run"
            )
        module = types.ModuleType(name)
        module.__file__ = path
        sys.modules[name] = module
        exec(compile(source, path, "exec"), module.__dict__)
    return sys.modules[pinned[-1][0]]


def pick_device():
    import torch

    # The model's own loader only knows cuda and cpu, so on Apple Silicon it
    # would settle on the CPU without saying so.
    if torch.cuda.is_available():
        return "cuda"
    if torch.backends.mps.is_available():
        return "mps"
    return "cpu"


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", required=True)
    parser.add_argument("--workdir", required=True)
    args = parser.parse_args()

    os.environ.setdefault("HF_HUB_OFFLINE", "1")
    os.environ.setdefault("TRANSFORMERS_OFFLINE", "1")
    os.environ.setdefault("TOKENIZERS_PARALLELISM", "false")

    try:
        agent = load_pinned(args.model).RLAgent(args.model, device=pick_device())
    except (OSError, RuntimeError) as error:
        sys.exit(str(error))
    send_json({"event": "ready"})

    while True:
        request = read_request()
        if request is None:
            break
        op = request.get("op")
        if op == "shutdown":
            break
        if op == "ping":
            send_json({"event": "pong"})
            continue
        if op != "chat":
            continue

        started = time.monotonic()
        try:
            state, questions = parse_question(last_user_text(request.get("messages", [])))
            send_json({"event": "begin"})
            judgment = agent.system_one(state, questions)
            send_json({"event": "text", "text": json.dumps(judgment, ensure_ascii=False)})
            send_json(
                {
                    "event": "done",
                    "seconds": time.monotonic() - started,
                    "prompt_tokens": judgment.get("usage", {}).get("input_tokens", 0),
                    "completion_tokens": 0,
                }
            )
        except Exception as error:
            send_json({"event": "error", "message": str(error)})


if __name__ == "__main__":
    main()
