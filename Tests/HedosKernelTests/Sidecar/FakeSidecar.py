import json
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


mode = sys.argv[1] if len(sys.argv) > 1 else "normal"

if mode == "never-ready":
    time.sleep(60)
    sys.exit(0)

send_json({"event": "ready", "sample_rate": 16000})

while True:
    frame = read_frame()
    if frame is None:
        break
    _, payload = frame
    request = json.loads(payload)
    op = request.get("op")
    if op == "shutdown":
        break
    if op == "ping":
        send_json({"event": "pong"})
        continue
    if op == "speak":
        if mode == "crash-mid-request":
            send_json({"event": "begin"})
            send(2, b"\x00" * 64)
            sys.exit(7)
        send_json({"event": "begin"})
        for i in range(3):
            send(2, bytes([i]) * 640)
        send_json({"event": "done", "seconds": 0.12})
