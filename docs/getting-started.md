# Getting started

This guide installs hedos and walks through the first five minutes: finding the models already on your machine, running one, opening the shelf, and serving it to other tools.

hedos runs on macOS and Linux.

## Install

The quickest way is the install script. It downloads the latest release for your platform, checks its SHA-256, and puts `hedos` in `~/.local/bin`:

```sh
curl -fsSL https://hedos.ai/install | bash
```

Set `HEDOS_BIN_DIR` to install somewhere else. If the directory is not on your `PATH`, the script tells you the line to add.

Other ways to install:

```sh
brew install theiskaa/tap/hedos    # Homebrew
cargo install hedos                # from crates.io
```

Prebuilt archives for Apple Silicon and Intel macOS and for ARM64 and x64 Linux are on the [releases page](https://github.com/theiskaa/hedos/releases/latest); the [README](../README.md#prebuilt-binaries) links each one with its checksum.

Check that it worked:

```sh
hedos --version
```

### Build from source

You need a recent stable Rust toolchain with edition 2024 support. Install it from [rustup.rs](https://rustup.rs) if you do not have it.

```sh
git clone https://github.com/theiskaa/hedos
cd hedos
cargo build --release
```

The binary is at `target/release/hedos`. Copy it somewhere on your `PATH`, or run it in place with `cargo run --release --bin hedos -- <command>`.

On a Mac whose SDK has Apple's `FoundationModels` framework, the build also produces the Apple Intelligence bridge, `libhedos_apple_shim.dylib`, under `target/release/build/hedos-runtime-*/out/`. A binary run in place finds it there. If you copy the binary elsewhere, copy the library next to it too, or Apple Intelligence will report that its bridge is not built in. The [README](../README.md#runtimes) shows how. No other runtime needs this, and installs that copy only the binary (the script, Homebrew, `cargo install`) do not include it.

### Optional backends

hedos serves whatever your machine can already run, so nothing else is required to start. Add these when you want the runtimes that need them:

- [`uv`](https://astral.sh/uv), for the Python sidecar runtimes (mlx-lm, mlx-vlm, speech, embeddings, diffusers, mflux, whisper). It builds their environments the first time they run; the runtime code ships inside the binary.
- A `llama-server` binary on your `PATH`, for local GGUF files.
- Ollama, for the models it manages. If it is installed but not running, hedos starts it.
- An API key in `HEDOS_OPENAI_API_KEY`, for a remote OpenAI-compatible endpoint that needs one.

## Find your models

hedos does not download anything to get started. It reads the models already on your machine.

```sh
hedos scan
```

This scans the Ollama store, the Hugging Face cache, LM Studio's library, and loose GGUF and safetensors files in `~/Downloads`, `~/Models`, and any folder you add in [the settings](configuration.md). It reconciles what it finds into a registry and resolves each model to the runtime that serves it, then prints a summary:

```
Found 13 models on this Mac (2 in Ollama, 10 in the Hugging Face cache, 1 built in). Total: 27 GB.

ollama              2  14.3 GB
huggingface-cache  10  12.7 GB
builtin             1
```

When it applies, the summary also lists:

- any model that can go because another model already keeps every file it holds, with the space removing it frees,
- how many models are too big for this machine,
- any issues it found, as `issue:` lines.

Nothing is moved or copied. The registry only records where each model's files already sit.

## List the shelf

```sh
hedos ls
```

```
   NAME                        RUNTIME        STORE              FIT   CAPABILITIES
○  gemma4:latest               ollama         ollama             fits  chat, complete, tools
○  Llama-3.2-3B-Instruct-4bit  python:mlx-lm  huggingface-cache  fits  chat, complete, tools
○  llava:latest                ollama         ollama             fits  chat, complete, see
○  Qwen2.5-0.5B-Instruct-4bit  python:mlx-lm  huggingface-cache  fits  chat, complete, tools
```

Each row is one model: its name, the runtime it resolved to, the store it came from, whether it fits in what its runtime may use here (`fits`, `tight`, or `too big`; on Apple Silicon, the GPU's share of memory), and what it can do.

The first column is the model's state: `●` is warm (loaded right now), `○` is cold, and `✕` means its weights are gone from disk. A model with no runtime that can serve it shows a dash in the runtime column.

If the shelf is empty, `ls` runs a scan for you first. `hedos ls --scan` rescans on purpose, and `hedos ls --capability see` shows only the models that serve one capability.

Some models need a runtime you approve first. `ls` says so under the table, for example `laya can run on python:laya, which needs your approval`. See [Manifest runtimes and consent](configuration.md#manifest-runtimes-and-consent).

## Run a completion

Pick a model from `hedos ls` and stream a completion:

```sh
hedos run qwen2.5 "write a haiku about rust"
```

```
Borrowed, never owned,
the compiler holds the line.
Fearless threads at rest.
```

The reply streams to your terminal as it is generated. The first run of a model can take a moment while its runtime starts (and, for a Python runtime, while `uv` builds its environment); a spinner stands in until the first token.

The model name does not have to be exact. hedos tries the id, then the exact name (ignoring case), then a unique substring, and tells you when a query matches more than one model. Here `qwen2.5` matches `Qwen2.5-0.5B-Instruct-4bit`.

A few useful flags:

- `--system "..."` sets a system prompt for this run.
- `--max-tokens <n>` caps the length.
- `--temperature <t>` adjusts sampling.
- `--image photo.png` attaches an image for a vision model (one with `see`). Repeat it for several.

Leave out the model or the prompt and hedos asks for them. For a back-and-forth conversation, use `hedos chat <model>`; press Ctrl-D to end it.

A decision model (a judge such as laya) does not chat. Once its runtime is approved, `hedos run laya` opens a composer for a typed question instead, and the answer comes back as a share for each option.

## Open the shelf

```sh
hedos shelf
```

`hedos shelf` is the same shelf as a screen you keep open: every model with its runtime, store and size, the selected model's fit and capabilities, what is loaded and how much memory it takes, your downloads, and what the gateway served in the last day.

What to press first:

| Key | Does |
|---|---|
| `j` / `k` or the arrows | Move through the shelf. |
| `enter` | Expand the selected model. |
| `t` | Try it: a conversation with the model, right there. |
| `?` | Show every key. |
| `q` | Quit. |

The footer only lists the keys that apply to the model under the cursor. If the shelf is empty, the first screen offers to pull a model. The [shelf guide](shelf.md) covers the whole screen.

## Install something new

Not sure what this machine can run? Ask:

```sh
hedos recommend
```

It reads the hardware (on Apple Silicon, the share of memory the GPU may give a model; on Linux, your NVIDIA or AMD cards) and lists the models worth pulling for each kind, chat, code, voice and image, with what to install first if an engine they need is missing. See [`hedos recommend`](cli.md#hedos-recommend).

```sh
hedos pull qwen2.5:3b
```

hedos works out the provider from the shape of the reference: an `org/repo` goes to Hugging Face, a `name:tag` goes to Ollama, and a `huggingface.co` or `ollama.com` link goes where it points. Pass `--from ollama` or `--from hf` to choose. It plans the install (the files, their sizes, where they go), then downloads in a worker process of its own and shows the progress.

Ctrl-C detaches and leaves the download running; `hedos pull ls` lists every pull, and `hedos pull attach <id>` follows one again. When the files have landed, a scan puts the model on the shelf. See [models.md](models.md) for how installs and removal work.

## Serve the shelf

```sh
hedos serve
```

```
gateway listening on http://127.0.0.1:43367/v1
```

This starts the local gateway on `127.0.0.1:43367`, prints its base URL, and notes that any local client is allowed. Leave it running and point any OpenAI-, Ollama-, or Anthropic-compatible tool at it:

```sh
curl http://127.0.0.1:43367/v1/chat/completions \
  -d '{"model":"Qwen2.5-0.5B-Instruct-4bit","messages":[{"role":"user","content":"hi"}]}'
```

The gateway matches model names more strictly than the CLI: use the name `hedos ls` shows (or its id), not a fragment of it. `GET /v1/models` lists them. An Ollama name may leave off `:latest`.

`-p <port>` picks another port. Ctrl-C stops taking new requests and waits for the ones in flight; a second Ctrl-C cuts them. See the [gateway guide](gateway.md) for every endpoint.

## Drive a coding agent

```sh
hedos launch opencode -m qwen2.5
```

```
OpenCode · Qwen2.5-0.5B-Instruct-4bit · gateway on 127.0.0.1:52817
```

`hedos launch` runs a coding harness (Claude Code, OpenCode, Aider, Goose, or Crush) against a model on your shelf, with nothing to configure. It starts a private gateway on a free port inside the same process, points the harness at it, and stops both when the harness exits. Your own harness config is left untouched.

Leave out the harness to pick from the ones installed on your `PATH`, and leave out `-m` to pick the model. Arguments after `--` go to the harness itself.

## Where things live

- Settings: `~/.config/hedos.toml` (or `$XDG_CONFIG_HOME/hedos.toml`). Optional; every key has a default.
- State: `~/.local/share/hedos` (or `$XDG_DATA_HOME/hedos`): the registry, generated artifacts, job history, pull records, and the gateway's audit log.

Neither contains your model weights. hedos only points at where the weights already sit.

## Where to go next

- [shelf.md](shelf.md): the shelf screen, every key and pane.
- [cli.md](cli.md): every command, its flags, and its output.
- [models.md](models.md): discovery, installing, removing, and the runtimes.
- [gateway.md](gateway.md): the HTTP API and how to point tools at it.
- [configuration.md](configuration.md): the settings file, the data directory, and the environment variables.
- [architecture.md](architecture.md): how the crates fit together.
