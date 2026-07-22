import argparse
import base64
import json
import os
import select
import struct
import time

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


def pending_op():
    if not select.select([0], [], [], 0)[0]:
        return None
    request = read_request()
    if request is None:
        return "shutdown"
    return request.get("op")


def tool_specs(request):
    tools = request.get("tools")
    if not isinstance(tools, list):
        return []
    return [tool for tool in tools if isinstance(tool, dict) and tool.get("name")]


def tool_system_block(tools):
    lines = ["You can call tools. The available tools are:", ""]
    for tool in tools:
        lines.append(
            "- {}: {} Parameters schema: {}".format(
                tool.get("name", ""),
                tool.get("description", ""),
                json.dumps(tool.get("parameters", {})),
            )
        )
    lines.append("")
    lines.append(
        "To call a tool, reply with exactly one block of the form "
        '<tool_call>{"name": "<tool name>", "arguments": {…}}</tool_call> '
        "and nothing after it. Only call a tool when it is needed to answer."
    )
    return "\n".join(lines)


def with_tool_block(messages, tools):
    # Merged into an existing leading system turn rather than prepended as a
    # second one — strict templates reject more than one system message.
    block = tool_system_block(tools)
    if messages and isinstance(messages[0], dict) and messages[0].get("role") == "system":
        first = dict(messages[0])
        content = first.get("content", "")
        first["content"] = f"{content}\n\n{block}" if content else block
        return [first] + messages[1:]
    return [{"role": "system", "content": block}] + messages


def with_tool_block_in_user(messages, tools):
    block = tool_system_block(tools)
    shaped = list(messages)
    for index, message in enumerate(shaped):
        if isinstance(message, dict) and message.get("role") == "user":
            merged = dict(message)
            content = merged.get("content", "")
            merged["content"] = f"{block}\n\n{content}" if content else block
            shaped[index] = merged
            return shaped
    return shaped + [{"role": "user", "content": block}]


def inline_tool_history(messages):
    # mlx_vlm's prompt_utils templates know nothing of tool_calls fields or a
    # "tool" role, so the history is folded into plain turns: calls become
    # <tool_call> blocks in the assistant text, results become labeled user
    # turns. The system block teaches the model the same block format.
    shaped = []
    for message in messages:
        if not isinstance(message, dict):
            shaped.append(message)
            continue
        message = dict(message)
        calls = message.pop("tool_calls", None)
        if isinstance(calls, list) and calls:
            blocks = []
            for call in calls:
                if not isinstance(call, dict) or not call.get("name"):
                    continue
                body = json.dumps({"name": call["name"], "arguments": call.get("arguments", {})})
                blocks.append(f"<tool_call>{body}</tool_call>")
            if blocks:
                content = message.get("content", "")
                joined = "\n".join(blocks)
                message["content"] = f"{content}\n{joined}" if content else joined
        if message.get("role") == "tool":
            name = message.pop("tool_name", "")
            content = message.get("content", "")
            label = f'[tool "{name}" result]' if name else "[tool result]"
            text = f"{label}\n{content}" if content else label
            # Folded into a preceding user turn when there is one — templates
            # that enforce strict user/assistant alternation reject the
            # back-to-back user messages a multi-call turn would produce.
            previous = shaped[-1] if shaped else None
            if isinstance(previous, dict) and previous.get("role") == "user":
                content = previous.get("content", "")
                previous["content"] = f"{content}\n\n{text}" if content else text
                continue
            message["role"] = "user"
            message["content"] = text
        shaped.append(message)
    return shaped


CALL_OPEN = "<tool_call>"
CALL_CLOSE = "</tool_call>"
TOOL_CALLS_MARKER = "[TOOL_CALLS]"
PYTHON_TAG = "<|python_tag|>"


def try_json(text):
    try:
        return json.loads(text)
    except ValueError:
        return None


def call_from_value(value, require_arguments=False):
    if not isinstance(value, dict):
        return None
    name = value.get("name")
    if not isinstance(name, str) or not name:
        return None
    if require_arguments and "arguments" not in value and "parameters" not in value:
        return None
    arguments = value.get("arguments", value.get("parameters"))
    if arguments is None:
        arguments = {}
    if not isinstance(arguments, dict):
        return None
    call = {"name": name, "arguments": arguments}
    if isinstance(value.get("id"), str) and value["id"]:
        call["id"] = value["id"]
    return call


def parse_tagged_calls(text):
    calls = []
    remaining = []
    cursor = 0
    while True:
        start = text.find(CALL_OPEN, cursor)
        if start == -1:
            remaining.append(text[cursor:])
            break
        end = text.find(CALL_CLOSE, start)
        if end == -1:
            remaining.append(text[cursor:])
            break
        remaining.append(text[cursor:start])
        call = call_from_value(try_json(text[start + len(CALL_OPEN) : end].strip()))
        if call:
            calls.append(call)
        else:
            remaining.append(text[start : end + len(CALL_CLOSE)])
        cursor = end + len(CALL_CLOSE)
    return "".join(remaining), calls


