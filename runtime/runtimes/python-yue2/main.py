import argparse
import json
import os
import select
import struct
import threading
import time

from audio_output import FormatError, encode, output_format, send_result

real_stdout = os.dup(1)
os.dup2(2, 1)

# The heartbeat thread sends while a stage runs on the main thread.
send_lock = threading.Lock()


def send(frame_type, payload):
    with send_lock:
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


COT_MODES = ("full", "melody", "off")

# A token stage reports every this many tokens: often enough to show
# movement, rarely enough not to flood the channel.
TOKEN_STRIDE = 64

# A stage with no progress hook of its own says it is still working this
# often, well inside the 600-second silence after which hedos stops a job.
HEARTBEAT_SECONDS = 30

SAMPLE_RATE = 48000

# What the job contract carries that YuE2 has no use for: the song is as long
# as its lyrics, and it takes no negative prompt or step count.
UNTAKEN = ("duration", "steps", "guidance", "negative_prompt")


class RequestFault(Exception):
    """A request YuE2 cannot answer as asked: refused, not retried."""


class CancelJob(Exception):
    pass


def song_request(request):
    """The `SongRequest` fields for a job request, checked: a style prompt and
    lyrics are both required, `cot` is one of its modes, and keys YuE2 has no
    use for are refused rather than ignored."""
    for key in UNTAKEN:
        value = request.get(key)
        if value is not None and not (isinstance(value, str) and not value.strip()):
            raise RequestFault(f"YuE2 does not take {key}; the lyrics set the length")
    style = str(request.get("prompt") or "").strip()
    if not style:
        raise RequestFault("YuE2 needs a style prompt (genre, mood, instruments, voice)")
    lyrics = str(request.get("lyrics") or "").strip()
    if request.get("instrumental") and not lyrics:
        lyrics = "[instrumental]"
    if not lyrics:
        raise RequestFault(
            "YuE2 writes songs from lyrics: send lyrics with [verse] and [chorus] tags"
        )
    fields = {"style": style, "lyrics": lyrics}
    cot = request.get("cot")
    if cot is not None:
        if cot not in COT_MODES:
            raise RequestFault(f"cot is one of {', '.join(COT_MODES)}, not {cot}")
        fields["cot"] = cot
    abc = request.get("abc")
    if abc is not None and str(abc).strip():
        fields["abc"] = str(abc)
    if request.get("cfg_scale") is not None:
        fields["cfg_scale"] = float(request["cfg_scale"])
    seed = request.get("seed")
    fields["seed"] = int(seed) if seed is not None else int.from_bytes(os.urandom(4), "little")
    return fields


def companions(entries):
    """`--companion role=path` arguments as a role to path mapping."""
    pairs = {}
    for entry in entries or []:
        role, _, path = entry.partition("=")
        if role and path:
            pairs[role] = path
    return pairs


# Each stage's share of the whole song's progress, as (where it starts, how
# much it spans): hedos never lets a job's progress move backwards, so the
# stages report one rising figure rather than restarting at zero.
STAGES = {
    "Writing the score": (0.0, 0.2),
    "Generating audio tokens": (0.2, 0.55),
    "Synthesizing": (0.75, 0.2),
    "Decoding": (0.95, 0.05),
}

# Progress is reported in thousandths of the whole song.
PROGRESS_STEPS = 1000


class Job:
    """One song's progress and cancel: turns each stage's tokens into the
    song's overall progress, and answers YuE2's `cancelled()` from the ops
    waiting on stdin."""

    def __init__(self):
        self.start, self.span = 0.0, 0.0
        self.total = 0
        self.tokens = 0
        self.cancel_requested = False
        self.shutdown_requested = False

    def stage(self, message, total=0):
        self.start, self.span = STAGES[message]
        self.total = total
        self.tokens = 0
        send_json({"event": "status", "message": message})
        self.report(0.0)

    def on_token(self, _phase, _token):
        self.tokens += 1
        if self.tokens % TOKEN_STRIDE == 0:
            # A token stage's total is a cap, not a promise, so this can
            # undershoot and then jump when the stage ends early.
            self.report(min(self.tokens / max(self.total, 1), 1.0))

    def report(self, within):
        done = self.start + self.span * within
        send_json({"event": "step", "n": int(done * PROGRESS_STEPS), "total": PROGRESS_STEPS})

    def cancelled(self):
        if not self.cancel_requested:
            op = pending_op()
            if op in ("cancel", "shutdown"):
                self.cancel_requested = True
                self.shutdown_requested = op == "shutdown"
        return self.cancel_requested


