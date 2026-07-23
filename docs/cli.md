# CLI reference

`hedos` is the command-line front end. It builds a kernel from your data directory and settings, runs one command, and exits. It links no UI, so it works over SSH and in scripts.

## Global options

Every command accepts `--json`, which emits machine-readable JSON on stdout instead of formatted text. Human notices (status lines, prompts, progress) always go to stderr, so `--json` output stays clean for piping.

## Resolving a model name

Commands that take a model accept an id, a name, an alias, or a unique substring. hedos tries an exact id first, then an exact case-insensitive name, then a unique substring match. If more than one model matches, it lists the candidates and asks you to be more specific. Commands that need a specific capability (chat, speak, image) only match models that serve it.

## Picking a model interactively

Every command that takes a model can be run without one. In a terminal, hedos opens a fuzzy-filterable picker of the eligible models, with the same columns as `ls`: type to narrow the list, use the arrow keys to move, Enter to choose, and Esc to cancel. The list is scoped to what the command needs, so `speak` only offers speech models and `unload` only offers models that are currently warm.

The same holds for a missing prompt or a missing text argument: in a terminal hedos asks for it inline. Outside a terminal (a pipe, a script, or `--json`), a missing argument is a plain error instead of a prompt, so nothing ever blocks waiting on input.

## Commands

### `hedos scan`

Discover models across the machine's stores, reconcile them into the registry, and resolve each to a runtime. Prints a one-line summary and any issues on stderr.

### `hedos ls`

List the shelf: a warm indicator, the name, the runtime, the store, a memory-fit verdict, and the capabilities. If the shelf is empty, it runs a scan first.

- `--scan` rescans before listing.
- `--capability <name>` shows only models serving that capability, for example `--capability embed`.

The FIT column reads `fits`, `tight`, `too big`, or `—` (footprint unknown), judged from the model's estimated footprint against this machine's memory — the same assessment the install recommendations use. `--json` carries it as a `fit` field on each record.

### `hedos run [model] [prompt]`

Stream a single completion to stdout. Omit the model to pick one interactively, and omit the prompt to type it at a prompt.

- `--system <text>` sets a system prompt for the run.
- `--max-tokens <n>` caps the generated length.
- `--temperature <f>` sets the sampling temperature.
- `--image <path>` attaches a local image for a vision (`see`) model to read; repeat it for several images. With `--image`, the picker and name resolution scope to vision-capable models, and a model that cannot see is refused up front rather than answering blind.

Under `--json`, streaming is suppressed and the full text plus the model id is printed as one object at the end.

### `hedos chat [model]`

An interactive session that reads turns from stdin and streams each reply. Press Ctrl-D to end. When stdin is a terminal it prints a prompt and a banner on stderr; when it is a pipe it just reads lines.

- `--system <text>` sets a system prompt for the conversation.
- `--max-tokens <n>` caps each reply.

### `hedos serve`

Start the OpenAI-, Ollama-, and Anthropic-compatible gateway on loopback and block until Ctrl-C. Prints the base URL. See the [gateway guide](gateway.md).

- `-p, --port <n>` overrides the port (the default comes from settings, else `43367`).

### `hedos launch [harness]`

Run a coding harness against a gateway served for exactly as long as that harness runs. There is nothing to start first: the gateway binds a free port inside the same process, the harness is spawned pointed at it, and it stops when the harness exits.

```sh
hedos launch                  # pick a harness, then a model
hedos launch opencode         # pick a model
hedos launch claude -m qwen3
```

Supported harnesses, and the dialect each needs:

| Harness | Binary | Dialect |
| --- | --- | --- |
| Claude Code | `claude` | Anthropic |
| OpenCode | `opencode` | OpenAI |
| Aider | `aider` | OpenAI |
| Goose | `goose` | OpenAI |
| Crush | `crush` | OpenAI |

- `-m, --model <name>` picks the model; omit it to choose interactively.
- Anything after `--` is passed straight through to the harness.
- Omitting the harness in a terminal lists only the ones actually installed.

Before the harness starts, hedos runs one throwaway request through the model, shaped like the ones the harness will send. A model whose backend is down (a stopped Ollama daemon, a missing `llama-server`, an out-of-memory GPU) fails here with the reason and what to do about it, rather than inside the harness where it reads as an unexplained error. It also leaves the model loaded, so the first real request is warm.

