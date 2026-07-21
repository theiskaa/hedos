# Models

hedos treats the models on your machine as one shelf, no matter where they came from or what they do. This page covers how it finds them, installs new ones, and removes them, and the one rule that holds through all of it: your weights are only ever read.

## Discovery

`hedos scan` looks in the places models actually live:

- the Ollama store,
- the Hugging Face hub cache (blobs, snapshots, refs),
- LM Studio's library,
- and loose GGUF or safetensors files in your Downloads and model folders.

For each one it reads the format, works out the modality and capabilities, and reconciles the result into the registry. A model that moved is migrated, a model that vanished is marked missing, and duplicates are noted. Nothing is copied or relocated. The registry records point at the files where they already are, so every other tool still sees the same models.

Discovery also resolves each model to a runtime. It reads the model's shape (context length, chat template, senses, tool dialect) and picks the engine that fits, so the shelf tells you not just what you have but how each model will actually be served — and, from each model's footprint against this machine's memory, whether it will run comfortably.

## Installing

`hedos pull <reference>` resolves a reference and plans the install before anything downloads. The reference can be a Hugging Face repo (`org/model`), an Ollama tag (`gemma3:4b`), or a link to either. hedos infers the provider from the shape, and `--from ollama` or `--from hf` forces it. Run `hedos pull` with no reference in a terminal to search Hugging Face by keyword or to pick from a short list of models that fit your machine's RAM.

Installs write into each platform's native layout:

- **Ollama** models pull through the daemon's own API.
- **Hugging Face** models download into the standard hub cache: content-addressed blobs, a snapshot directory of symlinks, and a ref pointing at the revision. Downloads resume with HTTP `Range`, and each file is verified with SHA-256 against its LFS oid.

hedos owns no weights directory of its own, so the moment an install finishes, every other tool sees the model too. Installs do not touch the registry directly; the follow-up scan discovers the result. Gated repositories authenticate with `HF_TOKEN` from the environment.

## Removing

Removal is symmetric with install. `hedos rm <model>` first shows a deletion preview: the items that would go and the estimated size. In a terminal it then asks for a yes/no confirmation and removes nothing unless you agree; in a script or pipe it removes nothing unless you pass `-y`.

- File-backed models are deleted from disk.
- Ollama models delete through the daemon.

The preview is honest about what remains. If duplicate copies of the same weights exist elsewhere on the machine, removing one does not remove the others, and the shelf will still show them.

## Runtimes at a glance

A model resolves to whichever of these fits it, and each serves only when its backend is present:

- **local GGUF** through a `llama-server` subprocess, for `.gguf` files.
- **Ollama** for models the daemon manages.
- **OpenAI-compatible endpoints** for remote models reached by URL and key.
- **Python sidecars** for mlx-lm, mlx-vlm, speech, embeddings, diffusers, and mflux.
- **whisper** for transcription.
- **image daemons** (ComfyUI, AUTOMATIC1111) for models they serve.

The Apple Foundation and MLX-Swift runtimes from the original macOS build are framework-bound and are not part of this headless port. A model that would need one of those still appears on the shelf, but hedos will tell you it cannot serve it here rather than dropping it.
