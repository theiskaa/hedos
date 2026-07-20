# hedos

> _ἕδος (hédos)_, Greek for "a seat, an abode, a foundation."

<p align="center">

[![Release](https://img.shields.io/github/v/release/theiskaa/hedos?color=orange)](https://github.com/theiskaa/hedos/releases)
[![Rust](https://img.shields.io/badge/rust-edition%202024-orange)](Cargo.toml)
[![License](https://img.shields.io/badge/license-MIT-blue)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-macOS%20%26%20Linux-orange)](docs/getting-started.md)

</p>

hedos is a headless engine for the local models already on your machine. It finds them wherever they live, installs new ones, and serves each through the runtime that actually fits, all from one binary and a local HTTP gateway. There is no app and no browser wrapper. Everything runs on your hardware, offline.

It scans the places models really sit (the Ollama store, the Hugging Face cache, LM Studio's library, and loose GGUF or safetensors files in your folders) and puts them on a single shelf: text, image, and speech together, each tagged by where it came from. Weights are never moved, copied, or re-downloaded. The records simply point at the files where they already are, so every other tool on the machine still sees the same models.

For each model it detects the format, works out what it is and what it can do, and resolves it to a runtime: local GGUF through a `llama-server` process, an OpenAI-compatible endpoint, the Ollama daemon, or a managed Python sidecar (mlx-lm, mlx-vlm, speech, embeddings, diffusers, mflux, whisper). It reads each model's real context length, chat template, senses, and tool-calling dialect and serves that, so a conversation behaves the same across engines. Where a model genuinely cannot do something, the shelf says so up front instead of failing quietly.

## What is here

hedos is a Rust workspace with four crates:

- **`kernel`** holds the pure logic: model records, the registry, discovery, install and removal planning, resolution, and settings. No async, no framework, just data and the filesystem.
- **`runtime`** wraps `llama.cpp` through a subprocess pool, supervises the Python sidecars, and runs the concurrency governor that coordinates memory across engines.
- **`gateway`** is the loopback HTTP server. It speaks the OpenAI (`/v1`) and Ollama (`/api`) dialects so existing tools connect without changes.
- **`cli`** is the `hedos` binary: a thin shell over the three crates above.

The near-term surface is the CLI and the gateway. A terminal UI is planned but not built yet.

## Install

### Install script

```sh
curl -fsSL https://hedos.ai/install | bash
```

### Homebrew

```sh
brew install theiskaa/tap/hedos
```

### Cargo

Install the binary globally using cargo:

```sh
cargo install hedos
```

For the latest git version:

```sh
cargo install --git https://github.com/theiskaa/hedos
```

### Prebuilt binaries

Prebuilt versions are available in our [GitHub releases](https://github.com/theiskaa/hedos/releases/latest):

|  File  | Platform | Checksum |
|--------|----------|----------|
| [hedos-aarch64-apple-darwin.tar.xz](https://github.com/theiskaa/hedos/releases/latest/download/hedos-aarch64-apple-darwin.tar.xz) | Apple Silicon macOS | [checksum](https://github.com/theiskaa/hedos/releases/latest/download/hedos-aarch64-apple-darwin.tar.xz.sha256) |
| [hedos-x86_64-apple-darwin.tar.xz](https://github.com/theiskaa/hedos/releases/latest/download/hedos-x86_64-apple-darwin.tar.xz) | Intel macOS | [checksum](https://github.com/theiskaa/hedos/releases/latest/download/hedos-x86_64-apple-darwin.tar.xz.sha256) |
| [hedos-aarch64-unknown-linux-gnu.tar.xz](https://github.com/theiskaa/hedos/releases/latest/download/hedos-aarch64-unknown-linux-gnu.tar.xz) | ARM64 Linux | [checksum](https://github.com/theiskaa/hedos/releases/latest/download/hedos-aarch64-unknown-linux-gnu.tar.xz.sha256) |
| [hedos-x86_64-unknown-linux-gnu.tar.xz](https://github.com/theiskaa/hedos/releases/latest/download/hedos-x86_64-unknown-linux-gnu.tar.xz) | x64 Linux | [checksum](https://github.com/theiskaa/hedos/releases/latest/download/hedos-x86_64-unknown-linux-gnu.tar.xz.sha256) |

### Optional backends

hedos serves whatever your machine can already run, so nothing else is required to start. Add these when you want the runtimes that need them:

- [`uv`](https://astral.sh/uv) for the Python sidecar runtimes (mlx-lm, mlx-vlm, speech, embeddings, diffusers, mflux, whisper). They provision their own environments the first time they run; the runtime code itself ships inside the binary.
- A `llama-server` binary on the `PATH` for local GGUF files.
- The Ollama daemon for models it manages.

## Quick start

```sh
hedos scan                        # discover every model on this machine
hedos ls                          # list them with their runtime, store, and capabilities
hedos pull qwen2.5:3b             # install from ollama or hugging face
hedos run gemma3 "explain this"   # stream a completion to your terminal
hedos rm gemma3 --yes             # delete a model
hedos serve                       # start the local gateway
```

Every command takes `--json` when you want machine-readable output instead of formatted text.

## The gateway

`hedos serve` binds an OpenAI- and Ollama-compatible HTTP server to loopback (`127.0.0.1:43367` by default). Any editor, script, or agent on the machine can point at it and reach the models you own, tools and all:

```sh
curl http://127.0.0.1:43367/v1/chat/completions \
  -d '{"model":"qwen2.5","messages":[{"role":"user","content":"hi"}]}'
```

The gateway is bound to loopback and treats every local caller as trusted. It does not require a token. Keep it on `127.0.0.1`; see [SECURITY.md](SECURITY.md) for what that means.

## Managing models

The shelf is not read-only. The install service resolves a reference (a `huggingface.co` or `ollama.com` link, an `org/repo`, or a `name:tag`) and plans the install before a byte moves: the file set, the sizes, the destination, and the pinned revision.

Installs write into each platform's native habitat. Ollama models pull through the daemon's own API. Hugging Face models download into the standard hub cache layout (blobs, snapshots, refs) with `Range` resume and incremental SHA-256 verified against the LFS oids. hedos owns no weights directory, so every other tool still sees the model, and installs never touch the registry; the scanners discover the result. Gated repositories authenticate with `HF_TOKEN` from the environment.

Removal is symmetric. `hedos rm <model>` shows a deletion preview (the files, the estimated bytes) and does nothing until you pass `-y`. File-backed models are deleted from disk; Ollama models delete through the daemon.

## Runtimes

Each runtime is present whether or not its backend is installed. A capability only actually serves when its backend is available on the machine:

- **local GGUF** needs a `llama-server` binary on the `PATH`.
- **Ollama** needs the Ollama daemon running.
- **OpenAI-compatible endpoints** need a base URL and an API key in the environment.
- **the Python sidecars** (mlx-lm, mlx-vlm, speech, embeddings, diffusers, mflux) and **whisper** need [`uv`](https://astral.sh/uv), which provisions their environment on first use. Their runtime code ships inside the binary.
- **image daemons** (ComfyUI, AUTOMATIC1111) need the daemon running.

The Apple Foundation and MLX-Swift runtimes from the original macOS build are framework-bound and are intentionally out of this headless port.

## Configuration

Settings live in one human-editable file at `~/.config/hedos.toml` (or `$XDG_CONFIG_HOME/hedos.toml`). State lives under `~/.local/share/hedos` (or `$XDG_DATA_HOME/hedos`): the registry, generated artifacts, job history, the gateway's audit log, and the sidecar work directories. See [docs/configuration.md](docs/configuration.md).

## Build and test

```sh
cargo build                          # compile the workspace
cargo test                           # run the suite
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

Nothing here touches your models. Weights are only ever read.

## Documentation

Deeper guides live in [docs/](docs/README.md): getting started, the CLI reference, the gateway API, configuration, model management, and the architecture.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for how to build, test, and open a pull request. The rule of thumb is that logic lives in the kernel and the CLI stays a thin shell over it. Participation is governed by our [Code of Conduct](CODE_OF_CONDUCT.md). Security issues have a private channel in [SECURITY.md](SECURITY.md).

hedos is MIT-licensed. See [LICENSE](LICENSE).
