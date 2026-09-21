import argparse
import json
import math
import os
import struct
import sys
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


USAGE = (
    'send the question as JSON in the user message: {"state": ..., "questions": '
    '{"<id>": {"type": "choice" | "score" | "noul", "instructions": "...", "criteria": ...}}}'
)

# The model card's own mapping from the raw "Yes" logit to a 0-1 relevance is
# sigmoid(logit / 5). Dividing by the same temperature before a softmax keeps a
# choice consistent with it: the odds of one option over another are exactly the
# odds sigmoid gives their difference, which is the Elo reading zerank is
# trained on.
TEMPERATURE = 5.0


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
        raise ValueError(f"zerank answers typed questions, not prose; {USAGE}") from None
    if not isinstance(body, dict) or "state" not in body:
        raise ValueError(f'the request has no "state"; {USAGE}')
    questions = body.get("questions")
    if questions is None and isinstance(body.get("question"), dict):
        questions = {"question": body["question"]}
    if not isinstance(questions, dict) or not questions:
        raise ValueError(f'the request has no "questions"; {USAGE}')
    state = as_text(body["state"])
    for key, question in questions.items():
        if not isinstance(question, dict):
            raise ValueError(f"question {key!r} is not an object; {USAGE}")
        if question.get("type") not in ("choice", "score", "noul"):
            raise ValueError(f"question {key!r} has no type of choice, score, or noul")
        if not as_text(question.get("instructions")):
            raise ValueError(f"question {key!r} has no instructions")
        criteria = question.get("criteria")
        if question["type"] == "choice" and not (isinstance(criteria, (dict, list)) and criteria):
            raise ValueError(f"question {key!r} is a choice with no criteria to choose between")
        if question["type"] == "score" and not (isinstance(criteria, list) and criteria):
            raise ValueError(f"question {key!r} is a score with no list of levels as criteria")
        # A listed option is its own key in the answer, so a repeat would fold
        # two scored options into one and leave the rest summing short of one.
        if question["type"] == "choice" and isinstance(criteria, list):
            labels = [as_text(label) for label in criteria]
            repeated = sorted({label for label in labels if labels.count(label) > 1})
            if repeated:
                raise ValueError(f"question {key!r} lists {', '.join(map(repr, repeated))} twice")
        # A reranker scores a document against a query. A noul's document is
        # the state, so with none there is nothing to score the statement on.
        if question["type"] == "noul" and not state:
            raise ValueError(
                f"question {key!r} is a noul with no state; zerank scores the state "
                "against the statement, so it needs one"
            )
    return state, questions


def as_text(value):
    if value is None:
        return ""
    if isinstance(value, str):
        return value.strip()
    return json.dumps(value, ensure_ascii=False)


def query_for(state, question):
    instructions = as_text(question["instructions"])
    if question["type"] == "noul" or not state:
        return instructions
    return f"{state}\n\n{instructions}"


def documents_for(state, question):
    """The (key, document) pairs a question is answered over, in the order given."""
    criteria = question.get("criteria")
    if question["type"] == "noul":
        return [("noul", state)]
    if question["type"] == "score":
        return [(str(level), as_text(text)) for level, text in enumerate(criteria)]
    if isinstance(criteria, list):
        return [(as_text(label), as_text(label)) for label in criteria]
    return [
        (label, f"{label}: {as_text(meaning)}" if as_text(meaning) else label)
        for label, meaning in criteria.items()
    ]


def softmax(logits):
    scaled = [logit / TEMPERATURE for logit in logits]
    top = max(scaled)
    weights = [math.exp(value - top) for value in scaled]
    total = sum(weights)
    return [weight / total for weight in weights]


def relevance(logit):
    return 1.0 / (1.0 + math.exp(-logit / TEMPERATURE))


def answer(question, keys, logits):
    """One answer in laya's envelope shape, so every judge reads the same.
    `logits` are the raw "Yes" logits, kept beside the probabilities because a
    reranker's scores mean something on their own, which a softmax hides."""
    raw = {key: round(logit, 4) for key, logit in zip(keys, logits, strict=True)}
    if question["type"] == "noul":
        return {"type": "noul", "noul": round(relevance(logits[0]), 4), "zerank": {"logit": raw}}
    probabilities = softmax(logits)
    extra = {
        "logits": raw,
        "relevance": {
            key: round(relevance(logit), 4) for key, logit in zip(keys, logits, strict=True)
        },
    }
    rounded = {key: round(p, 4) for key, p in zip(keys, probabilities, strict=True)}
    if question["type"] == "choice":
        best = max(range(len(keys)), key=lambda index: probabilities[index])
        return {"type": "choice", "choice": keys[best], "probabilities": rounded, "zerank": extra}
    expected = sum(level * p for level, p in enumerate(probabilities))
    return {
        "type": "score",
        "score": round(expected, 4),
        "legend": {key: text for key, text in documents_for("", question)},
        "probabilities": rounded,
        "zerank": extra,
    }


