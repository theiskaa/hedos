# Getting started

Hedos is a native macOS app that finds the models already on your Mac, installs new ones, and serves each through the runtime that fits — all on your own hardware, offline. This page covers installing it and what to expect in your first few minutes.

## Requirements

Hedos runs on an **Apple Silicon Mac** (M-series) on **macOS 26 (Tahoe) or later**. There is no Intel build, and nothing about it reaches off your machine to run a model.

## Installing

There are three ways in, and they land the same signed, notarized app. Pick whichever you prefer.

The install script downloads the latest release and puts Hedos in your Applications folder:

```sh
curl -fsSL https://hedos.ai/install | bash
```

With Homebrew, as a cask:

```sh
brew install --cask theiskaa/tap/hedos
```

Or download `Hedos.dmg` from the [releases page](https://github.com/theiskaa/hedos/releases/latest) and drag Hedos onto Applications yourself.

## The first run

Open Hedos and, if it doesn't find a model already installed, the home screen invites you to scan. Click **Scan this Mac** and Hedos looks where models actually live:

- the **Ollama** store
- the **Hugging Face** cache
- the **LM Studio** library
- loose **GGUF** and **safetensors** files in your folders
- the model **Apple ships in macOS**

Everything it finds lands on one shelf — text, image, and speech models together — each tagged with where it came from. For each one, Hedos reads the format, works out what it is and what it can do, and resolves it to a runtime, so a conversation behaves the same whichever engine is underneath.

Two things to know:

- **Weights are never moved, copied, or re-downloaded.** Hedos records point at the files where they already sit; every other tool on your Mac still sees them.
- If you keep models somewhere unusual, **Choose a folder…** lets you point Hedos at a specific directory. You can re-run the scan any time from the home screen.

If the scan comes up empty, the home screen shows a short list of models **recommended for your Mac**, fitted to its memory. That's the fast path into [installing new models](models.md).

## What you can do next

Once the shelf has something on it, the rest of Hedos opens up:

- **Chat, voice, and image** — talk to a text model, speak with a voice model, or generate images, all from the app.
- **[Install new models](models.md)** — resolve any `huggingface.co` or `ollama.com` link, an `org/repo`, or a `name:tag`; Hedos plans the exact file set, size, and destination before a byte moves.
- **[The command line](cli.md)** — the `hedos` command drives the same kernel headlessly, so it works over SSH and in scripts.
- **[The local gateway](gateway.md)** — serve your shelf to the rest of your machine over an OpenAI- and Ollama-compatible HTTP endpoint, so any editor, script, or agent can reach the models you own.

## Updating

Hedos checks GitHub Releases for a newer version about once a day. When one is available, the **version label** in the sidebar footer lights up and becomes the update button — click it to download, verify, and install the update in place; Hedos relaunches itself when it's done. You can also click that same label any time to check for updates on demand.

If you installed with Homebrew, update through the cask instead:

```sh
brew upgrade --cask theiskaa/tap/hedos
```

## Offline and open

Everything runs on your hardware, offline, on Apple's MLX foundation — no browser wrapper, nothing phoned home to run a model. Hedos is open source and MIT-licensed; the [README](../README.md) is the overview, and the rest of [docs/](README.md) holds the specifics.
