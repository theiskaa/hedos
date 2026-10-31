# Engines

An engine is the program a model actually runs on. hedos finds models and decides which runtime serves each one (see [models.md](models.md#how-a-runtime-is-resolved)); the runtime then starts its engine: `llama-server` for GGUF models, a Python environment that `uv` builds for the Python models, or the Ollama daemon for models in Ollama's store.

hedos needs none of them installed beforehand. It carries llama.cpp, installs llama.cpp and uv when you ask, and treats Ollama as optional. One rule holds throughout: **hedos never installs an engine on its own.** It installs one when you run `hedos engines install`, or when you answer yes to a question it asks first (after a pull, or on the shelf). The gateway never installs anything.

## At a glance

| Engine | Runs | Comes with hedos | Otherwise |
| --- | --- | --- | --- |
| llama.cpp (`llama-server`) | GGUF models: chat, code, embedders, decision models, and the Ollama models it can load | Yes, in release builds (install script, Homebrew, release archives) | `hedos engines install llama.cpp`, or one on your `PATH` |
| uv | The Python runtimes: MLX, diffusers, mflux, audio, speech, embeddings, and manifest runtimes with a Python environment | No | `hedos engines install uv`, or one you installed |
| Ollama | The models in Ollama's store | No | Optional. Install it from [ollama.com](https://ollama.com) |

`hedos engines` shows where each one was found on this machine, its version, and what to do when one is missing:

```
ENGINE     SERVES                    FOUND    VERSION              PATH OR NEXT STEP
llama.cpp  GGUF models               bundled  0.7.0 (build 11669)  /Users/you/.local/share/hedos/engines/llama.cpp/b11669-metal-bundled/llama-server
Ollama     models in Ollama's store  path     -                    /usr/local/bin/ollama
uv         Python models             hedos    0.13.4               /Users/you/.local/share/hedos/engines/uv/0.13.4/uv
```

`FOUND` reads `config` (the settings name it), `hedos` (a build hedos installed), `bundled` (the build hedos carries), `path` (your `PATH` or a standard install location), `daemon` (an Ollama that only answers on its port), or `missing`. The full command reference is in [cli.md](cli.md#hedos-engines).

## llama.cpp

### The build hedos carries

Release builds of hedos carry llama.cpp 0.7.0 (build 11669) inside the binary: the Metal build on Apple Silicon, and the CPU build on Intel Macs and Linux. Nothing is unpacked at install time. The first time a model needs it, hedos checks the embedded archive against its digest and unpacks it into the data directory (`engines/llama.cpp/b11669-metal-bundled/`, with llama.cpp's `LICENSE` beside it). Every model after that uses the unpacked copy. Until then, `hedos engines` says it "unpacks the first time a model runs".

The Linux build is hedos's own, built from llama.cpp's pinned source so that it starts on glibc 2.35 (Ubuntu 22.04, Debian 12) with nothing else installed. llama.cpp's own Linux builds need glibc 2.38.

A `cargo install` or source build carries none. It runs a `llama-server` from your `PATH`, or the one `hedos engines install llama.cpp` puts in place. A pull of a GGUF model offers that install once the model lands.

### Which `llama-server` runs

hedos runs the first one it finds of:

1. The absolute path `[engines] llama_server` names in [`hedos.toml`](configuration.md#engines).
2. A build `hedos engines install llama.cpp` installed.
3. The build hedos carries.
4. A `llama-server` on your `PATH`.
5. `/opt/homebrew/bin/llama-server`, then `/usr/local/bin/llama-server`.

Every build of hedos's own comes before your `PATH`, so an older llama.cpp installed there (from Homebrew, say) is never run by default. `hedos engines` names it as passed over. To run it anyway, name it in the settings. The order is read again each time a model server starts, so a build installed while `hedos serve` is running is the one the next model starts on.

### Installing another build

The carried build is portable rather than fast. `hedos engines install llama.cpp` fetches the build that suits the machine:

| Machine | Build (`--variant`) |
| --- | --- |
| Apple Silicon | `metal` |
| Intel Mac | `cpu` |
| NVIDIA card | `cuda-12` or `cuda-13` |
| AMD card | `vulkan` |
| Anything else | `cpu` |

`rocm` is used only when you ask for it, since it needs ROCm installed separately. When a faster build than the carried one suits the machine, `hedos engines` and `hedos recommend` say so in one line.

Before anything is fetched, hedos shows a card:
- the release and build;
- each archive's URL and SHA-256 (a CUDA build brings NVIDIA's runtime libraries as a second archive);
- the size;
- where it goes;
- for CUDA, the smaller Vulkan build as an alternative.

Then it asks, defaulting to no. Outside a terminal, or under `--json`, it needs `--yes`.

The install then:
1. Downloads each archive and checks it against its pinned size and SHA-256.
2. Unpacks it into a staging directory, refusing any entry that would land outside it.
3. Runs `llama-server --version` and requires the pinned build.
4. Records the build in `installed.json` and moves it into place, putting the previous build back if the move fails.

Only one install runs at a time. Ctrl-C stops it and leaves nothing half-installed. The next install clears what was left in staging.

### Linux and glibc

llama.cpp's own GPU builds (CUDA, Vulkan, ROCm) need glibc 2.38 (Ubuntu 24.04, Debian 13, Fedora 39). On an older system:
- the build offered is hedos's CPU build, from hedos's own GitHub release;
- asking for a GPU build with `--variant` is refused with the reason, before anything is fetched.

A build that will not start for want of a library is reported with the library's name and the package that has it.

### Removing

`hedos engines remove llama.cpp` removes the builds hedos installed or unpacked, and says which `llama-server` runs now. The carried build cannot be removed from the binary. It unpacks again the next time a model needs it.

## uv and the Python runtimes

The Python runtimes ship their code inside the hedos binary, with a lockfile that pins every package by hash. The first time one runs, `uv` builds its environment from that lock. Every later run reuses the environment.

### Which uv runs

hedos uses the first `uv` it finds of:

1. The one `hedos engines install uv` installed. That is the pinned version if present, else the one installed last.
2. `~/.local/bin/uv`, where uv's own installer puts it.
3. `/opt/homebrew/bin/uv`, then `/usr/local/bin/uv`.
4. Your `PATH`.

A uv counts only once it answers `--version`.

### Installing uv

`hedos engines install uv` fetches uv 0.13.4, the version hedos pins. On Linux that is uv's static build, which runs on any glibc. The install shows the archive's URL, SHA-256, size and destination. It also says that the first Python model then fetches Python 3.12 (when the machine has none) and that model's packages. Then it asks, defaulting to no. The archive is checked against its pinned size and SHA-256, and the unpacked `uv` must answer with its version before it is put in place. `hedos engines remove uv` removes only the one hedos installed, never yours.

A uv you installed yourself from [astral.sh/uv](https://astral.sh/uv) works too. To install PyTorch's CPU build on Linux (see below) it must be 0.6.9 or newer.

### How an environment is built

Each runtime's environment lives in the data directory: `env/<runtime>/` for the built-in ones, `env/manifests/` for manifest runtimes. hedos first tries to build it from uv's cache alone. Only when that fails does it download, and a download may take up to two hours, enough for PyTorch's CUDA build. While it builds, the model reports `Installing packages…`, then `Downloading packages…`.

- **Python.** When the machine has no Python 3.12, uv downloads one into `env/python/` under hedos's data, not into your home.
- **Hashes.** Every package is checked against a hash in the lock (`--require-hashes`), whatever your uv settings say.
- **Your settings.** uv runs with only the environment hedos gives it, so variables like `UV_INDEX_URL` in your shell do not reach it. A mirror or index set in `uv.toml` does (`~/.config/uv/uv.toml`, or under `XDG_CONFIG_HOME`).
- **After a build.** A model whose environment is built keeps running with uv removed. Only a model that still needs building needs uv.

### PyTorch: CUDA or CPU

The runtimes that use PyTorch (audio, diffusers images, laya, zerank and YuE2) ship two locks:
- `requirements.lock` takes PyTorch from PyPI. That is the CUDA build with NVIDIA's libraries on Linux, and the Metal build on a Mac.
- `requirements.cpu.lock` takes PyTorch's CPU-only build, which is much smaller.

On Linux, hedos installs the CPU lock when NVIDIA's driver is not loaded. It reads that from files rather than running a program: `/proc/driver/nvidia/version`, or WSL's `/usr/lib/wsl/lib/libcuda.so.1`. Everywhere else it installs the PyPI lock. The first CUDA install fetches about 3.5 GB.

At run time, a runtime uses Metal on a Mac. On Linux it uses CUDA only when the card is one its PyTorch build has kernels for; otherwise it runs on the CPU, in full precision. A manifest runtime can ship its own CPU variant the same way; see [configuration.md](configuration.md#manifest-runtimes-and-consent).

## Ollama

Ollama is optional, and hedos never installs it. hedos counts it present when it finds the `ollama` binary or the daemon answers:
- it looks for the binary in `/usr/local/bin`, `/opt/homebrew/bin` and `~/.local/bin`, then your `PATH`;
- it checks for a daemon on `127.0.0.1:11434`.

The models in Ollama's store are on the shelf either way:
- **With Ollama**, it serves them, and hedos starts the daemon if Ollama is installed but not running.
- **Without Ollama**, llama.cpp serves the ones it can load. Ollama keeps most models as plain GGUF files, so `llama-server` runs them directly, with their projector when they read images.

Two kinds stay with Ollama: a model whose vision or audio encoder is packed into its weights, and one whose only chat template is Ollama's own. `hedos ls` and the shelf say those need Ollama.

Installing or removing Ollama moves the models in its store between the two the next time hedos starts. No rescan is needed. `hedos rm` deletes an Ollama model through the daemon when Ollama is there. Without it, hedos deletes the files that model alone uses and keeps any blob another tag shares.

## Where hedos offers an install

| Where | When | What it asks |
| --- | --- | --- |
| `hedos engines install <llama.cpp\|uv>` | When you run it | The install card, defaulting to no; `--yes` skips the question |
| After `hedos pull` | A pull you follow to its end, in a terminal, lands a model whose engine is missing | The same card, once, defaulting to no. Under `--json` or outside a terminal it prints the command instead. The pull stays successful whatever the answer |
| The shelf | `w`, `t` and the other actions on a model whose engine is missing | The same card. `y` runs the install as a background task, `n` or `esc` installs nothing |

With a build of hedos that carries llama.cpp, llama.cpp is never missing, so only uv is offered.

## Where engines live

Everything hedos installs or unpacks is under the data directory (`~/.local/share/hedos`, or `$XDG_DATA_HOME/hedos`):

| Path | Holds |
| --- | --- |
| `engines/llama.cpp/` | The carried build once unpacked (`b11669-metal-bundled/`), and a build `hedos engines install` installed (`b11669-cuda-12/`), each with an `installed.json` |
| `engines/uv/` | Each uv hedos installed (`0.13.4/`, with `uv`, `uvx` and an `installed.json`) |
| `env/` | The Python environments, one directory per runtime, each with its package cache |
| `env/python/` | The Python uv downloaded, when the machine had no 3.12 |

None of it holds model weights. Deleting `engines/` costs a re-unpack or a re-install, and deleting `env/` a rebuild on next use.

## Other backends

Some runtimes reach something hedos neither carries nor installs:
- **OpenAI-compatible endpoints** need a reachable server, and its key in `HEDOS_OPENAI_API_KEY` when it needs one.
- **ComfyUI and AUTOMATIC1111** need their daemon running.
- **Apple Intelligence** needs a Mac where Apple's model is enabled, plus the bridge library beside the binary (see the [README](../README.md#runtimes)).
- **whisper.cpp** transcription needs its own engine, which is not bundled yet.

[models.md](models.md#runtimes-at-a-glance) lists every runtime and what it needs.

## Troubleshooting

**A GGUF model answers 503 with "llama.cpp isn't installed".** No `llama-server` is there to start for it. That happens only with a build of hedos that carries none (`cargo install`, a source build). Run `hedos engines install llama.cpp`, or put one on your `PATH`.

**A decision model fails with a message about llama.cpp 0.6.0.** The `llama-server` hedos runs is older than 0.6.0. `hedos engines` shows which one. Usually it is one `[engines] llama_server` names, or an older build installed with `hedos engines install`. Reinstall with that command, or upgrade the one the settings name.

**Your own `llama-server` is not used.** hedos's builds come first. Name yours in `[engines] llama_server`.

**`hedos engines install llama.cpp --variant cuda-12` is refused on Linux.** llama.cpp's GPU builds need glibc 2.38, and this system has an older one. The CPU build hedos carries or offers runs here.

**A Python model says "uv isn't installed".** Its environment is not built and no uv was found. Run `hedos engines install uv`.

**A Python model says its uv "cannot install PyTorch's CPU build".** The uv found is older than 0.6.9. Run `hedos engines install uv`; hedos's own uv comes ahead of yours.

**A model says only Ollama runs it.** It is kept in Ollama's own format. Install Ollama, or pull a Hugging Face GGUF version of the model.
