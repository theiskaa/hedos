import argparse
import json
import os
import select
import struct
import sys

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


def pending_op():
    if not select.select([0], [], [], 0)[0]:
        return None
    frame = read_frame()
    if frame is None:
        return "shutdown"
    _, payload = frame
    try:
        return json.loads(payload).get("op")
    except ValueError:
        return None


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", required=True)
    parser.add_argument("--workdir", required=True)
    args = parser.parse_args()

    import numpy as np
    from pywhispercpp.model import Model

    model = Model(args.model, print_progress=False, print_realtime=False)
    send_json({"event": "ready", "sample_rate": 16000})

    while True:
        frame = read_frame()
        if frame is None:
            break
        _, payload = frame
        try:
            request = json.loads(payload)
        except ValueError:
            continue
        op = request.get("op")
        if op == "shutdown":
            break
        if op == "ping":
            send_json({"event": "pong"})
            continue
        if op == "transcribe":
            pcm_path = request.get("pcm", "")
            if not os.path.isfile(pcm_path):
                send_json({"event": "error", "message": f"pcm file missing: {pcm_path}"})
                continue
            try:
                samples = np.fromfile(pcm_path, dtype=np.float32)
                try:
                    os.remove(pcm_path)
                except OSError:
                    pass
                if samples.size == 0:
                    send_json({"event": "error", "message": "pcm file carries no samples"})
                    continue
                send_json({"event": "begin"})
                seconds = samples.size / 16000.0
                language = request.get("language")
                params = {}
                if language:
                    params["language"] = language
                if request.get("translate"):
                    params["translate"] = True

                cancel_state = {"cancelled": False, "shutdown": False}

                def on_segment(segment):
                    if cancel_state["cancelled"]:
                        return
                    send_json(
                        {
                            "event": "text",
                            "text": segment.text,
                            "t0_ms": segment.t0 * 10,
                            "t1_ms": segment.t1 * 10,
                        }
                    )
                    op = pending_op()
                    if op in ("cancel", "shutdown"):
                        cancel_state["cancelled"] = True
                        cancel_state["shutdown"] = op == "shutdown"

                segments = model.transcribe(
                    samples,
                    new_segment_callback=on_segment,
                    **params,
                )
                if cancel_state["cancelled"]:
                    send_json({"event": "cancelled"})
                    if cancel_state["shutdown"]:
                        break
                    continue
                send_json({"event": "done", "seconds": seconds})
            except Exception as error:
                send_json({"event": "error", "message": str(error)})


main()
