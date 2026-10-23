import argparse
import inspect
import json
import os
import select
import struct
import time

from audio_output import FormatError, encode, output_format, send_result

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


# How each diffusers pipeline names what the job contract calls `duration`,
# tried in order against the pipeline's own signature.
DURATION_KEYS = ("audio_end_in_s", "audio_length_in_s", "audio_duration", "duration")

LIBRARY_MODEL_TYPES = ("musicgen", "musicgen_melody")


class RequestFault(Exception):
    """A request this model cannot answer as asked: refused, not retried."""


class CancelJob(Exception):
    pass


def family_of(model_dir):
    """What drives the model in `model_dir`: ("diffusers", class name) for a
    pipeline, ("musicgen", model type) for MusicGen, else None."""
    index = os.path.join(model_dir, "model_index.json")
    if os.path.isfile(index):
        with open(index) as handle:
            return ("diffusers", json.load(handle).get("_class_name"))
    config = os.path.join(model_dir, "config.json")
    if os.path.isfile(config):
        with open(config) as handle:
            model_type = json.load(handle).get("model_type")
        if model_type in LIBRARY_MODEL_TYPES:
            return ("musicgen", model_type)
    return None


def pipeline_options(parameters, request):
    """The keyword arguments a diffusers audio pipeline whose call takes
    `parameters` gets for `request`. Raises RequestFault for a contract key the
    pipeline has no way to take, so lyrics never vanish into an instrumental."""
    options = {"prompt": str(request.get("prompt", ""))}
    duration_key = next((key for key in DURATION_KEYS if key in parameters), None)
    lyrics = str(request.get("lyrics") or "").strip() or None
    # A model that sings is told to stay instrumental the way ACE-Step's own
    # lyrics format says it; one that never sings is instrumental already.
    if request.get("instrumental") and lyrics is None and "lyrics" in parameters:
        lyrics = "[instrumental]"
    supplied = {
        "lyrics": ("lyrics", lyrics),
        "duration": (duration_key, request.get("duration")),
        "steps": ("num_inference_steps", request.get("steps")),
        "guidance": ("guidance_scale", request.get("guidance")),
        "negative_prompt": ("negative_prompt", request.get("negative_prompt")),
    }
    for contract_key, (call_key, value) in supplied.items():
        if value is None or (isinstance(value, str) and not value.strip()):
            continue
        if call_key is None or call_key not in parameters:
            raise RequestFault(f"this model does not take {contract_key}")
        options[call_key] = coerce(contract_key, value)
    return options


def coerce(contract_key, value):
    if contract_key == "steps":
        return max(1, int(value))
    if contract_key in ("duration", "guidance"):
        return float(value)
    return str(value)


def musicgen_tokens(duration, frame_rate):
    """How many audio tokens MusicGen writes for `duration` seconds."""
    return max(1, int(round(float(duration) * float(frame_rate))))


def check_musicgen_request(request):
    for key in ("lyrics", "steps", "negative_prompt"):
        value = request.get(key)
        if value is not None and not (isinstance(value, str) and not value.strip()):
            raise RequestFault(f"this model does not take {key}")


class Steps:
    """Reports progress and watches for a cancel between steps."""

    def __init__(self):
        self.total = 0
        self.shutdown_requested = False

    def tick(self, step):
        send_json({"event": "step", "n": step, "total": self.total})
        op = pending_op()
        if op in ("cancel", "shutdown"):
            self.shutdown_requested = op == "shutdown"
            raise CancelJob


def cuda_runs(torch):
    # A CUDA build calls a card it has no kernels for available, then fails
    # every job on it; such a card is left to the CPU.
    if not torch.cuda.is_available():
        return False
    major, minor = torch.cuda.get_device_capability()
    built = [
        int("".join(c for c in arch[3:] if c.isdigit()))
        for arch in torch.cuda.get_arch_list()
        if arch.startswith("sm_")
    ]
    return bool(built) and major * 10 + minor >= min(built)


def device_of(torch):
    if torch.backends.mps.is_available():
        return "mps"
    if cuda_runs(torch):
        return "cuda"
    return "cpu"


def load_diffusers(model_dir, torch, device):
    from diffusers import DiffusionPipeline

    # Half precision only where it is the norm: an audio decoder in float16 on
    # Metal or the CPU can return silence.
    dtype = torch.float16 if device == "cuda" else torch.float32
    pipe = DiffusionPipeline.from_pretrained(model_dir, torch_dtype=dtype, local_files_only=True)
    pipe = pipe.to(device)
    pipe.set_progress_bar_config(disable=True)
    return pipe


def sample_rate_of(pipe):
    # ACE-Step names its own rate; the others carry it on their decoder.
    if getattr(pipe, "sample_rate", None):
        return int(pipe.sample_rate)
    for owner in ("vae", "vocoder"):
        component = getattr(pipe, owner, None)
        for holder in (component, getattr(component, "config", None)):
            rate = getattr(holder, "sampling_rate", None) or getattr(holder, "sample_rate", None)
            if rate:
                return int(rate)
    return 48000


