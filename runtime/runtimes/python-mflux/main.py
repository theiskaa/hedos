import argparse
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


class StepEmitter:
    def __init__(self, flux, latent_creator, image_util):
        self.flux = flux
        self.latent_creator = latent_creator
        self.image_util = image_util
        self.shutdown_requested = False

    def call_in_loop(self, t, seed, prompt, latents, config, time_steps):
        send_json({"event": "step", "n": t + 1, "total": config.num_inference_steps})
        preview = self.preview(latents, seed, prompt, config)
        if preview is not None:
            send_json({"event": "preview", "format": "png"})
            send(2, preview)
        op = pending_op()
        if op in ("cancel", "shutdown"):
            self.shutdown_requested = op == "shutdown"
            raise KeyboardInterrupt

    def preview(self, latents, seed, prompt, config):
        try:
            unpacked = self.latent_creator.unpack_latents(
                latents=latents, height=config.height, width=config.width
            )
            if hasattr(self.flux.vae, "decode_packed_latents"):
                decoded = self.flux.vae.decode_packed_latents(unpacked)
            else:
                decoded = self.flux.vae.decode(unpacked)
            generated = self.image_util.to_image(
                decoded_latents=decoded,
                config=config,
                seed=seed,
                prompt=prompt,
                quantization=self.flux.bits,
                generation_time=0,
            )
            return png_bytes(generated.image, max_side=256)
        except Exception:
            return None


def transformer_uses_guidance(model_path):
    config_path = os.path.join(model_path, "transformer", "config.json")
    try:
        with open(config_path, "r", encoding="utf-8") as handle:
            return bool(json.load(handle).get("guidance_embeds", False))
    except (OSError, ValueError):
        return False


def resolve_model_config(model_config_cls, model_path, name):
    try:
        return model_config_cls.from_name(model_name=name)
    except Exception:
        pass
    alias = "dev" if transformer_uses_guidance(model_path) else "schnell"
    return model_config_cls.from_name(model_name=alias)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", required=True)
    parser.add_argument("--name", default=None)
    parser.add_argument("--workdir", required=True)
    args = parser.parse_args()

    os.environ.setdefault("HF_HUB_OFFLINE", "1")

    from mflux.models.common.config.model_config import ModelConfig
    from mflux.models.flux.latent_creator.flux_latent_creator import FluxLatentCreator
    from mflux.models.flux.variants.txt2img.flux import Flux1
    from mflux.utils.exceptions import StopImageGenerationException
    from mflux.utils.image_util import ImageUtil

    name = args.name or os.path.basename(args.model)
    model_config = resolve_model_config(ModelConfig, args.model, name)
    flux = Flux1(model_config=model_config, model_path=args.model)
    emitter = StepEmitter(flux, FluxLatentCreator, ImageUtil)
    flux.callbacks.register(emitter)
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
        steps = max(1, int(request.get("steps", 4)))
        guidance = float(request.get("guidance", 4.0))
        size = str(request.get("size", "1024x1024"))
        seed = request.get("seed")
        seed = int(seed) if seed is not None else int.from_bytes(os.urandom(4), "little")
        try:
            width, height = (int(part) for part in size.split("x"))
        except ValueError:
            send_json({"event": "error", "message": f"size {size} is not WIDTHxHEIGHT"})
            continue

        started = time.monotonic()
        try:
            send_json({"event": "begin"})
            image = flux.generate_image(
                seed=seed,
                prompt=prompt,
                num_inference_steps=steps,
                height=height,
                width=width,
                guidance=guidance,
            )
            data = png_bytes(image.image)
            send_json({"event": "image", "format": "png", "index": 0, "count": 1})
            send(2, data)
            send_json({"event": "done", "seconds": time.monotonic() - started})
        except (KeyboardInterrupt, StopImageGenerationException):
            send_json({"event": "cancelled"})
            if emitter.shutdown_requested:
                break
        except Exception as error:
            send_json({"event": "error", "message": str(error)})


main()
