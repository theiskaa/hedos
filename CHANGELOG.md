# Changelog

## v0.1.1 - 2026-07-14

- In-app updater backed by GitHub Releases: the app checks for a newer version, downloads the DMG, verifies its checksum, and swaps itself in place. The sidebar version label doubles as the update button.
- Image attachments in chat: vision-capable models get a paperclip in the composer, and attached images travel with the conversation as content-addressed references.
- Restructured chat composer controls; the input keeps focus across mode switches and sends.
- Motion polish across the app: ad-hoc animation timings unified on the shared design tokens, modals and palettes dismiss faster than they open, and list stagger tightened so the last item never lands late.
- Image and voice model selection no longer drifts to the wrong record, and composer focus survives model changes.
- The Homebrew cask declares the macOS 26 (Tahoe) requirement with the modern `depends_on macos` syntax.
- The DMG background palette matches the website.

## v0.1.0 - 2026-07-14

The first release.

- Discovery of every local model already on the Mac: Ollama, the Hugging Face cache, LM Studio, loose GGUF and safetensors files, and the model Apple ships with macOS. Weights are never moved or re-downloaded; records point at where other tools put them.
- Chat, voice conversation, and image generation over the runtime that fits each model, with per-model parameter controls.
- A local gateway speaking the OpenAI and Ollama wire formats, so existing clients can talk to any discovered model.
- The `hedos` command-line tool, installed on the `PATH` by the app.
- Signed and notarized DMG for Apple Silicon, plus a Homebrew cask (`brew install --cask theiskaa/tap/hedos`).