def run_diffusers(pipe, parameters, request, tracker, torch, device):
    options = pipeline_options(parameters, request)
    tracker.total = int(
        options.get("num_inference_steps") or parameters.get("num_inference_steps") or 0
    )
    if "callback_on_step_end" in parameters:

        def on_step_end(_pipe, step, _timestep, callback_kwargs):
            tracker.tick(step + 1)
            return callback_kwargs

        options["callback_on_step_end"] = on_step_end
    elif "callback" in parameters:
        options["callback"] = lambda step, _timestep, _latents: tracker.tick(step + 1)
        if "callback_steps" in parameters:
            options["callback_steps"] = 1
    seed = request.get("seed")
    seed = int(seed) if seed is not None else int.from_bytes(os.urandom(4), "little")
    generator_device = "cpu" if device == "mps" else device
    options["generator"] = torch.Generator(generator_device).manual_seed(seed)
    if "output_type" in parameters:
        options["output_type"] = "np"
    result = pipe(**options)
    audio = result.audios[0]
    audio = audio.float().cpu().numpy() if hasattr(audio, "cpu") else audio
    return audio, sample_rate_of(pipe)


def load_musicgen(model_dir, model_type, torch, device):
    import transformers

    # MusicGen-Melody conditions on a prefix rather than cross-attention, so
    # its checkpoints only load into their own class.
    model_class = (
        transformers.MusicgenMelodyForConditionalGeneration
        if model_type == "musicgen_melody"
        else transformers.MusicgenForConditionalGeneration
    )
    processor = transformers.AutoProcessor.from_pretrained(model_dir, local_files_only=True)
    model = model_class.from_pretrained(model_dir, local_files_only=True)
    return processor, model.to(device)


# MusicGen reports progress every this many tokens: often enough to show
# movement, rarely enough not to flood the channel.
TOKEN_STRIDE = 25


def step_counter(tracker, stride, torch, stopping_criteria_class):
    """A stopping criterion that never stops generation itself: transformers
    calls it once per generated step, which is where progress is reported and
    a cancel is noticed. (Its streamer argument is not passed on to sampling,
    so a streamer would never hear of a step.)"""

    class StepCounter(stopping_criteria_class):
        def __init__(self):
            self.steps = 0

        def __call__(self, input_ids, scores, **kwargs):
            self.steps += 1
            if self.steps % stride == 0:
                tracker.tick(min(self.steps, tracker.total))
            return torch.zeros(input_ids.shape[0], dtype=torch.bool, device=input_ids.device)

    return StepCounter()


def run_musicgen(loaded, request, tracker, torch, device):
    from transformers import StoppingCriteriaList
    from transformers.generation.stopping_criteria import StoppingCriteria

    processor, model = loaded
    check_musicgen_request(request)
    encoder = model.config.audio_encoder
    tokens = musicgen_tokens(request.get("duration", 10.0), encoder.frame_rate)
    tracker.total = tokens

    seed = request.get("seed")
    torch.manual_seed(int(seed) if seed is not None else int.from_bytes(os.urandom(4), "little"))
    inputs = processor(text=[str(request.get("prompt", ""))], padding=True, return_tensors="pt")
    inputs = {key: value.to(device) for key, value in inputs.items()}
    counter = step_counter(tracker, TOKEN_STRIDE, torch, StoppingCriteria)
    options = {
        "max_new_tokens": tokens,
        "do_sample": True,
        "stopping_criteria": StoppingCriteriaList([counter]),
    }
    if request.get("guidance") is not None:
        options["guidance_scale"] = float(request["guidance"])
    values = model.generate(**inputs, **options)
    return values[0].float().cpu().numpy(), int(encoder.sampling_rate)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", required=True)
    parser.add_argument("--name", default=None)
    parser.add_argument("--workdir", required=True)
    args = parser.parse_args()

    os.environ.setdefault("HF_HUB_OFFLINE", "1")
    os.environ.setdefault("TRANSFORMERS_OFFLINE", "1")

    import torch

    family = family_of(args.model)
    if family is None:
        send_json({"event": "error", "message": "no audio pipeline or MusicGen config here"})
        return
    device = device_of(torch)
    if family[0] == "diffusers":
        loaded = load_diffusers(args.model, torch, device)
        signature = inspect.signature(loaded.__call__).parameters
        parameters = {
            name: (param.default if param.default is not inspect.Parameter.empty else None)
            for name, param in signature.items()
        }
    else:
        loaded = load_musicgen(args.model, family[1], torch, device)
        parameters = {}
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
        tracker = Steps()
        try:
            fmt = output_format(request)
            send_json({"event": "begin"})
            if family[0] == "diffusers":
                audio, rate = run_diffusers(loaded, parameters, request, tracker, torch, device)
            else:
                audio, rate = run_musicgen(loaded, request, tracker, torch, device)
            channels = audio.shape[0] if audio.ndim == 2 else 1
            seconds = audio.shape[-1] / rate
            send_json({"event": "status", "message": "Encoding"})
            # diffusers and MusicGen hand back channels first; a file is frames first.
            frames = audio.T if audio.ndim == 2 else audio
            send_result(send, encode(frames, rate, fmt), fmt, rate, channels, seconds)
            send_json({"event": "done", "seconds": time.monotonic() - started})
        except CancelJob:
            send_json({"event": "cancelled"})
            if tracker.shutdown_requested:
                break
        except (RequestFault, FormatError) as fault:
            send_json({"event": "error", "message": str(fault), "fault": "request"})
        except Exception as error:
            send_json({"event": "error", "message": str(error)})


if __name__ == "__main__":
    main()