class Heartbeat:
    """Says a stage is still running while it has no progress of its own."""

    def __init__(self, message, every=HEARTBEAT_SECONDS):
        self.message = message
        self.every = every
        self.stopped = threading.Event()
        self.thread = threading.Thread(target=self.run, daemon=True)

    def run(self):
        started = time.monotonic()
        while not self.stopped.wait(self.every):
            elapsed = int(time.monotonic() - started)
            send_json({"event": "status", "message": f"{self.message} ({elapsed}s)"})

    def __enter__(self):
        self.thread.start()
        return self

    def __exit__(self, *_):
        self.stopped.set()
        self.thread.join()


def sing(pipe, fields, job, protocol):
    """Run YuE2's four stages for one request, reporting each, and return the
    decoded audio (frames by channels)."""
    try:
        request = protocol.SongRequest(**fields)
    except (TypeError, ValueError) as error:
        # YuE2's own checks: cfg_scale in [0, 20], a score only with cot on.
        raise RequestFault(str(error)) from error
    config = pipe.generation_config
    job.stage("Writing the score", config.abc.max_tokens)
    plan = pipe.plan(request=request, cancelled=job.cancelled, on_token=job.on_token)
    if plan.truncated:
        send_json({"event": "status", "message": "The score reached its length limit"})
    job.stage("Generating audio tokens", config.semantic.max_tokens)
    semantic = pipe.generate_semantic(plan, cancelled=job.cancelled, on_token=job.on_token)
    if semantic.truncated:
        send_json({"event": "status", "message": "The song reached its length limit"})
    job.stage("Synthesizing")
    with Heartbeat("Synthesizing"):
        latents = pipe.synthesize(semantic, cancelled=job.cancelled)
    if job.cancelled():
        raise CancelJob
    job.stage("Decoding")
    with Heartbeat("Decoding"):
        return pipe.decode(latents)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", required=True)
    parser.add_argument("--workdir", required=True)
    parser.add_argument("--companion", action="append")
    args = parser.parse_args()

    os.environ["HF_HUB_OFFLINE"] = "1"
    os.environ["TRANSFORMERS_OFFLINE"] = "1"

    vae = companions(args.companion).get("vae")
    if vae is None:
        send_json({"event": "error", "message": "YuE2 needs its decoder, m-a-p/YuE2-Vae"})
        return

    from yue2 import YuE2Pipeline, protocol

    # The weights are hashed against the repo's manifest whatever this says,
    # so the check costs nothing extra and stays on.
    pipe = YuE2Pipeline(args.model, vae, device="auto", progress=False)
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
        if op != "audio":
            continue

        started = time.monotonic()
        job = Job()
        try:
            fmt = output_format(request)
            fields = song_request(request)
            send_json({"event": "begin"})
            audio = sing(pipe, fields, job, protocol)
            channels = audio.shape[1] if audio.ndim == 2 else 1
            send_json({"event": "status", "message": "Encoding"})
            data = encode(audio, SAMPLE_RATE, fmt)
            send_result(send, data, fmt, SAMPLE_RATE, channels, audio.shape[0] / SAMPLE_RATE)
            send_json({"event": "done", "seconds": time.monotonic() - started})
        except (CancelJob, InterruptedError):
            send_json({"event": "cancelled"})
            if job.shutdown_requested:
                break
        except (RequestFault, FormatError) as fault:
            send_json({"event": "error", "message": str(fault), "fault": "request"})
        except Exception as error:
            send_json({"event": "error", "message": str(error)})


if __name__ == "__main__":
    main()
