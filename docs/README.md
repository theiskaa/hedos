# hedos documentation

These guides go deeper than the top-level [README](../README.md). Start with getting started, then read whichever piece you need.

- **[Getting started](getting-started.md)** builds hedos, discovers your models, and runs your first completion.
- **[CLI reference](cli.md)** documents every `hedos` command, its flags, and its output.
- **[Gateway](gateway.md)** covers the local HTTP server: the OpenAI and Ollama endpoints, and how to point tools at it.
- **[Models](models.md)** explains discovery, installing, and removing models, and how weights are handled.
- **[Configuration](configuration.md)** lists the settings file, the data directory, and the environment variables hedos reads.
- **[Architecture](architecture.md)** describes the four crates, the kernel-first design, and how a request flows from the CLI down to a runtime.

If something here is wrong or unclear, a pull request or an issue is welcome.
