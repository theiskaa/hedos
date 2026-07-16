# The command-line tool

Hedos ships a first-class `hedos` command that drives the same kernel as the app, headlessly. It links no UI, so it runs over SSH, in a `cron` job, or anywhere a shell reaches — discovering models, streaming completions, generating speech and images, and standing up the gateway, all without a window ever opening.

The command is carried inside the app bundle, so **the app must be installed** for `hedos` to exist. On first launch Hedos offers to add it to your `PATH`; you can also install it any time from the Hedos menu. Under the hood it symlinks the bundled binary to `/usr/local/bin/hedos`, so after installing, open a new terminal and run `hedos --help`.

Everything the command touches is the same shelf, registry, and store the app uses. There is no separate state — `hedos pull` in the terminal and a pull in the app land in the same place.

## The shape of a command

Run `hedos` with no arguments (or `hedos --help`) to see the subcommands. Each one also takes `--help`:

```sh
hedos --help
hedos ls --help
```

The full set of subcommands:

- `scan` — discover models on this machine and refresh the shelf
- `ls` — list the shelf with fit, tier, warm state, and store
- `run` — stream a single completion
- `chat` — hold a streaming chat, reading turns from stdin
- `speak` — synthesize speech to a `.wav`
- `image` — generate an image to a `.png`
- `pull` — fetch a model from Ollama or Hugging Face
- `rm` — delete an installed model
- `warm` — load a model into residency
- `unload` — evict a model from residency
- `serve` — start the loopback gateway
- `token` — manage gateway client tokens

