# CLI reference

`hedos` is the command-line front end. Each run builds a kernel from your data directory and settings, runs one command, and exits. It needs no GUI, so it works over SSH and in scripts. The one full-screen command is [`hedos shelf`](#hedos-shelf), a terminal UI with [its own guide](shelf.md).

## Commands at a glance

| Command | What it does |
| --- | --- |
| [`hedos scan`](#hedos-scan) | Discover the models on this machine and refresh the shelf. |
| [`hedos ls`](#hedos-ls) | List the models on the shelf. |
| [`hedos run`](#hedos-run) | Stream one completion, or put a typed question to a judge. |
| [`hedos chat`](#hedos-chat) | Chat with a model over stdin. |
| [`hedos speak`](#hedos-speak) | Synthesize speech to a WAV file. |
| [`hedos transcribe`](#hedos-transcribe) | Transcribe an audio file to text. |
| [`hedos image`](#hedos-image) | Generate an image to a PNG file. |
| [`hedos serve`](#hedos-serve) | Run the OpenAI-, Ollama- and Anthropic-compatible gateway on loopback. |
| [`hedos launch`](#hedos-launch) | Run a coding harness against a gateway that lives as long as the harness. |
| [`hedos warm`](#hedos-warm) | Load a model into memory. |
| [`hedos unload`](#hedos-unload) | Evict a model from memory. |
| [`hedos pull`](#hedos-pull) | Fetch a model from Ollama or Hugging Face, and manage the pulls under way. |
| [`hedos recommend`](#hedos-recommend) | Read this machine's hardware and recommend models to pull for it. |
| [`hedos rm`](#hedos-rm) | Remove an installed model. |
| [`hedos runtimes`](#hedos-runtimes) | List the manifest runtimes, and approve or revoke one. |
| [`hedos bench`](#hedos-bench) | Measure what each model does on this machine. |
| [`hedos stats`](#hedos-stats) | Report usage from the gateway's audit log. |
| [`hedos shelf`](#hedos-shelf) | Open the shelf as a terminal screen. |

## Global options

| Flag | Meaning | Default |
| --- | --- | --- |
| `--json` | Print machine-readable JSON on stdout instead of formatted text. Accepted by every command. | off |

Human notices (status lines, prompts, progress, spinners) always go to stderr, so `--json` output stays clean for piping.

## Naming a model

### How a name resolves

Commands that take a model accept an id, a name, an alias, or a unique substring. hedos tries them in this order:

1. An exact id.
2. An exact name, ignoring case.
3. A unique substring of the name or the id, ignoring case.

If more than one model matches, the command lists up to eight candidates and asks you to be more specific. Commands that need a capability (`chat`, `speak`, `transcribe`, `image`, `bench`) only match models that serve it.

A model whose weights are gone from disk is refused, with a note to pull it again or to drop the record with `hedos rm`. `rm` itself accepts such a model, since dropping the record is exactly what it is for.

### Picking a model interactively

Every command that takes a model can be run without one. In a terminal, hedos opens a fuzzy-filterable picker of the eligible models, with the same columns as `ls`:

- type to narrow the list,
- use the arrow keys to move,
- press Enter to choose, or Esc to cancel.

The list is scoped to what the command needs, so `speak` only offers speech models and `unload` only offers models that are currently warm. Models that resolved to a runtime are listed first.

### Missing arguments

A missing prompt, text, or path works the same way: in a terminal, hedos asks for it inline.

Outside a terminal (a pipe, a script, or `--json`), a missing argument is a plain error instead of a prompt, so nothing ever blocks waiting on input.

## Finding models

### `hedos scan`

Discover models across the machine's stores, reconcile them into the registry, and resolve each to a runtime.

```sh
hedos scan
```

It takes no flags of its own. It prints:

1. a one-line summary,
2. the per-store split: each store's model count and the bytes its files take on disk,
3. the models that can go because another model keeps everything they hold,
4. how many of the models found are too big for this machine.

Each paragraph appears only when it has something to say. Issues go to stderr, each starting with `issue:`.

```
Found 9 models on this Mac (4 in Ollama, 2 in LM Studio, 1 built in, 2 loose files). Total: 28.3 GB.

ollama     4  14.2 GB
lm-studio  2   9.4 GB
builtin    1
file       2   4.7 GB

2 models are copies of weights another model keeps (identical in size and sampled content). Removing all of them frees up to 9.4 GB, each file counted once; a row says what removing that copy alone frees:
  keep qwen2.5:7b (ollama) [same files: qwen2.5:latest (ollama)]
    4.7 GB  Qwen2.5-7B-Instruct-Q4_K_M (lm-studio)
    4.7 GB  qwen2.5-7b-instruct-q4_k_m (file)
  A name in brackets reaches the same files as the copy before it. Removing all of them frees the space; removing one alone can free nothing while another keeps the files (a hard link, a second Ollama tag), and removing a symlink frees nothing.

1 model is too big for this machine. `hedos ls` marks it.
```

When the scan finds nothing, it prints `No models found on this Mac yet.` and nothing more.

#### What the sizes count

The sizes are bytes on disk, with each file counted once by device and inode across every store:

- Two Ollama tags over one blob, a hard link, or a symlink add nothing after the first.
- A file two stores reach counts under the first store in the order listed.
- The Total is the sum of the store rows.
- A copy-on-write clone (`cp -c`, a Finder duplicate) is its own file and counts in full, though it shares its blocks with the original.
- A Hugging Face repo or folder bundle counts every file in its directory, Finder's `.DS_Store` included, because `hedos rm` deletes them all.

The machine card of [`hedos shelf`](shelf.md#the-machine-card) counts disk the same way, so its figures match these. `hedos ls --json` gives each model's own `footprint_bytes`, in which a blob two tags share counts for each of them.

#### When a model is offered for removal

A model is offered for removal when another model keeps everything that removing it deletes. Each file `hedos rm` removes for it must be either:

- **a file the other model reaches too**, which frees nothing and loses nothing, or
- **a copy of one**: the same size and the same sampled content.

The files `hedos rm` removes for a model are:

- an Ollama model's layer blobs,
- every file in a Hugging Face repo's directory, whether in `blobs/`, a snapshot, an older snapshot, or the repo's root,
- every file of a folder bundle,
- a GGUF's file or shard set.

The sampled content is the first 8 MiB of each file and 256 blocks of 64 KiB spread evenly to its end. The heads are compared first, and two files stop being read at the first sample where they differ.

#### Bookkeeping files

Bookkeeping that holds no model content needs no counterpart, but it counts toward what removing the model frees. A file is bookkeeping only in the exact shape its writer gives it, so nothing a person put there passes for it:

| Bookkeeping | Shape it must have |
| --- | --- |
| A ref under a repo's `refs/` | A regular file of at most 64 bytes holding a commit hash (40 hex characters). |
| A `.no_exist/` marker | Empty. |
| A snapshot's links | Links into the repo's own `blobs/`. |
| An Ollama model's manifest and config blob | As the daemon writes them: no field it never writes, at most a MiB. A cloud model's config is that model, so it is not bookkeeping. |
| A Finder `.DS_Store` | Its magic, at most 64 KiB. |
| A `._X` AppleDouble file | Its magic, at most a MiB, and only when the file `X` it describes is removed with it. |

Any other file is content. So a repo holding a file of its own under `refs/` is not offered.

A folder bundle made by `hf download --local-dir` keeps that download's metadata under `.cache/huggingface/`, which is content too. Two such bundles, or one beside the repo it came from, are not offered.

#### What is never offered

- A repo or folder bundle that is itself a symlink, or a repo whose `blobs/` is one: `hedos rm` would remove only the link.
- What such a link points to.
- Anything another model on the shelf reaches through a symlink (its own path, a linked snapshot or subdirectory, a link to a link, whatever the case or Unicode form the link is typed in), since removing it would leave that model with nothing.
- A model whose removal would delete a watched folder, or a symlink on the way to one.

#### The rule runs one way

Being a copy is not symmetric:

- A loose or LM Studio GGUF of an Ollama model's weights is offered, with the Ollama model kept.
- The Ollama model is not offered in its favour when it has a template or parameters layer, which has no counterpart in the GGUF.
- An Ollama model with no such layer (as `ollama create` makes from a bare GGUF) can be offered in favour of an LM Studio or loose copy that cannot go itself. That loses nothing, since the copy keeps the weights.
- A loose file holding one quantization of a Hugging Face repo is offered, and the repo, holding others, is not.
- A folder bundle whose second shard differs is not offered.

#### How close a match has to be

Weights that differ throughout, as a fine-tune does, are not matched. A difference narrower than the step between two sampled blocks can be missed, though: under 1 MiB for a 256 MiB file, and about 4 MiB more for each GiB. So "a copy" means identical in size and sampled content, not a byte-for-byte proof.

#### How the groups read

Each group starts with the model kept (`keep`), then lists each model that can go, with the bytes removing that model alone frees:

- Those bytes are the files it alone names, through every hard link each one has. Hard links are counted as distinct directory entries, so a folder watched under two spellings is not two links.
- A file another model still reaches (a shard hard-linked into another set, a blob another Ollama model uses), or a hard link kept elsewhere, counts nothing there.
- A model must free at least 256 MiB to be offered.

The sentence above the groups gives what removing every model listed frees, each file counted once. That can be more than the rows add up to: when two of them share a file through a hard link (or as one Ollama blob), the file goes only with the last of them.

Removing every model listed loses nothing. No kept model is offered, and a model is never offered when removing it would take away a file another model lists or links to. The one exception is a blob two Ollama models list, which the daemon keeps until the last of them goes.

The total is an upper bound: deleting a copy-on-write clone frees less than its size. The figures and the copy rule cover file contents, not extended attributes or resource forks.

#### Which copy is kept

Which model is kept is decided once the content has been compared, in this order:

1. A model that cannot go.
2. One with a file it names other than through a symlink.
3. The best store among its names: Ollama, then the Hugging Face cache, then LM Studio before a loose file. So a loose file hard-linked to an Ollama blob ranks as Ollama.
4. Its name.

#### Names, aliases and links

Models whose removal deletes the same files (a symlink, a hard link, another Ollama tag over the same blobs, one folder under two spellings) are one copy. The copy is named by a path that is not a symlink when one reaches it, and the others follow it in brackets (`[same files: ...]`).

Removing all of them frees the space. Removing one alone can free nothing while another keeps the files, and removing a symlink frees nothing. So a copy whose every file is reached only through a symlink is labelled with the file it points to (`a link to ...`) and is kept. When the groups hold such a link but no bracketed names, the closing line says `Removing a symlink frees nothing: the file it points to holds the bytes.`

A name that means more than one model on the shelf (two loose files both named `model`) also gets its path.

#### The too-big count

The too-big count is the same verdict as the FIT column of [`hedos ls`](#hedos-ls), judged from what serving each model loads, over the models this scan found. A model whose weights are gone, or whose store could not be read this time, is not counted. Nothing is said about fit when the scan found nothing.

#### Unprintable names and multi-line issues

A control character, a bidirectional control (U+202A to U+202E, U+2066 to U+2069, U+200E, U+200F, U+061C), or a line or paragraph separator (U+2028, U+2029) in a name or path is printed as its escape (`\u{1b}`, `\n`, `\u{202e}`) rather than sent to the terminal, and a backslash is doubled.

An issue that spans several lines (a TOML parse error's caret diagram) is printed a line at a time, each line escaped the same way.

#### JSON output

`--json` prints one object. Names and paths in it are the raw strings.

| Key | Meaning |
| --- | --- |
| `totalCount` | How many models the scan found. |
| `headline` | The one-line summary. |
| `issues` | The issues, as strings. |
| `totalBytes` | The Total. |
| `stores` | One entry per store row, in the order above: `store`, `count`, `bytes`. |
| `duplicates` | One entry per group, described below. |
| `reclaimableBytes` | What removing every copy offered in every group frees, each file counted once. |
| `fit` | The fit tally: `runsWell`, `tightFit`, `tooLarge`, `unknown`. |
| `memoryBytes` | The machine's total memory. Each model's fit was judged against what its engine may use of it (see [Fit and memory](models.md#fit-and-memory)). |
| `failedStores` | The stores whose scan failed outright. |

Each group in `duplicates` carries:

- `reclaimableBytes`: what removing every copy it offers frees, each file counted once.
- `kept`: the copy kept.
- `removable`: the copies that can go.

Each copy carries `id`, `name`, `store`, `path`, `aliases` (the other names reaching the same files, each with the same four fields), and `linkTarget` (the file behind a copy whose every file is reached only through a symlink, or `null`). Each removable copy also carries its own `reclaimableBytes`: what removing that copy alone frees.

### `hedos ls`

List the shelf: a warm indicator, the name, the runtime, the store, a memory-fit verdict, and the capabilities.

```sh
hedos ls [--scan] [--capability <name>]
```

| Flag | Meaning | Default |
| --- | --- | --- |
| `--scan` | Rescan the machine's stores before listing. | off |
| `--capability <name>` | Show only models serving that capability, for example `embed`. | all models |

If the shelf is empty, `ls` runs a scan first.

```sh
hedos ls
hedos ls --capability tools
hedos ls --json
```

```
   NAME                         RUNTIME        STORE              FIT      CAPABILITIES
●  qwen2.5:7b                   ollama         ollama             fits     chat, complete, tools
○  gemma3:12b                   ollama         ollama             tight    chat, complete, see
○  Llama-3.3-70B-Instruct-4bit  python:mlx-lm  huggingface-cache  too big  chat, complete, tools
○  whisper-base                 whisper-cpp    file               —        transcribe
✕  phi4-mini                    llama-cpp      lm-studio          gone     chat, complete
```

#### The columns

- **The first column** is a filled dot for a model that is warm (in this process, the Ollama daemon, or a running gateway), a hollow dot for one that is cold, and a cross for one whose weights are gone.
- **RUNTIME** is the runtime the model resolved to, or a dash when none did.
- **FIT** reads `fits`, `tight`, or `too big`, judged from the model's estimated footprint against what the engine it resolved to may use on this machine: the GPU's share of memory where there is one, else all of it (see [Fit and memory](models.md#fit-and-memory)). It is the same assessment [`hedos recommend`](#hedos-recommend) uses. A dash means the footprint is unknown, and `gone` means the weights are gone, so there is nothing left to fit.

When a model on the shelf waits on a manifest runtime you have not approved, `ls` adds a line under the table naming the models and the command that approves it (`hedos runtimes approve <id>`). See [`hedos runtimes`](#hedos-runtimes).

A control character, a bidirectional control, or a line or paragraph separator in a model's name is printed as its escape (`\u{1b}`, `\n`, `\u{202e}`), and a backslash is doubled, so a name cannot color the terminal, split a row, or reorder it.

#### Serving size and disk size

The footprint that fit is judged on is what serving the model loads, which is not always what it takes on disk. A Hugging Face repo that holds several quantizations, or blobs from older revisions, serves one weight set (with its projector and config).

For a Hugging Face repo, the model's own disk figure is its `blobs/`; for a folder bundle, its visible top-level files. So it is below what `hedos rm` frees when the directory holds more. The per-store bytes and the duplicate figures of [`hedos scan`](#hedos-scan) count the whole directory.

The shelf screen uses the same figures: see [sizes on the shelf](shelf.md#sizes-on-the-shelf).

Sizes are decimal, as the hubs state them (`4.9 GB` is 4.9e9 bytes). Memory figures are in GiB.

#### JSON output

`--json` prints the shelf as an array of records, each with every field of the record plus:

- `fit`: `runs_well`, `tight_fit`, `too_large`, or `null` when the size is unknown or the weights are gone. The record's own `state` field says which.
- `footprint_bytes`: the model's own disk figure. A blob two Ollama tags share counts for each.
- `serving_bytes`: the serving figure, present when the store measured it apart from the disk figure.

## Running models

### `hedos run`

Stream a single completion to stdout, put a typed question to a judge, or find contacts in a text with an extractor.

```sh
hedos run [model] [prompt] [flags]
```

| Flag | Meaning | Default |
| --- | --- | --- |
| `--system <text>` | A system prompt for this run. | none |
| `--max-tokens <n>` | Cap the generated length. | the runtime's |
| `--temperature <f>` | The sampling temperature. | the runtime's |
| `--image <path>` | Attach a local image for a vision (`see`) model to read. Repeat it for several images. | none |
| `-f`, `--file <path>` | An extractor's text, read from a file. | none |
| `--contacts` | Have an extractor group what it finds into contacts. | off |
| `--address` | Have an extractor read the text as one address and split it into parts. | off |
| `--kinds <list>` | Only these kinds, comma-separated: `person`, `org`, `address`, `email`, `phone`. | every kind |
| `--country <list>` | Region hints for an extractor, comma-separated, in place of the one its model expects. | the model's |

Omit the model to pick one interactively, and omit the prompt to type it at a prompt. The picker offers the models that chat, answer typed questions, or extract; with `--image`, only vision-capable models. A named model that does none of them is refused, and so is a model that cannot see when `--image` is given, rather than answering blind.

```sh
hedos run gemma3 "explain this"
hedos run llava "describe" --image photo.png
hedos run qwen3 "summarize" --system "Be brief." --max-tokens 200
hedos run gemma3 "hi" --json
```

A spinner stands in until the first token. Ctrl-C cuts the answer short, and what streamed so far stands.

Under `--json`, streaming is suppressed and the full text plus the model id is printed as one object at the end (`model`, `text`).

The image's type is read from its extension (`.jpg`, `.jpeg`, `.webp`, `.gif`, else PNG). A path that cannot be read fails before anything is sent.

#### Judges

A judge is a model that answers typed questions, such as a decision GGUF or laya. It takes a question rather than a prompt:

- Pass the question as JSON: `{"state": ..., "questions": {...}}`.
- Or omit it in a terminal to compose one. hedos asks for a situation (Enter skips it when the question stands on its own), what to answer with (`choice`, `score`, or `noul`), the question, and, for a choice or a score, at least two options or levels, ended by an empty line. It then offers to add another question about the same situation.

The answer is laid out as each option's probability, with the model's pick marked; under `--json` the envelope is printed as it came.

```
how should support answer?
  ▸ refund     █████████████████          71.2%  money back
    replace    █████                      20.0%
    apologize  ██                          8.8%
  confidence 0.80
```

A judge answers in one pass, so `--system`, `--max-tokens` and `--temperature` have no effect on it; hedos says so on stderr if you pass them. Ctrl-C while it weighs prints nothing and is not an error.

#### Extractors

An extractor such as [Tessera](models.md#extractors) takes a text: as the argument, from `--file`, piped in, or typed at a prompt. It lists every entity it finds by default, groups them with `--contacts`, and splits one address with `--address`:

```
$ hedos run tessera --contacts "Jordan Lee, 123 Main St, Bismarck, ND 58501, (701) 555-0142, jordan@acme.example"
Jordan Lee · 0.95
  address  123 Main St, Bismarck, ND 58501  0.95
  email    jordan@acme.example              0.99  jordan@acme.example
  phone    (701) 555-0142                   0.99  +17015550142
```

Under `--json` the extractor's answer is printed as it came. A confidence it suggests a person check is marked `review`. `--system`, `--max-tokens`, `--temperature` and `--image` have no effect on an extractor, and hedos says so on stderr if you pass them. A request the extractor refuses (an unknown kind, say) fails with its reason.

`--image` on a judge that sees (clef, OpenJev) adds each file to the question's `images` as a data URL, leaving the rest of the question as written. A question that already lists its `images` cannot take `--image` too, and a question that carries images is refused by a judge that cannot see.

### `hedos chat`

An interactive session that reads turns from stdin and streams each reply.

```sh
hedos chat [model] [flags]
```

| Flag | Meaning | Default |
| --- | --- | --- |
| `--system <text>` | A system prompt for the conversation. | none |
| `--max-tokens <n>` | Cap each reply. | the runtime's |

```sh
hedos chat qwen3
echo "what is a kv cache?" | hedos chat qwen3
```

- When stdin is a terminal, it prints a banner and a `›` prompt on stderr. When stdin is a pipe, it just reads lines.
- Empty lines are skipped. The whole conversation so far goes with each turn.
- Ctrl-C stops the reply in progress and returns to the prompt. Ctrl-D ends the session.
- Under `--json`, each reply is printed as an object: `{"role": "assistant", "content": ...}`.

### `hedos speak`

Synthesize speech and write a WAV file. There is no playback.

```sh
hedos speak [model] [text] [flags]
```

| Flag | Meaning | Default |
| --- | --- | --- |
| `--voice <name>` | The voice to use. | see below |
| `--speed <f>` | The speed multiplier. | `1.0` |
| `-o, --output <path>` | The output file. | the text, slugged, with `.wav`, in the current directory |

Omit the model or the text to be prompted for them.

When a model has several voices and `--voice` is not given, hedos offers a picker in a terminal; otherwise it uses the first bundled voice.

The default file name is the text lowercased, with each run of other characters turned into a `-`, cut at about 40 characters (`output.wav` when nothing is left).

```sh
hedos speak kokoro "Hello there."
hedos speak kokoro "Hello there." --voice af_heart --speed 1.2 -o hello.wav
```

```
wrote hello-there.wav
```

Under `--json`, it prints `model`, `path`, and `voice`.

### `hedos transcribe`

Transcribe an audio file to text through a local whisper model: the inverse of `speak`.

```sh
hedos transcribe [model] [audio] [flags]
```

| Flag | Meaning | Default |
| --- | --- | --- |
| `--language <code>` | Force the source language, for example `en`. | auto-detect |
| `--translate` | Translate to English instead of transcribing verbatim. | off |

Omit the model or the audio path to be prompted for them. The transcript streams to stdout as it is produced.

The audio is a WAV file, and the path may start with `~`, even when typed at the prompt.

```sh
hedos transcribe whisper voice.wav
hedos transcribe whisper ~/notes.wav --language de --translate
```

Under `--json`, the model, the path, and the full transcript are printed as one object (`model`, `path`, `text`).

### `hedos image`

Generate an image and write a PNG file. This runs as a job, with progress on stderr.

```sh
hedos image [model] [prompt] [flags]
```

| Flag | Meaning | Default |
| --- | --- | --- |
| `--steps <n>` | The number of diffusion steps. | the runtime's |
| `--seed <n>` | The random seed. | the runtime's |
| `-o, --output <path>` | The output file. | the prompt, slugged, with `.png`, in the current directory |

Omit the model or the prompt to be prompted for them. The default file name is slugged from the prompt the same way `speak` slugs its text.

```sh
hedos image flux "a koala on a bookshelf" --steps 4 --seed 7
```

```
wrote a-koala-on-a-bookshelf.png
```

Under `--json`, it prints `model` and `path`.

## Serving

### `hedos serve`

Start the OpenAI-, Ollama-, and Anthropic-compatible gateway on loopback, and block until Ctrl-C, SIGTERM, or SIGHUP. See the [gateway guide](gateway.md).

```sh
hedos serve [-p <port>]
```

| Flag | Meaning | Default |
| --- | --- | --- |
| `-p, --port <n>` | The port to bind. | `gateway.port` from settings, else `43367` |

```sh
hedos serve
hedos serve -p 8080
```

```
gateway listening on http://127.0.0.1:43367/v1
auth: open (loopback) — any local client is allowed. Ctrl-C to stop.
```

The first line goes to stdout; the auth notice goes to stderr. Under `--json` it prints `running`, `port`, and `baseUrl`.

Stopping:

- **Ctrl-C** stops taking requests and waits for the ones in flight. A second Ctrl-C ends them.
- **SIGTERM or SIGHUP** waits 5 seconds for the requests in flight, then ends them.

### `hedos launch`

Run a coding harness against a gateway served for exactly as long as that harness runs. There is nothing to start first: the gateway binds a free port inside the same process, the harness is spawned pointed at it, and the gateway stops when the harness exits.

```sh
hedos launch [harness] [-m <model>] [-- <harness args>...]
```

| Flag | Meaning | Default |
| --- | --- | --- |
| `-m, --model <name>` | The model to open the harness on. | pick interactively |
| `-- <args>` | Anything after `--` is passed straight through to the harness. | none |

```sh
hedos launch                  # pick a harness, then a model
hedos launch opencode         # pick a model
hedos launch claude -m qwen3
```

```
Claude Code · qwen3 · gateway on 127.0.0.1:52144
```

Under `--json` it prints `harness`, `model`, `port`, and `dialect`.

#### Supported harnesses

| Harness | Binary | Dialect |
| --- | --- | --- |
| Claude Code | `claude` | Anthropic |
| OpenCode | `opencode` | OpenAI |
| Aider | `aider` | OpenAI |
| Goose | `goose` | OpenAI |
| Crush | `crush` | OpenAI |

Omitting the harness in a terminal lists only the ones actually installed. Naming one that is not on your `PATH` says where to get it.

Codex is not supported: it speaks the OpenAI Responses API, which this gateway does not serve, and it removed the setting that made it speak chat completions.

#### The pre-flight check

Before the harness starts, hedos runs one throwaway request through the model, shaped like the ones the harness will send.

- A model whose backend is down (a stopped Ollama daemon, a missing `llama-server`, an out-of-memory GPU) fails here with the reason and what to do about it, rather than inside the harness, where it reads as an unexplained error.
- It leaves the model loaded, so the first real request is warm.
- When the model's context window is under 16,384 tokens, hedos warns that the harness's opening prompt often runs past it.

#### Tool calls

Every harness here except Aider drives the model entirely through tool calls, so it needs a model that supports them.

- Tool support shows as a `tools` capability in `hedos ls` and the picker, read from the model's chat template during discovery.
- The launch picker offers only tool-capable models to the harnesses that need them. An explicit `-m` resolves against every chat model, so a model without tools gets a precise reason instead of "no such model".
- This includes models served by the MLX sidecars: the offered tools are rendered through the model's own chat template and the calls are parsed back out of its reply, so an MLX build of Llama or Qwen seats a harness the same way an Ollama model does.
- Apple Intelligence seats them too: the bridge offers the tools to Apple's model and captures the calls it makes back out.
- A model whose tool support couldn't be read from disk is assumed capable and left in the list. The pre-flight then probes with a tool and catches it before the harness starts, with a note to pick another model or use Aider (whose edits are plain text and need no tools).

#### Your config and the model list

Your own harness config is never read around or written to. Harnesses that can be configured through the environment are; the rest get a generated config under the hedos data directory (owner-only), so running the harness directly afterwards behaves exactly as it did before.

The whole tool-capable shelf (the whole chat-capable shelf, for Aider) is offered inside the harness, not just the model you named, so you can switch models there. `-m` only chooses the one it opens on.

#### Interrupts and exit code

Ctrl-C goes to the harness, not to hedos, so it handles the interrupt the way it normally would. The harness's exit code becomes the exit code of `hedos launch`. A harness killed by a signal other than SIGINT exits `128 + signal`.

### `hedos warm`

Load a model into residency with a tiny request, so the next real request starts warm, and report whether it is resident afterwards.

```sh
hedos warm [model] [-p <port>]
```

| Flag | Meaning | Default |
| --- | --- | --- |
| `-p, --port <n>` | The port of a gateway to warm on, when it is not the configured one. | the configured port, if a gateway answers there |

```sh
hedos warm qwen3
hedos warm qwen3 --port 8080
```

```
qwen3 is warm on the gateway :43367
```

A model is warm where it is served:

- When a gateway is running on the configured port, or on the one `--port` names, the model is loaded there rather than in this command's own process, which would exit and take the loaded model with it. `--port` also reaches a gateway started with `hedos serve --port`, which is not the one this command probes for. A named port is taken at its word rather than probed.
- A model whose warm request is not a conversation, a speech model for instance, is loaded locally either way: the gateway's chat endpoint has no route for it.

The probe fits the model. A judge is asked the smallest well-formed typed question rather than being greeted, since prose is the one thing it refuses, and on a running gateway it is asked on `/v1/systemone`, the route it is served on.

A runtime whose residency hedos does not track reports `loaded (residency not tracked for this runtime)`. Under `--json` it prints `model`, `resident`, and, when warmed on a gateway, `gatewayPort`.

### `hedos unload`

Evict a model from residency and report the result.

```sh
hedos unload [model]
```

Omit the model to pick from the models that are currently warm. hedos evicts the model from this process and, when the Ollama daemon holds it, asks the daemon to unload it and waits for it to let go.

```
qwen3 unloaded
```

If the model is still resident afterwards, the line reads `<name> is still resident`. Under `--json` it prints `model` and `resident`.

## Installing and removing

### `hedos pull`

Fetch a model from Ollama or Hugging Face, and manage the pulls under way.

```sh
hedos pull [reference] [--from <ollama|hf>] [-d]
hedos pull <subcommand> ...
```

| Flag | Meaning | Default |
| --- | --- | --- |
| `--from <ollama\|hf>` | Force the provider (`huggingface` also works). | inferred from the reference |
| `-d, --detach` | Start the download and return straight away. | follow it |

```sh
hedos pull qwen2.5:3b                       # an Ollama tag
hedos pull Qwen/Qwen2.5-1.5B-Instruct       # a Hugging Face repo
hedos pull gemma3:4b -d                     # start it and return
hedos pull                                  # search interactively
```

#### Choosing what to pull

- The reference is a Hugging Face repo (`org/model`) or an Ollama tag (`gemma3:4b`). hedos infers the provider from the shape; `--from` forces it.
- Omit the reference in a terminal to search. Type a query to search Hugging Face (results show download and like counts), or leave it blank for the picks [`hedos recommend`](#hedos-recommend) makes for this machine that are not on the shelf yet, each with its download size, its fit, and what to install first when the engine that serves it is missing. A "search again" entry in the list returns to the prompt, so you can move between recommendations and a search, or try another query, without restarting the command.
- Before any bytes move, hedos shows the plan (the name, the destination, and the size) and asks `Download now?`, defaulting to yes. Outside a terminal there is no prompt.
- Gated Hugging Face repositories need a token with access to the repo (`HF_TOKEN`, `HF_TOKEN_PATH`, or `huggingface-cli login`), and you must accept the model's terms on its Hugging Face page first.

#### How a pull runs

The download runs in a worker process of its own, so it outlives the terminal that started it.

- `hedos pull` follows that worker's progress. Ctrl-C detaches from it rather than cancelling.
- `-d` starts the download and returns straight away, printing the commands that reach it:

  ```
  pulling gemma3:4b in the background as <job>
    watch:  hedos pull attach <job>
    stop:   hedos pull cancel <job>
  ```

- The worker scans when it finishes, so the model reaches the shelf whether anything is watching or not. It waits at most `pull.register_timeout_seconds` for that scan; a slower one is left to the next scan. See [configuration](configuration.md).
- Pulling a model that is already being fetched joins that download instead of starting a second one. Pulling one that stopped part-way carries on from the bytes on disk.

#### Managing pulls

The pulls under way are managed under the same verb:

```
hedos pull ls                  every pull, its state, progress, and what it is waiting for
hedos pull attach <job>        follow one again; Ctrl-C detaches
hedos pull pause <job>         stop it, keeping what it has downloaded
hedos pull resume <job>|--all  start a stopped one again
hedos pull cancel <job>        stop it for good
hedos pull logs <job> [-n n]   its history
hedos pull clean [--keep n]    drop the records of ended pulls past the newest n (pull.keep_ended)
```

| Subcommand flag | Meaning | Default |
| --- | --- | --- |
| `resume --all` | Start every paused or interrupted pull. Cannot be combined with a job. | off |
| `logs -n, --lines <n>` | Show only the last `n` lines. | the whole history |
| `clean --keep <n>` | Keep the newest `n` ended pulls, however old; `0` drops every ended pull. | `pull.keep_ended` (20) |

`pull ls` prints a table with the columns ID, REFERENCE, STATE, PROGRESS, and NOTE.

A job is named by its id, an unambiguous prefix of one, or its reference. A name several pulls answer to means the one still going.

Since a bare word is a valid Ollama tag, a subcommand name shadows a model of the same name. Write `hedos pull -- ls` to pull a model called `ls`. A reference or `-d`/`--from` written beside a subcommand is refused rather than silently dropped.

#### Pausing and cancelling

`pull pause` and `pull cancel` wait a few seconds for the worker and report what it did:

| Outcome | Line | `--json` `outcome` | Exit |
| --- | --- | --- | --- |
| The worker stopped | `paused <job>` or `cancelled <job>` | `"honoured"` | 0 |
| The worker has not answered yet (the ask stands) | `pausing <job>; its worker has not answered yet` (or `cancelling`) | `"pending"` | 0 |
| Every byte landed first | `<job>: every byte landed before the pause was read; it is done` (or `cancel`) | `"too_late"` | 1 |
| The pull ended some other way first (a cancel that overtook the pause, say) | `<job> ended <state>, so the pause had no effect` (or `cancel`) | `"too_late"` | 1 |

Under `--json` both print the job's record with the `"outcome"` field. A stop that came too late is reported on stderr, as an error.

A few edge cases:

- A pull being registered refuses both, naming how many seconds the registration has left.
- A cancel written just after the worker stopped reading (it was already honouring a pause) is settled by the command once that worker exits, so it reads `cancelled <job>` rather than being left for the next resume. The shelf's pulls screen does the same for `c x` pressed while a pause is being honoured.
- A pause asked while a cancel is still waiting to be read is refused, and the cancel stands: a pause never turns "stop for good" into "stop for now".
- If another process holds the job's control file for more than 3 seconds, `pause`, `cancel`, `resume`, and the pulls screen's `c` and `R` give up, say so, and change nothing. Opening the shelf skips such a pull and takes it up the next time.

#### Interrupted, superseded, and unreadable pulls

**Interrupted.** A pull no worker ever took up (its worker died before it started) reads `interrupted` with the note `no worker`, in `pull ls`, on the shelf's pulls screen, and to `pull resume --all`, which starts every paused or interrupted pull. Under `--json` its `state` is `"interrupted"` and it carries `"abandoned": true`; the key is absent for every other pull.

**Superseded.** Once such a pull's model has been pulled by another job (one under way, or one that reached `done` after this pull was created, however old that job is), it reads `failed` instead, with the note `no worker took it up; the model was pulled again`. Every surface agrees:

- `pull ls` shows it failed,
- `--json` shows `"state": "failed"` and `"superseded": true`,
- the pulls screen offers `x`, not `R`,
- `pull attach` reports it failed,
- `pull resume` refuses it,
- `pull resume --all` and opening the shelf skip it.

Listing it changes nothing. The first command that acts on it (`pull resume`, `pull resume --all`, `pull cancel`, `pull clean`, opening the shelf, or a resume or forget from the pulls screen) writes that `failed` into its record, so it stays failed once the other job is cleaned away or fails. From then on `--json` reads it as a plain failed pull carrying the same message, without `"superseded"`. `pull clean` collects it with the other ended pulls.

**Unreadable.** A pull whose record cannot be read (a damaged `status.json`, or one a newer hedos wrote in a state this build does not know) reads `unreadable`, with the reason as its note and `"state": "unreadable"` under `--json`. Nothing touches it:

- `pull pause`, `pull cancel`, and `pull resume` refuse it,
- `pull resume --all` and opening the shelf skip it,
- `pull clean` and the pulls screen's `x` leave it,
- pulling the same model starts a new job beside it.

Delete its directory under the pull store by hand once it is no longer wanted.

### `hedos recommend`

Read this machine's hardware and recommend models from hedos's catalog to pull for it.

```sh
hedos recommend [--kind <chat|code|voice|image>]... [--all]
```

| Flag | Meaning | Default |
| --- | --- | --- |
| `--kind <kind>` | Only this kind of model. Repeat it for more than one. | every kind |
| `--all` | Every model in the catalog, each with where it stands on this machine. | the picks and what is on the shelf |

```
Apple M5 Pro · 6 performance + 12 efficiency cores · 64 GiB memory
51.8 GiB for models on the GPU, its Metal working set
207.9 GiB free on disk

chat
  qwen3.8:27b                               17.7 GB  fits      Qwen's newest, strong on research and long tasks.
  gemma4:26b                                18.7 GB  fits      A mixture of experts: big-model answers at a small model's pace.
  gemma4:31b                                20.4 GB  fits      Gemma's flagship, for a machine with room to spare.

code
  qwen2.5-coder:14b                         9.0 GB   fits      A strong local coding model with a large context.
  qwen3.6:27b-coding                        17.8 GB  fits      Qwen3.6 tuned for agentic coding.
  qwen3-coder:30b                           18.6 GB  fits      Agentic coding over long context. A mixture of experts.

voice
  mlx-community/Kokoro-82M-bf16             0.3 GB   on shelf  Tiny, warm text-to-speech. Instant on Apple Silicon.

image
  stabilityai/sdxl-turbo                    26.9 GB  fits      Images in one to four steps, fast enough to iterate on.
  stabilityai/stable-diffusion-xl-base-1.0  41.2 GB  on shelf  Dependable, well-supported image workhorse.

hedos pull <name> fetches one · hedos recommend --all shows every model and why
```

Nothing is pulled; `hedos pull <name>` does that. The blank search of `hedos pull` and the shelf's pull screen offer the same picks.

#### What it reads

- **The memory a model may use.** On Apple Silicon that is Metal's working set, the share of memory the GPU may take, which every Metal engine budgets against: 51.8 GiB of 64 on the machine above. On Linux it is each NVIDIA card's memory (from `nvidia-smi`) or AMD card's (from the driver's files). On a machine with neither it is all of memory, and models run on the processor. When none of those answer, `llama-server --list-devices` is asked, when it is on `PATH`. None of this needs a model engine installed.
- **The engines.** Whether Ollama (installed, or its daemon answering), `llama-server`, and `uv` are there.
- **The chip, cores and memory**, for the header, and **the free disk** where each provider's pulls land: the Ollama models directory, and the Hugging Face cache. It reads one figure when both are on the same disk.

#### How the picks are made

Each catalog model is judged by what serving it loads, on the engine that serves it: Ollama for every chat and code model, mlx-audio for speech (Apple Silicon only), diffusers for images. Each kind picks the largest models that run well, up to three, among those its engine can run here and that are not on the shelf. When none runs well, it picks the smallest that still fits, and when none fits, it says so (`nothing of this kind fits this machine`). A model on the shelf is listed as `on shelf` and takes no pick.

A card that cannot hold a model whole is not the end of it: Ollama and llama.cpp run what does not fit on the card from memory, slower. Such a model is judged against all of memory instead whenever that reads better, so a machine with a small card is never judged worse than it would be without one, and the model reads `spills`. A card under 1 GiB (an integrated GPU's carve-out) is not counted at all.

A missing engine never changes what fits. The picks stay the same, a line under the header says what to install first (once for each engine), and each pick that needs it reads `needs Ollama` or `needs uv`. A pick whose download would not fit on the free disk reads `short on disk`.

With `--all`, every model is listed: the picks, `also fits` for one the larger picks passed over, `too big`, `on shelf`, and `can't run here` for one whose engine does not run on this machine.

#### JSON output

`--json` prints `machine` and `recommendations`:

- `machine`: `os`, `arch`, `chip`, `cores` (`performance`, `efficiency`, `logical`), `memory_bytes`, `devices` (each `name`, `kind` `unified` or `discrete`, `memory_bytes`), `devices_from` (`metal`, `nvidia-smi`, `amd-sysfs`, `llama-cpp`, or `none`), `models_budget_bytes`, `free_disk` (each `provider` and the `bytes` free where its pulls land), and `engines` (`ollama`, `llama_cpp`, `uv`).
- `recommendations`: each with `kind`, `reference`, `provider`, `name`, `blurb`, `download_bytes`, `serving_bytes`, `engine`, `status` (`pick`, `on_shelf`, `fits`, `too_large`, `no_engine`), `verdict`, `required_bytes`, `placement` (`gpu`, `spill`, `cpu`), and `notes` (`{"kind": "install", "engine", "hint"}` or `{"kind": "disk", "needs_bytes", "free_bytes"}`). Without `--all`, only the picks and what is on the shelf.

### `hedos rm`

Remove an installed model.

```sh
hedos rm [model] [-y]
```

| Flag | Meaning | Default |
| --- | --- | --- |
| `-y, --yes` | Skip the confirmation. Required to delete outside a terminal. | off |

It always shows a deletion preview first: the item count and the estimated size.

- In a terminal, it then asks for a yes/no confirmation and deletes only if you agree. The default is no.
- Outside a terminal, it does nothing unless `-y` is given, so a script can never delete without asking. It prints `Would delete ... Re-run with -y to confirm.` (and `"deleted": false` under `--json`).

File-backed models are deleted from disk; Ollama models are deleted through the daemon. A record whose weights are already gone can be removed too, which drops it from the shelf.

```sh
hedos rm gemma3
hedos rm gemma3 --yes
```

Under `--json` a deletion prints `model`, `name`, `trashedPaths`, `freedBytesEstimate`, `daemonDeleted`, and `"deleted": true`.

### `hedos runtimes`

List the manifest runtimes on this machine, and approve or revoke one. A manifest runtime runs code on the host, so it neither bids on a model nor serves one until you approve it.

```sh
hedos runtimes [ls]
hedos runtimes approve <id> [-y]
hedos runtimes revoke <id>
```

| Flag | Meaning | Default |
| --- | --- | --- |
| `approve -y, --yes` | Approve without being asked to confirm. Required outside a terminal. | off |

```sh
hedos runtimes
hedos runtimes approve python:zerank
```

- **`ls`** (the default) prints a table with the columns RUNTIME, SERVES, CONSENT, and MODELS. CONSENT reads `approved`, `approved, not downloaded`, `needs approval`, `changed since approval`, or `needs a VM`. With no manifest runtimes, it names the directory to put one in (`runtimes.d` under the data directory).
- **`approve`** prints what you are agreeing to on stderr (its files, what it runs, what it installs, the release it downloads and its sha256, the paths and network it declares, the models it would serve, and its hash), then asks. A runtime that pins a release, such as `cli:tessera`, has this machine's build downloaded and checked against the pin before the approval is recorded; `approve` on an approved runtime whose binary is missing downloads it again. What a runtime declares is its own account and is not enforced: it runs as you, unsandboxed. A runtime that needs a VM cannot be approved in this build.
- **`revoke`** takes an approval back. It works by what the settings hold, so it works even after the runtime's manifest is gone.

The approval is bound to a hash of the runtime's files, so editing any of them asks for it again. After approving or revoking, hedos rescans and says which models the runtime now serves. A gateway that is already running keeps the approvals it booted with; restart `hedos serve` to pick the change up.

## Measuring

### `hedos bench`

Measure what each model actually does on this machine: tokens a second, time to first token, and cold start, the same way for every row so two of them can be compared.

```sh
hedos bench [model...] [flags]
```

| Flag | Meaning | Default |
| --- | --- | --- |
| `--all` | Bench the models too big for this machine too. | off |
| `--runs <n>` | Warm runs per model, after the cold one. | `3` |
| `--max-tokens <n>` | Cap each reply. A reply cut by the cap is still a whole measurement. | `128` |
| `--prompt <text>` | Replace the prompt every model answers. | `Explain how a hash map works, in plain prose, in about two hundred words.` |
| `--keep-warm` | Leave residency alone: nothing is evicted, so nothing is measured cold. | off |

Name models to bench only those, whatever their size, or omit them for every chat model that fits this machine (judged as `hedos ls` judges FIT).

```sh
hedos bench                          # every model that fits
hedos bench qwen3 gemma3             # just these two, whatever their size
hedos bench --all --runs 5           # the too-big ones as well, five warm runs each
```

#### How it measures

For each model in turn, hedos clears it from memory, runs it once cold, then runs it `--runs` times warm, and clears it again so the next one has the machine to itself.

- The first token of the cold run is the COLD figure; the warm runs' medians are the rest.
- A model a running `hedos serve` holds cannot be cleared from another process, so its cold cell reads `held` and its warm figures stand.
- Where a runtime reports the two phases apart, the rate is its own decode figure; where it does not, it is measured from the stream, first token to last.
- Token counts come from the runtime. Where it reports none, they are counted from the text at roughly four characters a token, and the figure wears a `~`.
- Time to first token is always measured here, since it is what a caller waits for.
- Thinking tokens count as generated text, because they cost the same time.

#### Output

The table has the columns NAME, RUNTIME, QUANT, TOK/S, SPREAD, TTFT, and COLD.

- On a terminal, the table redraws in place while the models run and settles into a ranked one, fastest first, which stays in your scrollback.
- Piped, nothing is printed until the end, and the same table arrives as plain text.
- `--json` carries every figure, each run behind it, and where its timing came from.

Ctrl-C stops the bench; what was measured stands. The command exits non-zero when no model produced a figure.

The shelf's [bench screen](shelf.md#the-bench-screen) runs the same driver and shows the same figures.

### `hedos stats`

Read the gateway's audit log back and report usage.

```sh
hedos stats
```

It prints the total request count and the rejection rate, then per model the request count, the error rate, p50/p90/p99 serving latency, and when it was last seen.

```
142 requests · 9 rejected (6.3%)

MODEL       REQUESTS  ERRORS    P50    P90     P99     LAST SEEN
qwen2.5:7b  118       2 (1.7%)  412ms  1310ms  2875ms  2026-10-07T09:14:02Z
gemma3:12b  24        1 (4.2%)  890ms  2140ms  3022ms  2026-10-06T17:40:51Z
```

Under `--json` it prints the full summary. With no audit log yet (nothing has been served), it says so and exits `0`.

## The terminal UI

### `hedos shelf`

Open the shelf as a terminal screen: the same table `hedos ls` prints, with the machine's memory, what is loaded and by whom, disk per store, the gateway's state, background pulls, and a try screen for talking to a model, all on one screen.

```sh
hedos shelf
```

It takes no flags and needs a terminal on both stdin and stdout. Every key is a subcommand: `p` pulls, `w` and `u` warm and unload, `x` removes, `S` serves.

The full guide, with every key and screen, is in [the shelf guide](shelf.md).

## Exit codes

| Code | When |
| --- | --- |
| `0` | Success. Also when the reader of stdout goes away (`hedos run ... \| head -c 1`): the command stops as if it had finished. |
| `1` | Most failures. The error is written to stderr. This includes a failed write to stdout, such as to a full disk. |
| The harness's code | `hedos launch` passes the harness's exit code through. A harness killed by a signal other than SIGINT gives `128 + signal`. |
| `128 + signal` | A command cut short by SIGTERM (`143`) or SIGHUP (`129`). `serve` and `shelf` instead stop in order (draining requests, saving state) and exit normally. |

Some commands also exit non-zero for a reason of their own: `bench` when no model produced a figure, and `pull pause` or `pull cancel` when the stop came too late.
