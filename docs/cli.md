# CLI reference

`hedos` is the command-line front end. It builds a kernel from your data directory and settings, runs one command, and exits. It links no UI, so it works over SSH and in scripts.

## Global options

Every command accepts `--json`, which emits machine-readable JSON on stdout instead of formatted text. Human notices (status lines, prompts, progress) always go to stderr, so `--json` output stays clean for piping.

## Resolving a model name

Commands that take a model accept an id, a name, an alias, or a unique substring. hedos tries an exact id first, then an exact case-insensitive name, then a unique substring match. If more than one model matches, it lists the candidates and asks you to be more specific. Commands that need a specific capability (chat, speak, image) only match models that serve it.

## Commands

### `hedos scan`

Discover models across the machine's stores, reconcile them into the registry, and resolve each to a runtime. Prints a one-line summary and any issues on stderr.

### `hedos ls`

List the shelf: a warm indicator, the name, the runtime, the store, and the capabilities. If the shelf is empty, it runs a scan first.

- `--scan` rescans before listing.
- `--capability <name>` shows only models serving that capability, for example `--capability embed`.

### `hedos run <model> <prompt>`

Stream a single completion to stdout.

- `--system <text>` sets a system prompt for the run.
- `--max-tokens <n>` caps the generated length.
- `--temperature <f>` sets the sampling temperature.

Under `--json`, streaming is suppressed and the full text plus the model id is printed as one object at the end.

### `hedos chat <model>`

An interactive session that reads turns from stdin and streams each reply. Press Ctrl-D to end. When stdin is a terminal it prints a prompt and a banner on stderr; when it is a pipe it just reads lines.

- `--system <text>` sets a system prompt for the conversation.
- `--max-tokens <n>` caps each reply.

### `hedos serve`

Start the OpenAI- and Ollama-compatible gateway on loopback and block until Ctrl-C. Prints the base URL. See the [gateway guide](gateway.md).

- `-p, --port <n>` overrides the port (the default comes from settings, else `43367`).

### `hedos pull [reference]`

Fetch a model from Ollama or Hugging Face, with a download progress bar. Ctrl-C cancels. When it finishes it runs a scan so the model appears on the shelf.

- The reference is a Hugging Face repo (`org/model`) or an Ollama tag (`gemma3:4b`). hedos infers the provider from the shape.
- `--from <ollama|hf>` forces the provider.
- Gated Hugging Face repositories need `HF_TOKEN` in the environment.

### `hedos rm <model>`

Remove an installed model. Without `-y` it prints a deletion preview (the item count and estimated size) and does nothing else.

- `-y, --yes` performs the deletion. File-backed models are deleted from disk; Ollama models delete through the daemon.

### `hedos speak <model> <text>`

Synthesize speech and write a WAV file. There is no playback.

- `--voice <name>` picks a voice. The default is the model's first bundled voice.
- `--speed <f>` sets the speed multiplier (default `1.0`).
- `-o, --output <path>` sets the output file. The default is a slugged model name with a `.wav` extension in the current directory.

### `hedos image <model> <prompt>`

Generate an image and write a PNG file. This runs as a job, with progress on stderr.

- `--steps <n>` sets the number of diffusion steps.
- `--seed <n>` sets the random seed.
- `-o, --output <path>` sets the output file. The default is a slugged model name with a `.png` extension in the current directory.

### `hedos warm <model>`

Load a model into residency with a tiny request, so the next real request starts warm. Reports whether the model is resident.

### `hedos unload <model>`

Evict a model from in-process residency and report the result.

## Exit codes

A command exits `0` on success. On failure it writes the error to stderr and exits non-zero.