def pairs_for(state, questions):
    """Every (query, document) pair the questions ask for, and where each
    question's answers sit among them."""
    pairs, spans = [], []
    for qid, question in questions.items():
        query = query_for(state, question)
        documents = documents_for(state, question)
        spans.append((qid, question, [key for key, _ in documents], len(pairs)))
        pairs.extend((query, document) for _, document in documents)
    return pairs, spans


def pair_tokens(model, query, document):
    """The tokens the model reads for one pair, laid out by its chat template."""
    return len(
        model.tokenizer.apply_chat_template(
            [{"role": "query", "content": query}, {"role": "document", "content": document}],
            add_generation_prompt=True,
            tokenize=True,
        )["input_ids"]
    )


def refuse_overlong(counts, limit):
    # Past the positions it was trained on the model neither errs nor truncates;
    # it answers from positions it never saw, and costs 20 GB doing it.
    longest = max(counts, default=0)
    if limit and longest > limit:
        raise ValueError(
            f"a question reads {longest} tokens with its state, past the {limit} zerank reads"
        )


def judge(model, pairs, spans):
    logits = [float(score) for score in model.predict(pairs, convert_to_numpy=True)]
    if len(logits) != len(pairs) or not all(math.isfinite(logit) for logit in logits):
        raise RuntimeError("the model returned no usable score for every option")
    answers = {
        qid: answer(question, keys, logits[start : start + len(keys)])
        for qid, question, keys, start in spans
    }
    return {"model": "zerank", "answers": answers, "usage": {"output_tokens": 0}}


def position_limit(model_dir):
    try:
        with open(os.path.join(model_dir, "config.json"), encoding="utf-8") as handle:
            return int(json.load(handle).get("max_position_embeddings") or 0)
    except (OSError, ValueError, TypeError):
        return 0


def pick_device():
    import torch

    if torch.cuda.is_available():
        return "cuda"
    if torch.backends.mps.is_available():
        return "mps"
    return "cpu"


def load(model_dir):
    import torch
    from sentence_transformers import CrossEncoder

    # The weights are bf16; left to itself the loader widens them to float32,
    # which doubles the 8 GB it takes.
    return CrossEncoder(
        model_dir,
        device=pick_device(),
        local_files_only=True,
        model_kwargs={"dtype": torch.bfloat16},
    )


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", required=True)
    parser.add_argument("--workdir", required=True)
    args = parser.parse_args()

    os.environ.setdefault("HF_HUB_OFFLINE", "1")
    os.environ.setdefault("TRANSFORMERS_OFFLINE", "1")
    os.environ.setdefault("TOKENIZERS_PARALLELISM", "false")

    try:
        model = load(args.model)
    except (OSError, RuntimeError, ValueError) as error:
        sys.exit(str(error))
    limit = position_limit(args.model)
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
        if op not in ("chat", "judge"):
            continue

        started = time.monotonic()
        try:
            state, questions = parse_question(last_user_text(request.get("messages", [])))
            pairs, spans = pairs_for(state, questions)
            counts = [pair_tokens(model, query, document) for query, document in pairs]
            refuse_overlong(counts, limit)
        except ValueError as error:
            send_json({"event": "error", "message": str(error), "fault": "request"})
            continue
        # Past the parse, anything raised is the model's or this runtime's, a
        # ValueError from torch or the tokenizer included, never the caller's.
        try:
            send_json({"event": "begin"})
            judgment = judge(model, pairs, spans)
            judgment["usage"]["input_tokens"] = sum(counts)
            send_json({"event": "text", "text": json.dumps(judgment, ensure_ascii=False)})
            send_json(
                {
                    "event": "done",
                    "seconds": time.monotonic() - started,
                    "prompt_tokens": judgment["usage"]["input_tokens"],
                    "completion_tokens": 0,
                }
            )
        except Exception as error:
            send_json({"event": "error", "message": str(error)})


if __name__ == "__main__":
    main()
