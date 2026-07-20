import argparse
import json
import os
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


def as_list(value):
    if value is None:
        return []
    if isinstance(value, str):
        return [value]
    return list(value)


def embed_one(model, tokenizer, text):
    import mlx.core as mx

    inputs = tokenizer.batch_encode_plus(
        [text], return_tensors="mlx", padding=True, truncation=True)
    output = model(inputs["input_ids"], attention_mask=inputs.get("attention_mask"))
    vector = output.text_embeds
    if vector.ndim > 1:
        vector = vector[0]
    return [float(x) for x in mx.array(vector).tolist()]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", required=True)
    parser.add_argument("--workdir", required=True)
    args = parser.parse_args()

    os.environ.setdefault("HF_HUB_OFFLINE", "1")

    from mlx_embeddings.utils import load

    model, tokenizer = load(args.model)
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
        if op != "embed":
            continue

        started = time.monotonic()
        try:
            inputs = as_list(request.get("input"))
            for text in inputs:
                send_json({"event": "vector", "values": embed_one(model, tokenizer, text)})
            send_json(
                {
                    "event": "done",
                    "seconds": time.monotonic() - started,
                    "prompt_tokens": sum(len(text.split()) for text in inputs),
                }
            )
        except Exception as error:
            send_json({"event": "error", "message": str(error)})


if __name__ == "__main__":
    main()