def parse_mistral_calls(text):
    start = text.find(TOOL_CALLS_MARKER)
    if start == -1:
        return text, []
    value = try_json(text[start + len(TOOL_CALLS_MARKER) :].strip())
    entries = value if isinstance(value, list) else [value]
    calls = [call for call in (call_from_value(entry) for entry in entries) if call]
    if not calls:
        return text, []
    return text[:start], calls


def parse_llama_calls(text):
    stripped = text.strip()
    if stripped.startswith(PYTHON_TAG):
        call = call_from_value(try_json(stripped[len(PYTHON_TAG) :].strip()))
        if call:
            return "", [call]
        return text, []
    # Without a marker, a JSON reply is only a call when it names its
    # arguments — a data answer that merely contains a "name" field stays text.
    value = try_json(stripped)
    if isinstance(value, list):
        calls = [
            call
            for call in (call_from_value(entry, require_arguments=True) for entry in value)
            if call
        ]
        if calls and len(calls) == len(value):
            return "", calls
        return text, []
    call = call_from_value(value, require_arguments=True)
    if call:
        return "", [call]
    return text, []


def extract_tool_calls(text):
    # The marker formats are mutually exclusive, so a fixed order is safe; the
    # bare-JSON fallback (inside parse_llama_calls) must run last because any
    # model family can degrade to it.
    for parser in (parse_tagged_calls, parse_mistral_calls, parse_llama_calls):
        remaining, calls = parser(text)
        if calls:
            return remaining, calls
    return text, []


def materialize_images(messages, workdir):
    paths = []
    stripped = []
    for index, message in enumerate(messages):
        images = message.get("images") or []
        for offset, encoded in enumerate(images):
            data = base64.b64decode(encoded)
            path = os.path.join(workdir, f"image-{index}-{offset}.img")
            with open(path, "wb") as handle:
                handle.write(data)
            paths.append(path)
        copy = dict(message)
        copy.pop("images", None)
        stripped.append(copy)
    return stripped, paths


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", required=True)
    parser.add_argument("--workdir", required=True)
    args = parser.parse_args()

    os.environ.setdefault("HF_HUB_OFFLINE", "1")

    from mlx_vlm import load, stream_generate
    from mlx_vlm.prompt_utils import apply_chat_template
    from mlx_vlm.utils import load_config

    model, processor = load(args.model)
    config = load_config(args.model)
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
        if op not in ("chat", "complete", "see"):
            continue

        started = time.monotonic()
        cancelled = False
        shutdown_requested = False
        last = None
        try:
            tools = tool_specs(request) if op == "chat" else []
            messages, images = materialize_images(request.get("messages", []), args.workdir)
            messages = inline_tool_history(messages)
            if tools:
                try:
                    prompt = apply_chat_template(
                        processor, config, with_tool_block(messages, tools), num_images=len(images)
                    )
                except Exception:
                    # Templates that reject a system role (Gemma family) get
                    # the block folded into the first user turn instead.
                    prompt = apply_chat_template(
                        processor,
                        config,
                        with_tool_block_in_user(messages, tools),
                        num_images=len(images),
                    )
            else:
                prompt = apply_chat_template(processor, config, messages, num_images=len(images))
            kwargs = {"max_tokens": int(request.get("max_tokens", 4096))}
            if "temperature" in request:
                kwargs["temperature"] = float(request["temperature"])
            if "top_p" in request:
                kwargs["top_p"] = float(request["top_p"])

            # With tools offered, visible text is held to end-of-turn so a tool
            # call can be parsed out of the full reply.
            collector = [] if tools else None
            send_json({"event": "begin"})
            for response in stream_generate(model, processor, prompt, images, **kwargs):
                last = response
                if collector is None:
                    send_json({"event": "text", "text": response.text})
                else:
                    collector.append(response.text)
                inner = pending_op()
                if inner in ("cancel", "shutdown"):
                    cancelled = True
                    shutdown_requested = inner == "shutdown"
                    break
            if cancelled:
                send_json({"event": "cancelled"})
                if shutdown_requested:
                    break
                continue
            if collector is not None:
                remaining, calls = extract_tool_calls("".join(collector))
                # Leftover text always flows when nothing parsed; once a call
                # did, whitespace-only leftovers are noise and are dropped.
                if remaining and (not calls or remaining.strip()):
                    send_json({"event": "text", "text": remaining})
                for call in calls:
                    send_json({"event": "tool_call", **call})
            send_json(
                {
                    "event": "done",
                    "seconds": time.monotonic() - started,
                    "prompt_tokens": getattr(last, "prompt_tokens", 0) if last else 0,
                    "completion_tokens": getattr(last, "generation_tokens", 0) if last else 0,
                }
            )
        except Exception as error:
            send_json({"event": "error", "message": str(error)})


if __name__ == "__main__":
    main()
