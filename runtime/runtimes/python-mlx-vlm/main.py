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
            messages, images = materialize_images(request.get("messages", []), args.workdir)
            prompt = apply_chat_template(processor, config, messages, num_images=len(images))
            kwargs = {"max_tokens": int(request.get("max_tokens", 4096))}
            if "temperature" in request:
                kwargs["temperature"] = float(request["temperature"])
            if "top_p" in request:
                kwargs["top_p"] = float(request["top_p"])

            send_json({"event": "begin"})
            for response in stream_generate(model, processor, prompt, images, **kwargs):
                last = response
                send_json({"event": "text", "text": response.text})
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
                    "prompt_tokens": getattr(last, "prompt_tokens", 0) if last else 0,
                    "completion_tokens": getattr(last, "generation_tokens", 0) if last else 0,
                }
            )
        except Exception as error:
            send_json({"event": "error", "message": str(error)})


if __name__ == "__main__":
    main()
