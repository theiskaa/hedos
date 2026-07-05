import argparse
import json
import os
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


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", required=True)
    parser.add_argument("--workdir", required=True)
    args = parser.parse_args()

    import misaki.espeak

    class DisabledEspeakFallback:
        def __init__(self, *a, **k):
            raise RuntimeError("espeak fallback disabled")

    misaki.espeak.EspeakFallback = DisabledEspeakFallback

    import numpy as np
    from mlx_audio.tts.utils import load_model
    from mlx_audio.tts.models.kokoro import KokoroPipeline

    link = os.path.join(args.workdir, "kokoro-model")
    if os.path.islink(link):
        os.unlink(link)
    os.symlink(args.model, link)

    model = load_model(link)
    pipeline = KokoroPipeline(lang_code="a", model=model, repo_id=link)
    send_json({"event": "ready", "sample_rate": 24000})

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
        if op == "speak":
            text = request.get("text", "")
            voice = request.get("voice", "af_heart")
            voice_path = os.path.join(link, "voices", f"{voice}.safetensors")
            if not os.path.exists(voice_path):
                send_json({"event": "error", "message": f"voice {voice} not found"})
                continue
            try:
                send_json({"event": "begin"})
                total = 0
                speed = float(request.get("speed", 1.0))
                for result in pipeline(text, voice=voice_path, speed=speed):
                    audio = np.asarray(result.audio, dtype=np.float32).reshape(-1)
                    total += audio.shape[0]
                    send(2, audio.tobytes())
                send_json({"event": "done", "seconds": total / 24000})
            except Exception as error:
                send_json({"event": "error", "message": str(error)})


main()
