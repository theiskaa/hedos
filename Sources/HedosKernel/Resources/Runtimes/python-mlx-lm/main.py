import argparse
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


def sampler_kwargs(request):
    kwargs = {}
    if "temperature" in request:
        kwargs["temp"] = float(request["temperature"])
    if "top_p" in request:
        kwargs["top_p"] = float(request["top_p"])
    return kwargs


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", required=True)
    parser.add_argument("--workdir", required=True)
    args = parser.parse_args()

    os.environ.setdefault("HF_HUB_OFFLINE", "1")

    from mlx_lm import load, stream_generate
    from mlx_lm.sample_utils import make_sampler

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
        if op not in ("chat", "complete"):
            continue

        if op == "chat":
            prompt = tokenizer.apply_chat_template(
                request.get("messages", []), add_generation_prompt=True
            )
        else:
            prompt = request.get("prompt", "")
        max_tokens = int(request.get("max_tokens", 4096))

        started = time.monotonic()
        cancelled = False
        shutdown_requested = False
        last = None
        try:
            send_json({"event": "begin"})
            for response in stream_generate(
                model, tokenizer, prompt,
                max_tokens=max_tokens,
                sampler=make_sampler(**sampler_kwargs(request)),
            ):
                send_json({"event": "text", "text": response.text})
                last = response
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
            send_json(
                {
                    "event": "done",
                    "seconds": time.monotonic() - started,
                    "prompt_tokens": last.prompt_tokens if last else 0,
                    "completion_tokens": last.generation_tokens if last else 0,
                }
            )
        except Exception as error:
            send_json({"event": "error", "message": str(error)})


main()