Every command accepts a `--json` flag. Without it you get human-readable text on stdout and incidental status on stderr; with it you get a single machine-readable JSON document on stdout, described under [Scripting with `--json`](#scripting-with---json).

## Naming a model

Commands that act on a model take it as the first positional argument. You can pass a model's id, its name, or its display name — resolution runs in that order:

1. An exact **id** match wins immediately.
2. Otherwise, an exact **name** or **display name** match (case-insensitive). If more than one model matches exactly, the command stops and lists the candidates so you can pick one by id.
3. Otherwise, a **substring** match against name, display name, or id. Exactly one match is used; several again produce the candidate list.

So `hedos run gemma3 "..."` finds a model whose name contains `gemma3`, and an ambiguous query fails loudly rather than guessing:

```sh
$ hedos run llama "hi"
"llama" matches 3 models — pick one by id:
  <id-a>  ·  Llama 3.1 8B
  <id-b>  ·  Llama 3.2 3B
  <id-c>  ·  CodeLlama 7B
```

Commands are also scoped to a capability where it makes sense — `run` and `chat` resolve only among chat models, `speak` only among speech models, `image` only among image models — so the same short query can land on different models depending on the verb. If nothing matches, the command tells you to check `hedos ls`.

This is close to, but not identical to, the gateway's model resolution: the gateway additionally matches aliases and normalized tags (see [the gateway guide](gateway.md)). The CLI resolves by id, name, and substring only.

## Discovering and listing

`hedos scan` walks the machine — the Ollama store, the Hugging Face cache, LM Studio's library, loose GGUF and safetensors, and the model Apple ships — and refreshes the shelf. It prints a one-line headline; any problems it hit go to stderr as `issue:` lines.

```sh
hedos scan
```

`hedos ls` prints the shelf. Each row shows a warm dot (`●` resident, `○` cold), the name, the fit verdict for this Mac (`runs well`, `tight`, `too large`), the runtime tier, the resolved runtime, the store the model came from, and its capabilities.

```sh
hedos ls
```

Two options refine it:

- `--scan` rescans the machine before listing, so the shelf is fresh.
- `--capability <cap>` keeps only models serving that capability. Valid values are `chat`, `complete`, `embed`, `see`, `image`, `speak`, and `transcribe`.

```sh
hedos ls --scan --capability image
```

If the shelf is empty, `ls` scans once automatically before giving up.

## Running a completion

`hedos run` streams a single completion. It takes the model and the prompt as positional arguments, and the generated text lands on stdout as it arrives:

```sh
hedos run qwen2.5 "explain quorum in one paragraph"
```

Options:

- `--system <text>` overrides the system prompt.
- `--max-tokens <n>` caps the reply length.
- `--temperature <x>` sets the sampling temperature.

```sh
hedos run qwen2.5 "write a haiku about disks" \
  --system "You are terse." \
  --max-tokens 64 \
  --temperature 0.4
```

Status messages (model loading, and so on) go to stderr, so piping stdout gives you just the completion.

## Holding a chat

`hedos chat` keeps a running conversation, reading one user turn per line from stdin and holding the history in memory. Each reply streams to stdout; press Ctrl-D to end.

```sh
hedos chat llama3
```

Because it reads stdin, it works both interactively and piped:

```sh
printf 'summarize this log\n%s\n' "$(cat run.log)" | hedos chat llama3
```

It takes `--system` and `--max-tokens`, the same as `run`. With `--json`, each assistant turn is emitted as its own JSON object.

## Speech and images

`hedos speak` synthesizes text to a `.wav`. It takes the model and the text, and prints the path of the file it wrote:

```sh
hedos speak kokoro "the shelf is warm" -o hello.wav
```

Options:

- `--voice <name>` picks a voice; without it the model's first voice is used.
- `--speed <x>` sets a speaking-speed multiplier (default `1.0`).
- `-o, --output <path>` writes the `.wav` to a path you choose; without it, the file stays in the artifact store and its path is printed.

`hedos image` generates an image to a `.png`, taking the model and a prompt and printing the resulting path. It shows step progress on stderr while it denoises:

```sh
hedos image sdxl "a koala on a circuit board" --steps 30 --seed 7 -o koala.png
```

Options are `--steps <n>` (denoising steps), `--seed <n>` (for reproducible output), and `-o, --output <path>`.

For both, `-o` must be a file path, not a directory, and an existing file at that path is replaced.

## Installing and deleting

`hedos pull` fetches a model onto the shelf. Weights land in each platform's native habitat — Ollama through the daemon, Hugging Face into the standard hub cache with resume — so every other tool on your Mac sees the result too.

```sh
hedos pull gemma3:4b
hedos pull Qwen/Qwen2.5-3B-Instruct
```

The source is inferred from the reference: an `org/repo` goes to Hugging Face and everything else to Ollama (use `user/model:latest` for Ollama community models). Force it with `--from`:

```sh
hedos pull some/name --from ollama
```

`--from` accepts `ollama` or `huggingface` (`hf` is an alias). A gated Hugging Face repo needs a token — set `HF_TOKEN` or run `huggingface-cli login`, then retry. Download progress renders on stderr; press Ctrl-C to cancel, and substantial progress stays on disk so re-running the same pull resumes it.

Run `hedos pull` with **no reference** to get a size-aware list of models recommended for this Mac:

```sh
hedos pull
```

`hedos rm` deletes an installed model — files to the Trash, the entry off the shelf. It is a dry run by default: without `--yes` it prints exactly what would be removed and then exits, deleting nothing.

```sh
hedos rm gemma3            # preview only, deletes nothing
hedos rm gemma3 --yes      # actually delete
```

`-y` is a short form of `--yes`. For an Ollama model, `rm` asks the daemon to delete it and shared layers stay; for others, the on-disk paths move to the Trash.

Crucially, **the dry run exits nonzero.** `hedos rm` without `--yes` ends in an error (`nothing deleted — re-run with --yes to confirm`), so a script that forgets the flag fails loudly instead of silently believing it deleted something. See [Exit codes](#exit-codes).

## Residency

`hedos warm` loads a model into memory through the governor, so the next request skips the load. `hedos unload` evicts it.

```sh
hedos warm llama3
hedos unload llama3
```

There is an honesty caveat worth knowing: a model warmed by a bare `hedos warm` is resident only for the life of that short-lived process, so it is effectively unloaded again the moment the command returns. To keep a model hot, warm it inside a running `hedos serve` (or use the app). Ollama models are the exception — Ollama's own daemon holds them loaded and controls their keep-alive, not hedos, which `warm` and `unload` report plainly.

## Serving the gateway

`hedos serve` starts the authenticated, OpenAI- and Ollama-compatible gateway on `127.0.0.1`, the same server the app runs. It stays in the foreground and holds any warmed models resident until you press Ctrl-C.

```sh
hedos serve
hedos serve --port 8080
```

`-p` is a short form of `--port`; without it the configured default port is used. Every request needs a bearer token — mint one with `hedos token new` (below). For the routes it serves, the two API dialects, token scoping, and concurrency behavior, see [the gateway guide](gateway.md).

## Client tokens

`hedos token` manages the gateway's client tokens. With no subcommand it lists them.

```sh
hedos token              # list (same as `token ls`)
hedos token ls
hedos token new "my-editor"
hedos token revoke <client-id>
```

`hedos token new <name>` mints a token and prints it **once** — store it then, it is not recoverable. You can scope it at creation:

- `--models <id> <id> ...` restricts it to specific model ids (default: all).
- `--capabilities <cap> ...` restricts it to specific capabilities (default: all).

```sh
hedos token new "speech-bot" --capabilities speak --models <id-a> <id-b>
```

An unscoped token reaches the whole ready shelf; a scoped one is an exact allowlist. `hedos token revoke <id>` takes a client id (the value shown by `token ls`), not the secret.

## Scripting with `--json`

Every command accepts `--json`. It swaps the human-readable output for a single JSON document on stdout, so you can pipe straight into `jq` without scraping formatted text. Progress and status still go to stderr, keeping stdout clean.

```sh
# Ids of every image model on the shelf
hedos ls --capability image --json | jq -r '.[].id'

# Just the completion text
hedos run qwen2.5 "one word: hi" --json | jq -r '.text'

# What a pull actually landed
hedos pull gemma3:4b --json | jq '{pulled, bytes, shelfCount}'
```

The shapes are per-command — `ls` is an array of model rows, `run` is `{model, text, stats}`, `pull` reports `{pulled, provider, bytes, shelfCount}`, `rm` reports `{model, name, kind, deleted, viaDaemon, paths, bytes}`, and so on. Fields are stable keys with slash-unescaped values and ISO 8601 dates.

## Exit codes

The command exits `0` on success and nonzero on failure, so `&&` chains and `set -e` scripts behave. The case to remember is `hedos rm`: a dry run (no `--yes`) is treated as a failure and exits nonzero, on purpose, so a script never mistakes a preview for a deletion.

```sh
hedos rm gemma3 --yes && echo "gone"     # only prints on a real delete
```

An unresolved or ambiguous model argument, a failed pull, a gated repo without a token, and a gateway that won't bind all exit nonzero with a message on stderr. `--json` does not change the exit code — it still fails nonzero, it just also had nothing meaningful to print.
