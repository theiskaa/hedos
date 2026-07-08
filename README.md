# Hedos

> _ἕδος (hédos)_ — Ancient Greek for "a seat, an abode, a foundation" — the place where something comes to rest and is established.

Hedos gives every local model on your Mac a single home. Open it and it already knows what you have — the models pulled through Ollama, sitting in the Hugging Face cache, or downloaded by hand into a folder — and puts them on one shelf: text, image, and speech side by side, whatever tool installed them. Hedos figures out what each model is and what can run it, then runs it through one native interface, built for Apple Silicon.

Everything is local-first and private by design: models stay exactly where their tools put them (Hedos never moves or re-downloads weights), inference runs on your hardware, and nothing phones home. Open source, top to bottom.

Hedos is in early development, pre-v0.1 — there is nothing to install yet.

## Development

Building Hedos requires the macOS 26 SDK toolchain: `swift build` and `swift test`.
`swift test` needs `python3` on `PATH` — tests drive stdlib-only fake sidecars, no `uv`, no network.
`make app` assembles `dist/Hedos.app` for local use.

## License

MIT — see [LICENSE](LICENSE).
