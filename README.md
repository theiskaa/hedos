# Hedos

> _ἕδος (hédos)_ — Ancient Greek for "a seat, an abode, a foundation" — the place where something comes to rest and is established.

[![Platform](https://img.shields.io/badge/platform-macOS%2026%20·%20Apple%20Silicon-orange)](#building)
[![Swift](https://img.shields.io/badge/Swift-6.1-orange)](Package.swift)
[![License](https://img.shields.io/badge/license-MIT-blue)](LICENSE)

Hedos gives every local model on your Mac a single home. Open it and it has already walked your machine — the Ollama store, the Hugging Face cache, LM Studio's library, loose GGUFs in your folders, and the model Apple builds into macOS — and put everything it found on one shelf: text, image, and speech side by side, whatever tool installed them. The first thing you see is everything you already have:

```text
14 models · 87 GB
   6   Ollama
   5   Hugging Face cache
   3   GGUF in ~/Downloads
```

It never moves, copies, or re-downloads weights; records point at the files where they already sit. For each model, Hedos detects the format, decides what it is and what it can do, and auctions it to the runtime that can serve it — in-process llama.cpp and MLX-Swift, managed Python sidecars (mlx-lm, mlx-vlm, diffusers, mflux, Kokoro speech, whisper), Ollama, Apple Foundation, or an OpenAI-compatible endpoint. It reads each model's true context length, chat template, senses, and tool-calling dialect and serves _that_, so the same conversation behaves the same across runtimes — and where a model genuinely can't do something, the shelf says so before you click instead of silently dropping it.

A conversation can also be given a **place** — a folder on your disk that becomes its world. Inside that boundary, and only with your per-action consent, the model can read files, write them, and run commands, with every action shown in the transcript as it happens:

```text
place · ~/projects/notes

   read    spec.md          ✓ done
   read    TODO.md          ✓ done
   write   summary.md       ? asks first
   run     git status       ? asks first
```

The boundary is a real canonicalized path prefix, not a suggestion: reading is quiet once you've opened the door, and anything that changes the world asks first, every time, unless you've said otherwise.

The same shelf is served locally over OpenAI- and Ollama-compatible HTTP, so any editor, script, or agent on your Mac can reach the models you already own, tools and all:

```sh
curl http://127.0.0.1:43367/v1/chat/completions \
  -H "Authorization: Bearer hd_xxx.yyy" \
  -d '{"model": "qwen2.5", "messages": [{"role": "user", "content": "hi"}]}'
```

Everything runs on your hardware, offline, private by design — a native macOS app built for Apple Silicon on Apple's MLX foundation, no browser wrapper. Open source, top to bottom.

> Hedos is in early development. The app bundle is built from source for local use; there is no signed release yet.

## Building

Hedos runs on macOS 26 (Tahoe) on Apple Silicon and builds with the macOS 26 SDK toolchain on Swift 6.1. You also need `python3` on your `PATH` — the managed Python runtimes provision their own hash-locked environments with `uv` at first use, and the test suite drives stdlib-only fake sidecars that need no network.

`make build` compiles the package and `make test` runs the suite; both must pass before any commit. `make app` assembles `dist/Hedos.app` for local use, and `make run` builds that bundle and opens it. Nothing in this process touches your models: weights are never moved, copied, or re-downloaded — Hedos only reads what is already on disk.

## Documentation

Guides live in **[docs/](docs/)**. The theme system — the two appearance axes, the TOML palette schema, and how to reskin a built-in theme or add a new one — is covered in **[docs/themes.md](docs/themes.md)**, and the local endpoint — enabling the loopback server, client tokens and scopes, and the OpenAI- and Ollama-compatible routes it serves — in **[docs/gateway.md](docs/gateway.md)**. Start from the **[index](docs/README.md)**; more guides are added as the surfaces they describe settle.

## Contributing

Contributions are welcome. See **[CONTRIBUTING.md](CONTRIBUTING.md)** for how to build, test, and open a pull request, and the conventions the codebase holds (chiefly: logic lives in the kernel, UI stays a thin shell). Participation is governed by our [Code of Conduct](CODE_OF_CONDUCT.md), and security issues have their own private channel in [SECURITY.md](SECURITY.md).

Hedos is MIT-licensed; see [LICENSE](LICENSE).
