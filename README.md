# hedos

> _ἕδος (hédos)_ — Greek for "a seat, an abode, a foundation."

[![Platform](https://img.shields.io/badge/platform-macOS%2026%20·%20Apple%20Silicon-orange)](#download)
[![Swift](https://img.shields.io/badge/Swift-6.1-orange)](Package.swift)
[![License](https://img.shields.io/badge/license-MIT-blue)](LICENSE)

hedos is a native macOS app — and a headless kernel beneath it — that discovers every local model already on your Mac and serves each one through the runtime that fits. It scans where models actually live (the Ollama store, the Hugging Face cache, LM Studio's library, loose GGUF and safetensors in your folders, and the model Apple ships in macOS) and puts them on one shelf, text, image, and speech together, tagged by where they came from. Weights are never moved, copied, or re-downloaded; records point at the files where they already sit.

For each model it detects the format, decides what it is and what it can do, and resolves it to a runtime: in-process **llama.cpp** and **MLX-Swift**, managed Python sidecars (**mlx-lm**, **mlx-vlm**, **diffusers**, **mflux**, **Kokoro** speech, **whisper**), **Ollama**, **Apple Foundation**, or any **OpenAI-compatible** endpoint. It reads each model's true context length, chat template, senses, and tool-calling dialect and serves _that_, so a conversation behaves the same across engines — and where a model genuinely can't do something, the shelf says so before you click instead of dropping it silently.

Everything runs on your hardware, offline, on Apple's MLX foundation — no browser wrapper, open source top to bottom.

## Download

Grab the latest `Hedos.dmg` from the [releases page](https://github.com/theiskaa/hedos/releases/latest). Requires an Apple Silicon Mac on macOS 26 (Tahoe) or later.

Or with Homebrew:

```sh
brew install --cask theiskaa/tap/hedos
```

Both install the app and its bundled `hedos` command-line tool; the app puts `hedos` on your `PATH` for you.

## Beyond chat

Chat is one capability of many. Any chat model streams replies with code, tables, and tool calls; diffusion models (diffusers, mflux) turn a prompt into an image inside the same conversation; Kokoro synthesizes speech and whisper transcribes it, so a thread can be spoken or dictated. A global hotkey summons a small ask panel over any app without leaving what you're doing.

A conversation can also be given a **place** — a folder that becomes its world. Within that canonicalized path prefix, and only with your per-action consent, the model lists, reads, searches, writes, and runs commands, every action rendered in the transcript as it happens: reads are quiet once you open the door, and anything that changes the world asks first, every time. Models compose, too — chained into a pipeline, three you already own (transcribe → chat → speak) become one new tool.

## The gateway and the CLI

The same shelf is served locally over an OpenAI- and Ollama-compatible HTTP gateway, bound to loopback and authenticated with scoped client tokens, so any editor, script, or agent on your Mac can reach the models you own, tools and all:

```sh
curl http://127.0.0.1:43367/v1/chat/completions \
  -H "Authorization: Bearer hd_xxx.yyy" \
  -d '{"model":"qwen2.5","messages":[{"role":"user","content":"hi"}]}'
```

The `hedos` command drives the same kernel headlessly — it links no UI, so it runs over SSH and in scripts:

```sh
hedos scan                        # discover every model on this Mac
hedos ls                          # list them with fit, tier, and runtime
hedos run gemma3 "explain this"   # stream a completion
hedos serve                       # start the gateway
```

## Build from source

hedos runs on **macOS 26 (Tahoe)** on Apple Silicon and builds with the **Xcode 26** toolchain (Swift 6.1). You also need `python3` on your `PATH` — the managed Python runtimes provision their own hash-locked environments with `uv` at first use.

```sh
make build   # compile the package
make test    # run the suite
make app     # assemble dist/Hedos.app
make run     # build the bundle and open it
```

Nothing here touches your models: weights are only read, never moved, copied, or re-downloaded.

## Contributing

See **[CONTRIBUTING.md](CONTRIBUTING.md)** for how to build, test, and open a pull request — logic lives in the kernel, the UI stays a thin shell. Deeper guides live in **[docs/](docs/README.md)**. Participation is governed by our [Code of Conduct](CODE_OF_CONDUCT.md); security issues have a private channel in [SECURITY.md](SECURITY.md).

hedos is MIT-licensed — see [LICENSE](LICENSE).