Every harness here except Aider drives the model entirely through tool calls, so it needs a model that supports them. Tool support shows as a `tools` capability in `hedos ls` and the picker, read from the model's chat template during discovery, and the launch picker offers only tool-capable models to the harnesses that need them. This includes models served by the MLX sidecars: the offered tools are rendered through the model's own chat template and the calls are parsed back out of its reply, so an MLX build of Llama or Qwen seats a harness the same way an Ollama model does. Apple Intelligence seats them too: the bridge offers the tools to Apple's model and captures the calls it makes back out. A model whose tool support couldn't be read from disk is assumed capable and left in the list; the pre-flight then probes with a tool and catches it before the harness starts, with a note to pick another model or use Aider (whose edits are plain text and need no tools).

Your own harness config is never read around or written to. Harnesses that can be configured through the environment are; the rest get a generated config under the hedos data directory, so running the harness directly afterwards behaves exactly as it did before.

The whole chat-capable shelf is offered, not just the model you named, so you can switch models inside the harness. `-m` only chooses the one it opens on.

Ctrl-C goes to the harness, not to hedos, so it handles the interrupt the way it normally would. The harness's exit code becomes the exit code of `hedos launch`.

Codex is not supported: it speaks the OpenAI Responses API, which this gateway does not serve, and it removed the setting that made it speak chat completions.

### `hedos pull [reference]`

Fetch a model from Ollama or Hugging Face, with a download progress bar. Ctrl-C cancels. When it finishes it runs a scan so the model appears on the shelf.

- The reference is a Hugging Face repo (`org/model`) or an Ollama tag (`gemma3:4b`). hedos infers the provider from the shape.
- Omit the reference in a terminal to search: type a query to search Hugging Face (results show download and like counts), or leave it blank for a short list of models that fit this machine's RAM. A "search again" entry in the list returns to the prompt, so you can move between recommendations and a search — or try another query — without restarting the command.
- Before any bytes move, hedos shows the plan (the name, the destination, and the size) and asks you to confirm.
- `--from <ollama|hf>` forces the provider.
- Gated Hugging Face repositories need a token with access to the repo — `HF_TOKEN`, `HF_TOKEN_PATH`, or `huggingface-cli login` — and you must accept the model's terms on its Hugging Face page first.

### `hedos rm [model]`

Remove an installed model. It always shows a deletion preview first: the item count and the estimated size.

- In a terminal, it then asks for a yes/no confirmation and deletes only if you agree (the default is no).
- Outside a terminal, it does nothing unless `-y` is given, so a script can never delete without asking.
- `-y, --yes` skips the confirmation. File-backed models are deleted from disk; Ollama models delete through the daemon.

### `hedos speak [model] [text]`

Synthesize speech and write a WAV file. There is no playback. Omit the model or the text to be prompted for them.

- `--voice <name>` picks a voice. When a model has several voices and none is given, hedos offers a picker in a terminal, otherwise it uses the first bundled voice.
- `--speed <f>` sets the speed multiplier (default `1.0`).
- `-o, --output <path>` sets the output file. The default is a name slugged from the text with a `.wav` extension in the current directory.

### `hedos transcribe [model] [audio]`

Transcribe an audio file to text through a local whisper model — the inverse of `speak`. Omit the model or the audio path to be prompted for them. The transcript streams to stdout as it is produced.

- `--language <code>` forces the source language (for example `en`); the default auto-detects.
- `--translate` translates to English instead of transcribing verbatim.

The audio is a WAV file, and the path may start with `~`. Under `--json`, the model, the path, and the full transcript are printed as one object.

### `hedos image [model] [prompt]`

Generate an image and write a PNG file. This runs as a job, with progress on stderr. Omit the model or the prompt to be prompted for them.

- `--steps <n>` sets the number of diffusion steps.
- `--seed <n>` sets the random seed.
- `-o, --output <path>` sets the output file. The default is a name slugged from the prompt with a `.png` extension in the current directory.

### `hedos warm [model]`

Load a model into residency with a tiny request, so the next real request starts warm. Reports whether the model is resident.

### `hedos unload [model]`

Evict a model from in-process residency and report the result. Omit the model to pick from the models that are currently warm.

### `hedos stats`

Read the gateway's audit log back and report usage: the total request count, the rejection rate, and per model the request count, the error rate, and p50/p90/p99 serving latency. Prints a table, or the full summary under `--json`. With no audit log yet (nothing has been served), it says so and exits `0`.

## Exit codes

A command exits `0` on success. On failure it writes the error to stderr and exits non-zero.
