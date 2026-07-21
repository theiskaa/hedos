# Getting started

This walks from a clean checkout to a running gateway.

## Prerequisites

- A recent Rust toolchain with edition 2024 support. Install it from [rustup.rs](https://rustup.rs) if you do not have it.
- [`uv`](https://astral.sh/uv) on your `PATH`, if you want to run the Python sidecar runtimes (mlx-lm, speech, embeddings, and the rest). It provisions their environments the first time they run; the runtime code itself ships inside the binary.
- Optionally, whatever backends serve the models you care about: a `llama-server` binary for local GGUF files, the Ollama daemon, or an API key for a remote OpenAI-compatible endpoint.

hedos runs on macOS and Linux.

## Build

```sh
git clone https://github.com/theiskaa/hedos
cd hedos
cargo build --release
```

The binary is at `target/release/hedos`. Copy it somewhere on your `PATH`, or run it in place with `cargo run --release --bin hedos -- <command>`.

## Find your models

hedos does not download anything to get started. It reads the models already on your machine.

```sh
hedos scan
```

This scans the Ollama store, the Hugging Face cache, LM Studio's library, and loose GGUF or safetensors files in the usual folders, then reconciles them into a registry and resolves each to a runtime. It prints a short summary and any issues it found.

```sh
hedos ls
```

`ls` lists the shelf: each model's name, the runtime it resolved to, the store it came from, whether it will fit in this machine's memory, and what it can do. A filled dot in the first column means the model is currently warm. If the shelf is empty, `ls` runs a scan for you on first use.

## Run a completion

Pick a model from `hedos ls` and stream a completion:

```sh
hedos run gemma3 "write a haiku about rust"
```

The reply streams to your terminal as it generates. You can pass `--system` for a system prompt, `--max-tokens` to cap the length, and `--temperature` to adjust sampling. For a back-and-forth session that reads turns from stdin, use `hedos chat <model>` and press Ctrl-D to end.

The model name does not have to be exact. hedos matches on the id, then the name, then a unique substring, and tells you if a query is ambiguous.

## Install something new

```sh
hedos pull qwen2.5:3b
```

hedos works out the provider from the reference shape (an `org/repo` goes to Hugging Face, a `name:tag` goes to Ollama), plans the install, and shows download progress. Ctrl-C cancels. When it finishes, it runs a scan so the new model appears on the shelf. Pass `--from ollama` or `--from hf` to force a provider.

## Serve the shelf

```sh
hedos serve
```

This starts the local gateway on `127.0.0.1:43367` and prints the base URL. Leave it running and point any OpenAI-, Ollama-, or Anthropic-compatible tool at it. Ctrl-C stops it cleanly. See the [gateway guide](gateway.md) for the full API.

## Drive a coding agent

```sh
hedos launch opencode
```

`hedos launch` runs a coding harness — Claude Code, OpenCode, Aider, Goose, or Crush — against a model on your shelf, with nothing to configure. It starts a gateway on a free port inside the same process, points the harness at it, and stops both when the harness exits; your own harness config is left untouched. Omit the harness to pick from the ones you have installed, and pass `-m <model>` to choose the model. See the [CLI reference](cli.md) for the details.

## Where things live

- Settings: `~/.config/hedos.toml` (or `$XDG_CONFIG_HOME/hedos.toml`).
- State: `~/.local/share/hedos` (or `$XDG_DATA_HOME/hedos`), which holds the registry, generated artifacts, job history, and the gateway's audit log.

Neither of these contains your model weights. hedos only points at where the weights already sit.
