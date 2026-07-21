import argparse
import json
import os
import select
import struct

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


def setup_espeak():
    import misaki.espeak

    brew_lib = "/opt/homebrew/lib/libespeak-ng.dylib"
    brew_share = "/opt/homebrew/share"
    if os.path.exists(brew_lib) and os.path.exists(brew_share + "/espeak-ng-data/phontab"):
        from phonemizer.backend.espeak.wrapper import EspeakWrapper

        EspeakWrapper.set_library(brew_lib)
        EspeakWrapper.set_data_path(brew_share)
        return "homebrew"

    class DisabledEspeakFallback:
        def __init__(self, *a, **k):
            raise RuntimeError("no usable espeak")

    misaki.espeak.EspeakFallback = DisabledEspeakFallback
    return "disabled"


def patch_sinegen():
    import mlx.core as mx
    from mlx_audio.tts.models.kokoro import istftnet

    def aligned_call(self, f0):
        fn = f0 * mx.arange(1, self.harmonic_num + 2)[None, None, :]
        sine_waves = self._f02sine(fn) * self.sine_amp
        uv = self._f02uv(f0)
        n = min(sine_waves.shape[1], uv.shape[1])
        sine_waves = sine_waves[:, :n, :]
        uv = uv[:, :n, :]
        noise_amp = uv * self.noise_std + (1 - uv) * self.sine_amp / 3
        noise = noise_amp * mx.random.normal(sine_waves.shape)
        sine_waves = sine_waves * uv + noise
        return sine_waves, uv, noise

    istftnet.SineGen.__call__ = aligned_call


KOKORO_LANGUAGES = {
    "a": "American English",
    "b": "British English",
    "e": "Spanish",
    "f": "French",
    "h": "Hindi",
    "i": "Italian",
    "j": "Japanese",
    "p": "Brazilian Portuguese",
    "z": "Mandarin Chinese",
}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", required=True)
    parser.add_argument("--workdir", required=True)
    args = parser.parse_args()

    espeak_mode = setup_espeak()

    import numpy as np
    from mlx_audio.tts.models.kokoro import KokoroPipeline
    from mlx_audio.tts.utils import load_model

    patch_sinegen()

    link = os.path.join(args.workdir, "kokoro-model")
    if os.path.islink(link):
        os.unlink(link)
    os.symlink(args.model, link)

    voices_dir = os.path.join(link, "voices")
    voices = (
        sorted(f[:-12] for f in os.listdir(voices_dir) if f.endswith(".safetensors"))
        if os.path.isdir(voices_dir)
        else []
    )

    model = load_model(link)
    pipelines = {}

    def pipeline_for(code):
        if code not in pipelines:
            pipelines[code] = KokoroPipeline(lang_code=code, model=model, repo_id=link)
        return pipelines[code]

    pipeline_for("a")

    send_json(
        {
            "event": "ready",
            "sample_rate": 24000,
            "voices": voices,
            "espeak": espeak_mode,
        }
    )

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
            voice_path = os.path.join(voices_dir, f"{voice}.safetensors")
            if not os.path.exists(voice_path):
                send_json({"event": "error", "message": f"voice {voice} not found"})
                continue
            code = voice[0] if voice and voice[0] in KOKORO_LANGUAGES else "a"
            try:
                pipeline = pipeline_for(code)
            except Exception as error:
                language = KOKORO_LANGUAGES.get(code, code)
                send_json(
                    {
                        "event": "error",
                        "message": f"cannot synthesize {language}: {error}",
                    }
                )
                continue
            try:
                send_json({"event": "begin"})
                total = 0
                speed = float(request.get("speed", 1.0))
                cancelled = False
                shutdown_requested = False
                for result in pipeline(text, voice=voice_path, speed=speed):
                    audio = np.asarray(result.audio, dtype=np.float32).reshape(-1)
                    total += audio.shape[0]
                    send(2, audio.tobytes())
                    op = pending_op()
                    if op in ("cancel", "shutdown"):
                        cancelled = True
                        shutdown_requested = op == "shutdown"
                        break
                if cancelled:
                    send_json({"event": "cancelled"})
                    if shutdown_requested:
                        break
                    continue
                send_json({"event": "done", "seconds": total / 24000})
            except Exception as error:
                send_json({"event": "error", "message": str(error)})


if __name__ == "__main__":
    main()
