import argparse
import inspect
import io
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


def png_bytes(pil_image, max_side=None):
    image = pil_image
    if max_side is not None:
        image = image.copy()
        image.thumbnail((max_side, max_side))
    buffer = io.BytesIO()
    image.save(buffer, format="PNG")
    return buffer.getvalue()


class CancelJob(Exception):
    pass


LATENT_FACTORS = [
    [0.3920, 0.4054, 0.4549],
    [-0.2634, -0.0196, 0.0653],
    [0.0568, 0.1687, -0.0755],
    [-0.3112, -0.2359, -0.2076],
]


def latent_preview(latents, torch, image_cls):
    try:
        if latents is None or latents.shape[1] != 4:
            return None
        sample = latents[0].to(torch.float32).cpu()
        factors = torch.tensor(LATENT_FACTORS)
        rgb = torch.einsum("chw,cr->hwr", sample, factors)
        rgb = ((rgb + 1.0) / 2.0).clamp(0, 1).mul(255).to(torch.uint8).numpy()
        return png_bytes(image_cls.fromarray(rgb), max_side=256)
    except Exception:
        return None


class StepEmitter:
    def __init__(self, total, torch, image_cls):
        self.total = total
        self.torch = torch
        self.image_cls = image_cls
        self.cancelled = False
        self.shutdown_requested = False

    def __call__(self, pipe, step, timestep, callback_kwargs):
        send_json({"event": "step", "n": step + 1, "total": self.total})
        preview = latent_preview(callback_kwargs.get("latents"), self.torch, self.image_cls)
        if preview is not None:
            send_json({"event": "preview", "format": "png"})
            send(2, preview)
        op = pending_op()
        if op in ("cancel", "shutdown"):
            self.cancelled = True
            self.shutdown_requested = op == "shutdown"
            if hasattr(pipe, "_interrupt"):
                pipe._interrupt = True
            else:
                raise CancelJob
        return callback_kwargs


def detect_variant(model_path):
    for _, _, files in os.walk(model_path):
        for name in files:
            if name.endswith(".fp16.safetensors"):
                return "fp16"
    return None


def load_pipeline(model_path, torch, pipeline_cls):
    variant = detect_variant(model_path)
    try:
        pipe = pipeline_cls.from_pretrained(
            model_path, torch_dtype=torch.float16, local_files_only=True, variant=variant
        )
    except Exception:
        fallback = None if variant is not None else "fp16"
        pipe = pipeline_cls.from_pretrained(
            model_path, torch_dtype=torch.float16, local_files_only=True, variant=fallback
        )
    pipe = pipe.to("mps")
    pipe.set_progress_bar_config(disable=True)
    return pipe


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", required=True)
    parser.add_argument("--name", default=None)
    parser.add_argument("--workdir", required=True)
    args = parser.parse_args()

    os.environ.setdefault("HF_HUB_OFFLINE", "1")

    import torch
    from diffusers import AutoPipelineForText2Image
    from PIL import Image

    pipe = load_pipeline(args.model, torch, AutoPipelineForText2Image)
    supports_callback = "callback_on_step_end" in inspect.signature(pipe.__call__).parameters
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
        if op != "image":
            continue

        prompt = request.get("prompt", "")
        steps = max(1, int(request.get("steps", 2)))
        guidance = float(request.get("guidance", 0.0))
        size = str(request.get("size", "1024x1024"))
        seed = request.get("seed")
        seed = int(seed) if seed is not None else int.from_bytes(os.urandom(4), "little")
        try:
            width, height = (int(part) for part in size.split("x"))
        except ValueError:
            send_json({"event": "error", "message": f"size {size} is not WIDTHxHEIGHT"})
            continue

        started = time.monotonic()
        emitter = StepEmitter(steps, torch, Image)
        options = {
            "prompt": prompt,
            "num_inference_steps": steps,
            "guidance_scale": guidance,
            "width": width,
            "height": height,
            "generator": torch.Generator("mps").manual_seed(seed),
            "output_type": "pil",
        }
        if supports_callback:
            options["callback_on_step_end"] = emitter
        try:
            send_json({"event": "begin"})
            result = pipe(**options)
            if emitter.cancelled:
                send_json({"event": "cancelled"})
                if emitter.shutdown_requested:
                    break
                continue
            data = png_bytes(result.images[0])
            send_json({"event": "image", "format": "png", "index": 0, "count": 1})
            send(2, data)
            send_json({"event": "done", "seconds": time.monotonic() - started})
        except CancelJob:
            send_json({"event": "cancelled"})
            if emitter.shutdown_requested:
                break
        except Exception as error:
            send_json({"event": "error", "message": str(error)})


main()
