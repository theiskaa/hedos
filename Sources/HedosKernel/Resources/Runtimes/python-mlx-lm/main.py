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
    if "min_p" in request:
        kwargs["min_p"] = float(request["min_p"])
    if "top_k" in request:
        kwargs["top_k"] = int(request["top_k"])
    return kwargs


def stop_strings(request):
    stops = request.get("stop")
    if stops is None:
        return []
    if isinstance(stops, str):
        return [stops]
    return [s for s in stops if isinstance(s, str) and s]


class StopScanner:
    def __init__(self, stops):
        self.stops = [s for s in stops if s]
        self.buffer = ""
        self.stopped = False

    @property
    def active(self):
        return bool(self.stops)

    def feed(self, chunk):
        if not self.stops or self.stopped:
            return "" if self.stopped else chunk
        self.buffer += chunk
        earliest = None
        for stop in self.stops:
            index = self.buffer.find(stop)
            if index != -1 and (earliest is None or index < earliest):
                earliest = index
        if earliest is not None:
            emit = self.buffer[:earliest]
            self.buffer = ""
            self.stopped = True
            return emit
        longest = max((len(s) for s in self.stops), default=1) - 1
        cap = min(longest, len(self.buffer))
        hold = 0
        for length in range(cap, 0, -1):
            suffix = self.buffer[-length:]
            if any(s.startswith(suffix) for s in self.stops):
                hold = length
                break
        cut = len(self.buffer) - hold
        emit = self.buffer[:cut]
        self.buffer = self.buffer[cut:]
        return emit

    def flush(self):
        if self.stopped:
            return ""
        emit = self.buffer
        self.buffer = ""
        return emit


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", required=True)
    parser.add_argument("--workdir", required=True)
    args = parser.parse_args()

    os.environ.setdefault("HF_HUB_OFFLINE", "1")

    import mlx.core as mx
    from mlx_lm import load, stream_generate
    from mlx_lm.sample_utils import make_logits_processors, make_sampler

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

        started = time.monotonic()
        cancelled = False
        shutdown_requested = False
        last = None
        try:
            if op == "chat":
                prompt = tokenizer.apply_chat_template(
                    request.get("messages", []), add_generation_prompt=True
                )
            else:
                prompt = request.get("prompt", "")
            max_tokens = int(request.get("max_tokens", 4096))
            if request.get("seed") is not None:
                mx.random.seed(int(request["seed"]))
            scanner = StopScanner(stop_strings(request))
            generate_kwargs = {
                "max_tokens": max_tokens,
                "sampler": make_sampler(**sampler_kwargs(request)),
            }
            if "repeat_penalty" in request:
                generate_kwargs["logits_processors"] = make_logits_processors(
                    repetition_penalty=float(request["repeat_penalty"])
                )
            send_json({"event": "begin"})
            stopped = False
            for response in stream_generate(
                model, tokenizer, prompt, **generate_kwargs
            ):
                last = response
                if scanner.active:
                    emit = scanner.feed(response.text)
                    if emit:
                        send_json({"event": "text", "text": emit})
                    if scanner.stopped:
                        stopped = True
                        break
                else:
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
            if scanner.active and not stopped:
                tail = scanner.flush()
                if tail:
                    send_json({"event": "text", "text": tail})
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
