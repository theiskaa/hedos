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

Discover models across the machine's stores, reconcile them into the registry, and resolve each to a runtime. Prints a one-line summary, then the per-store split (each store's model count and the bytes its files take on disk), the models that can go because another model keeps everything they hold, and how many of the models found are too big for this machine's memory. Issues go to stderr.

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

1 model is too big for this machine's 16 GiB. `hedos ls` marks it.
```

The sizes are bytes on disk with each file counted once, by device and inode, across every store: two Ollama tags over one blob, a hard link, or a symlink add nothing after the first, and a file two stores reach counts under the first store in the order listed. The Total is the sum of the store rows. A copy-on-write clone (`cp -c`, a Finder duplicate) is its own file and counts in full, though it shares its blocks with the original. A Hugging Face repo or folder bundle counts every file in its directory, Finder's `.DS_Store` included, as `hedos rm` deletes them all. The machine pane of `hedos shelf` counts disk the same way, so its figures match these. It counts in a separate process in the background, shows `counting` until the first count finishes and the last count while another runs, so a slow or stalled disk never holds up the screen, and quitting never waits for a count; `hedos ls --json` gives each model's own `footprint_bytes`, in which a blob two tags share counts for each.

A model is offered for removal when another model keeps everything removing it deletes. Each file `hedos rm` removes for it (an Ollama model's layer blobs, every file in a Hugging Face repo's directory, whether in `blobs/`, a snapshot, an older snapshot or the repo's root, every file of a folder bundle, a GGUF's file or shard set) must be either a file the other model reaches too, which frees nothing and loses nothing, or a copy of one: the same size and the same sampled content, the first 8 MiB of each file and 256 blocks of 64 KiB spread evenly to its end. The heads are compared first, and two files stop being read at the first sample where they differ. Bookkeeping that holds no model content needs no counterpart but counts toward what removing it frees, and a file is bookkeeping only in the shape its writer gives it, so nothing a person put there passes for it: a ref under a repo's `refs/` holding a commit hash (a regular file of at most 64 bytes, 40 hex characters), an empty `.no_exist/` marker, a snapshot's links into the repo's own `blobs/`, an Ollama model's manifest and config blob as the daemon writes them (no field it never writes, at most a MiB; a cloud model's config is that model, so it is not), a Finder `.DS_Store` (its magic, at most 64 KiB), and a `._X` AppleDouble file (its magic, at most a MiB) when the file `X` it describes is removed with it. Any other file is content, so a repo holding a file of its own under `refs/` is not offered. A folder bundle made by `hf download --local-dir` keeps that download's metadata under `.cache/huggingface/`, which is content too, so two such bundles, or one beside the repo it came from, are not offered. A repo or folder bundle that is itself a symlink, or a repo whose `blobs/` is one, is never offered, since `hedos rm` would remove only the link. What it links to is not offered either, nor anything another model on the shelf reaches through a symlink (its own path, a linked snapshot or subdirectory, a link to a link, whatever the case or Unicode form the link is typed in), since removing it would leave that model with nothing. Nor is a model whose removal would delete a watched folder, or a symlink on the way to one. The rule runs one way. A loose or LM Studio GGUF of an Ollama model's weights is offered with the Ollama model kept, and the Ollama model is not offered in its favour when it has a template or parameters layer, which has no counterpart there; an Ollama model with no such layer (as `ollama create` makes from a bare GGUF) can be offered in favour of an LM Studio or loose copy that cannot go itself, which loses nothing, since that copy keeps the weights. A loose file holding one quantization of a Hugging Face repo is offered and the repo, holding others, is not, and a folder bundle whose second shard differs is not offered. Weights that differ throughout, as a fine-tune does, are not matched, but a difference narrower than the step between two blocks (under 1 MiB for a 256 MiB file, about 4 MiB more for each GiB) can be missed, so a copy means identical in size and sampled content, not a byte-for-byte proof. Each group starts with the model kept (`keep`), then lists each model that can go with the bytes removing that model alone frees: the files it alone names, through every hard link each has (counted as distinct directory entries, so a folder watched under two spellings is not two links). A file another model still reaches (a shard hard-linked into another set, a blob another Ollama model uses) or a hard link elsewhere keeps counts nothing there, and a model must free at least 256 MiB to be offered. The sentence above the groups gives what removing every model listed frees, each file counted once: more than the rows add up to when two of them share a file through a hard link (or as one Ollama blob), which goes only with the last of them. Removing every model listed loses nothing: no kept model is offered, and a model is never offered when removing it would take away a file another model lists or links to, except a blob two Ollama models list, which the daemon keeps until the last of them goes. Which model is kept is decided once content is compared: a model that cannot go is kept first, then one with a file it names other than through a symlink, then by the best store among its names (Ollama, then the Hugging Face cache, then LM Studio before a loose file, so a loose file hard-linked to an Ollama blob ranks as Ollama), then by name. Models whose removal deletes the same files (a symlink, a hard link, another Ollama tag over the same blobs, one folder under two spellings) are one copy, named by a path that is not a symlink when one reaches it, with the others in brackets after it. Removing all of them frees the space; removing one alone can free nothing while another keeps the files, and removing a symlink frees nothing, so a copy whose every file is reached only through a symlink is labelled with the file it points to (`a link to ...`) and is kept. A name that means more than one model on the shelf (two loose files both named `model`) also gets its path. The total is an upper bound: deleting a copy-on-write clone frees less than its size. The figures and the copy rule cover file contents, not extended attributes or resource forks. The too-big count is the same verdict as the FIT column of `ls`, judged from what serving each model loads, over the models this scan found; a model whose weights are gone, or whose store could not be read this time, is not counted, and nothing is said about fit when the scan found nothing. A control character, a bidirectional control (U+202A to U+202E, U+2066 to U+2069, U+200E, U+200F, U+061C) or a line or paragraph separator (U+2028, U+2029) in a name or path is printed as its escape (`\u{1b}`, `\n`, `\u{202e}`) rather than sent to the terminal, and a backslash is doubled. An issue that spans several lines (a TOML parse error's caret diagram) is printed a line at a time, each escaped the same way.

`--json` keeps `totalCount`, `headline` and `issues` and adds `totalBytes`, `stores` (`store`, `count`, `bytes`, in the order above), `duplicates` (each with `reclaimableBytes`, what removing every copy it offers frees with each file counted once, `kept`, the copy kept, and `removable`, the copies that can go, each copy carrying `id`, `name`, `store`, `path`, `aliases` with the same four fields, and `linkTarget`, the file behind a copy whose every file is reached only through a symlink, or `null`, and each removable one its own `reclaimableBytes`, what removing that copy alone frees), `reclaimableBytes` (what removing every copy offered in every group frees, each file counted once), `fit` (`runsWell`, `tightFit`, `tooLarge`, `unknown`), `memoryBytes`, and `failedStores`, the stores whose scan failed outright. Names and paths in `--json` are the raw strings.

### `hedos ls`

List the shelf: a warm indicator, the name, the runtime, the store, a memory-fit verdict, and the capabilities. If the shelf is empty, it runs a scan first.

- `--scan` rescans before listing.
- `--capability <name>` shows only models serving that capability, for example `--capability embed`.

A control character, a bidirectional control or a line or paragraph separator in a model's name is printed as its escape (`\u{1b}`, `\n`, `\u{202e}`), and a backslash is doubled, so a name cannot color the terminal, split a row or reorder it.

The FIT column reads `fits`, `tight`, `too big`, or `—` (footprint unknown), judged from the model's estimated footprint against this machine's memory — the same assessment the install recommendations use. `--json` carries it as a `fit` field on each record.

The footprint fit is judged on is what serving the model loads, which is not always what it takes on disk: a Hugging Face repo that holds several quantizations, or blobs from older revisions, serves one weight set (with its projector and config). `--json` carries each model's own disk figure as `footprint_bytes` (a blob two Ollama tags share counts for each) and, when the store measured it apart from that, the serving figure as `serving_bytes`. In `hedos shelf` the size column and the size sort follow the serving figure, the detail pane's `size` row adds the disk figure when it differs (`8.5 GB · ctx 32k · 34 GB on disk`), the machine pane's disk per store counts everything on disk with each file once, as `hedos scan` does, and the removal preview counts the model's own disk figure. For a Hugging Face repo that figure is its `blobs/`, and for a folder bundle its visible top-level files, so it is below what `hedos rm` frees when the directory holds more; the per-store bytes and the duplicate figures of `hedos scan` count the whole directory.

Sizes are decimal, as the hubs state them (`4.9 GB` is 4.9e9 bytes); memory figures are in GiB.

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

Start the OpenAI-, Ollama-, and Anthropic-compatible gateway on loopback and block until Ctrl-C, SIGTERM, or SIGHUP. Prints the base URL. Ctrl-C waits for the requests in flight and a second Ctrl-C ends them; a termination waits 5 seconds. See the [gateway guide](gateway.md).

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

Fetch a model from Ollama or Hugging Face. The download runs in a worker process of its own, so it outlives the terminal that started it; `hedos pull` follows that worker's progress, and Ctrl-C detaches from it rather than cancelling. `-d` starts the download and returns straight away. The worker scans when it finishes, so the model reaches the shelf whether anything is watching or not. It waits at most `pull.register_timeout_seconds` for that scan; a slower one is left to the next scan.

Pulling a model that is already being fetched joins that download instead of starting a second one, and pulling one that stopped part-way carries on from the bytes on disk.

The pulls under way are managed under the same verb:

```
hedos pull ls                  every pull, its state, progress, and what it is waiting for
hedos pull attach <job>        follow one again
hedos pull pause <job>         stop it, keeping what it has downloaded
hedos pull resume <job>|--all  start a stopped one again
hedos pull cancel <job>        stop it for good
hedos pull logs <job> [-n n]   its history
hedos pull clean [--keep n]    drop the records of ended pulls past the newest n (pull.keep_ended)
```

`pull pause` and `pull cancel` wait a few seconds for the worker and report what it did: `paused <job>` or `cancelled <job>` once it stopped, or, while it has not answered yet, `pausing <job>; its worker has not answered yet` (the ask stands). A stop that came too late says so and exits non-zero: `<job>: every byte landed before the pause was read; it is done`, or, for a pull that ended some other way first (a cancel that overtook the pause, say), `<job> ended <state>, so the pause had no effect`. A pull being registered refuses both, naming how many seconds the registration has left. Under `--json` both print the record with an `"outcome"` of `"honoured"`, `"pending"`, or `"too_late"`, matching the line and the exit code.

A cancel written just after the worker stopped reading (it was already honouring a pause) is settled by the command once that worker exits, so it reads `cancelled <job>` rather than being left for the next resume; the shelf's pulls screen does the same for `c x` pressed while a pause is being honoured. A pause asked while a cancel is still waiting to be read is refused and the cancel stands: a pause never turns "stop for good" into "stop for now". If another process holds the job's control file for more than 3 seconds, `pause`, `cancel`, `resume`, and the pulls screen's `c` and `R` give up, say so, and change nothing; opening the shelf skips such a pull and takes it up the next time.

A pull no worker ever took up (its worker died before it started) reads `interrupted` with the note `no worker`, in `pull ls`, on the shelf's pulls screen, and to `pull resume --all`, which starts every paused or interrupted pull. Under `--json` its `state` is `"interrupted"` and it carries `"abandoned": true`; the key is absent for every other pull. Once its model has been pulled by another job (one under way, or one that reached `done` after this pull was created, however old that job is), it reads `failed` instead, with the note `no worker took it up; the model was pulled again`, on every surface alike: `pull ls`, `--json` (`"state": "failed"`, `"superseded": true`), the pulls screen (which offers `x`, not `R`), `pull attach`, `pull resume` (refused), and `pull resume --all` and opening the shelf (skipped). Listing it changes nothing, but the first command that acts on it (`pull resume`, `pull resume --all`, `pull cancel`, `pull clean`, opening the shelf, or a resume or forget from the pulls screen) writes that `failed` into its record, so it stays failed once the other job is cleaned away or fails; from then on `--json` reads it as a plain failed pull carrying the same message, without `"superseded"`. `pull clean` collects it with the other ended pulls.

A pull whose record cannot be read (a damaged `status.json`, or one a newer hedos wrote in a state this build does not know) reads `unreadable`, with the reason as its note and `"state": "unreadable"` under `--json`. Nothing touches it: `pull pause`, `pull cancel`, and `pull resume` refuse it, `pull resume --all` and opening the shelf skip it, `pull clean` and the pulls screen's `x` leave it, and pulling the same model starts a new job beside it. Delete its directory under the pull store by hand once it is no longer wanted.

A job is named by its id, an unambiguous prefix of one, or its reference; a name several pulls answer to means the one still going. Since a bare word is a valid Ollama tag, a model named after a subcommand is written `hedos pull -- ls`.

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

### `hedos shelf`

Manage the shelf in a terminal UI: the same table `hedos ls` prints, with the machine's memory, what is loaded and by whom, disk per store, and the gateway's state kept on screen. Every key is a subcommand, and the footer shows only the ones that apply to the selected model.

- `p` pulls: a catalog grouped by what you'd use a model for, a search over Hugging Face as you type, and a plan (size, destination, fit) before a byte moves. Downloads run in a task strip with progress; `c` opens a stop card that offers to pause, keeping what landed, or cancel. A finished pull lands on the model it added.
- `P` opens the pulls screen in the shelf's place: every pull the store still holds, newest first, with the selected one's record, rate, estimate, and history beside it. `c`, `R`, and `Y` there stop, resume, and copy the id of the selected pull. `x` forgets an ended one's record, which is what `hedos pull clean` does to all of them; `p` starts a new pull as it does on the shelf; `esc` goes back.
- `B` opens the bench screen in the shelf's place: every chat model on the machine measured one at a time, with the selected row's runs, cold start, and where its timing came from beside it. `b` on the shelf benches the model under the cursor and opens the screen on it. Inside, `a` benches everything, `b` measures the selected row again, `c` stops the bench with what it has measured standing, and `esc` goes back to the shelf while the bench carries on. The figures are the ones `hedos bench` reports, from the same driver. A bench and a chat pane would each skew the other, so `t` is refused while one runs.
- `w` / `u` warm and unload, through the Ollama daemon when the daemon holds the model. `x` removes, showing exactly what leaves the disk and asking first.
- `t` opens a chat pane on the selected model, in place of the shelf: type, `enter` sends, the reply streams in with its token rate under it, and the conversation carries on until `esc` closes the pane (while a reply streams, `esc` and Ctrl-C stop it first; idle, Ctrl-C closes the pane too). The transcript scrolls with the wheel, `↑`/`↓`, `PageUp`/`PageDown` and `Home`/`End`; a scrolled view holds still while more text streams in, with its position in the title, and `End` follows the newest text again. The wheel also moves the shelf and the pull list. The model is warm afterwards, like after `hedos run`.
- `l` launches a coding harness on the selected model, `T` opens `hedos chat` on it in the plain terminal, and `S` runs `hedos serve`. Each of these is a hand-off: the UI steps aside, the command owns the terminal, and the shelf is back the moment it ends, with a row saying how it went. Ctrl-C reaches the harness or stops the reply; Ctrl-D ends a chat.
- Every text field (the chat prompt, the pull search, the filter) edits like a shell line: Ctrl-A / Ctrl-E jump to the ends, Ctrl-U clears back to the start, Ctrl-W or Option+Delete cuts the word before the cursor, the arrows and Option+arrows move by character and word. Cmd+Delete is a macOS binding the terminal keeps to itself; in iTerm, map it to send Ctrl-U (hex `0x15`) if you want it here.
- `/` filters, `o` sorts, `enter` expands the detail with the model's gateway activity, `y` copies the weights path and `Y` the id (through `pbcopy` where there is one, else by OSC 52, which tmux relays only with `set -g set-clipboard on`), `r` refreshes, `d` dismisses a failed row, `?` lists every key, `q` or Ctrl-C quits. The selection and the dismissed rows are remembered between runs.

A running `hedos serve` on the configured port is detected and its loaded models count as warm; warming through the UI then loads the model where it will be served. Needs a terminal.

### `hedos warm [model]`

Load a model into residency with a tiny request, so the next real request starts warm, and report whether it is resident afterwards.

A model is warm where it is served. When a gateway is running on the configured port (or on the one `--port` names), the model is loaded there rather than in this command's own process, which would exit and take the loaded model with it. `--port` also reaches a gateway started with `hedos serve --port`, which is not the one this command probes for. A model whose warm request is not a conversation, a speech model for instance, is loaded locally either way: the gateway's chat endpoint has no route for it.

The probe fits the model. A judge is asked the smallest well-formed typed question rather than being greeted, since prose is the one thing it refuses.

### `hedos unload [model]`

Evict a model from in-process residency and report the result. Omit the model to pick from the models that are currently warm.

### `hedos bench [model...]`

Measure what each model actually does on this machine: tokens a second, time to first token, and cold start, the same way for every row so two of them can be compared. Name models to bench only those, or omit them for every chat model that fits this machine's memory.

```sh
hedos bench                          # every model that fits
hedos bench qwen3 gemma3             # just these two, whatever their size
hedos bench --all --runs 5           # the too-big ones as well, five warm runs each
```

For each model in turn, hedos clears it from memory, runs it once cold, then runs it `--runs` times warm, and clears it again so the next one has the machine to itself. The first token of the cold run is the COLD figure; the warm runs' medians are the rest. A model a running `hedos serve` holds cannot be cleared from another process, so its cold cell reads `held` and its warm figures stand.

- `--all` benches the models too big for this machine's memory too.
- `--runs <n>` sets the warm runs per model (default 3).
- `--max-tokens <n>` caps each reply (default 128). A reply cut by the cap is still a whole measurement.
- `--prompt <text>` replaces the prompt every model answers.
- `--keep-warm` leaves residency alone: nothing is evicted, so nothing is measured cold.

Where a runtime reports the two phases apart, the rate is its own decode figure; where it does not, it is measured from the stream, first token to last. Token counts come from the runtime; where it reports none, they are counted from the text at roughly four characters a token and the figure wears a `~`. Time to first token is always measured here, since it is what a caller waits for. Thinking tokens count as generated text, because they cost the same time.

On a terminal the table redraws in place while the models run and settles into a ranked one, fastest first, which stays in your scrollback. Piped, nothing is printed until the end and the same table arrives as plain text. `--json` carries every figure, each run behind it, and where its timing came from. Ctrl-C stops the bench; what was measured stands. The command exits non-zero when no model produced a figure.

### `hedos stats`

Read the gateway's audit log back and report usage: the total request count, the rejection rate, and per model the request count, the error rate, and p50/p90/p99 serving latency. Prints a table, or the full summary under `--json`. With no audit log yet (nothing has been served), it says so and exits `0`.

## Exit codes

A command exits `0` on success. On failure it writes the error to stderr and exits non-zero.
