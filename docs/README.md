# Hedos documentation

Guides for using and customizing Hedos. The [README](../README.md) is the overview; these pages hold the specifics.

- **[getting-started.md](getting-started.md)** — installing Hedos, the first-run scan, and what to do next. Start here.
- **[models.md](models.md)** — installing and managing models: the install browser, pasting a Hugging Face or Ollama reference, where downloads land, gated repos and tokens, and deleting a model.
- **[runtimes.md](runtimes.md)** — how your models run: the engine that fits each model, the managed Python sidecars, and what to expect on first use.
- **[configuring-models.md](configuring-models.md)** — per-model customization: parameters, the system prompt, context length, renaming, and the honest boundary where a knob applies on one runtime and is refused on another.
- **[orchestra.md](orchestra.md)** — running a conversation with several models: the main model and its seats (Images, Voice, Eyes), how specialists are offered as tools, borrowed eyes for text-only mains, and per-chat versus default arrangements.
- **[cli.md](cli.md)** — the `hedos` command-line tool: every command and flag, `--json` output, and running headless over SSH.
- **[gateway.md](gateway.md)** — the local endpoint: enabling the loopback server, client tokens and scopes, the OpenAI- and Ollama-compatible routes, and what it does and doesn't serve.
- **[themes.md](themes.md)** — the theme system: the two appearance axes, the TOML palette schema, overriding a built-in theme without a rebuild, and adding a new one.

More guides are added as the surfaces they describe settle.
