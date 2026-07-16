# How your models run

Hedos is built on one promise: it is designed for every model. The models on your Mac come in many formats, from many tools, with different senses and different ideas of how a conversation is shaped. Hedos does not force them into a single mold. For each model it detects the format, decides what the model actually is and what it can do, and resolves it to the runtime that fits — the engine best suited to run that particular file. Then it serves the model as what it truly is, so the experience stays consistent no matter which engine is doing the work underneath.

## One shelf, many engines

When Hedos scans your machine it reads each model closely: its real context length, its chat template, its senses (whether it can see images, speak, transcribe, embed), and the dialect it uses to call tools. Those facts travel with the model onto the shelf, and they are what the runtime serves — not a lowest-common-denominator guess. A model with a 128k context window is served with that window; a model that ships its own chat template is prompted through that template; a model that speaks a particular tool-calling convention keeps it.

The point is that a conversation behaves the same across engines. You can switch from a GGUF model running in-process to an MLX model in a Python sidecar to a model served by Ollama, and the surface you talk to does not change shape underneath you. The engine is an implementation detail; the model is what you deal with.

And where a model genuinely cannot do something, the shelf says so up front. If a model has no vision, the attach-image affordance is not offered for it. If Hedos cannot yet resolve a model to any runtime it can run, it is marked as needing a recipe rather than silently pretending to work and failing at the first message. The honesty is deliberate: you learn what a model can and cannot do before you click, not after.

## The runtimes

Hedos routes each model to one of several kinds of engine. You do not choose the runtime by hand in the normal case — resolution picks the one that fits — but it helps to know what is running your model and why.

**In-process engines** run inside Hedos itself, with nothing else to install:

- **llama.cpp** serves GGUF models — the quantized text models that dominate the local ecosystem (from Ollama's store, LM Studio's library, or loose `.gguf` files in your folders). This is the workhorse for text.
- **MLX-Swift** serves many MLX-format models natively, using Apple's MLX framework directly from Swift.

**Managed Python sidecars** are small isolated Python programs Hedos starts on demand for the model families the in-process engines don't cover. Each is sandboxed with no network access, and reaches only the model's own files:

- **mlx-lm** — MLX text models beyond what MLX-Swift serves in-process.
- **mlx-vlm** — MLX vision-language models, the ones that can see images.
- **diffusers** — Hugging Face diffusion models for image generation.
- **mflux** — FLUX image models on MLX.
- **mlx-audio** — Kokoro and other MLX speech models, for turning text into spoken audio.
- **whisper** — speech-to-text transcription.
- **embeddings** — models that turn text into vectors.

**Ollama** is served through its own daemon: if you already run Ollama, Hedos talks to it over its local API rather than duplicating your models. **ComfyUI** and **Automatic1111** work the same way for image generation — Hedos connects to the running daemon instead of managing the weights itself.

**Apple Foundation** is the on-device language model Apple ships inside macOS. It has no file to download; if your Mac provides it, it appears on the shelf like any other model.

**Any OpenAI-compatible endpoint** you point Hedos at is served too. You supply the base URL and key, and models behind that endpoint join the shelf. This is the one runtime that can reach beyond your machine — it goes wherever you tell it to.

## The first run of a Python model

The first time you run a model that needs a Python sidecar — a diffusers image model, a whisper transcription, an MLX vision model — there is a one-time setup step, and it is slower than the runs that follow.

Hedos provisions an isolated Python environment for that runtime using [`uv`](https://astral.sh/uv). The set of packages is hash-locked: Hedos ships an exact lockfile per runtime, and the environment is keyed to that lockfile so you get precisely the versions it was tested against. You need a Python available on your Mac for this to work. Nothing is installed globally and nothing touches your system Python — each runtime lives in its own directory, and installing one never disturbs another.

Once built, the environment is cached and reused. Subsequent runs of anything in that family skip the setup entirely and start straight into the model. So the slow first run is the environment being assembled once, not something that repeats. (Building it does fetch the packages the first time, which is the one moment a sidecar needs the network; after that the sandboxed sidecar runs offline.)

## Memory: what can be loaded at once

Your Mac has only so much memory, and a large model can occupy a lot of it. Hedos manages loading and unloading for you so you don't have to think about it. When you use a model, Hedos loads it; when you move on, it keeps the model warm for a short while in case you come back, then releases it.

The consequence worth knowing: two heavyweight models generally cannot be resident at the same time. When you start a heavy model and another heavy one is already loaded, Hedos evicts the first to make room — waiting, if it is mid-generation, for that work to finish before unloading it. This is automatic and safe; the only thing you may notice is that switching between two large models has a load pause, because the second one is coming into memory as the first leaves. Small models coexist freely.

## Everything on your hardware

With the single exception of an OpenAI-compatible endpoint you deliberately point elsewhere, everything here runs locally: on your Apple Silicon, offline, on Apple's MLX foundation. The in-process engines run inside the app, the Python sidecars run sandboxed on your machine, and the daemons are ones already running on your Mac. Your weights are never moved, copied, or re-downloaded to make any of this work — the runtimes read the files where they already sit. The models are yours, and they run where they are.
