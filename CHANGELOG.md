# Changelog

All notable changes to hedos are documented here. Each release section below is what ships as the GitHub Release notes.

## v1.3.1 - 2026-08-29

A craft pass over `hedos shelf`. Nothing the screen does has changed; how well it does it has.

- The help is generated from the keymap and grouped by what each key acts on, so a key cannot exist without its help line and nothing collides.
- The footer sheds keys one at a time, keeps `? help  q quit` at every width, and adds `enter expand` and `o sort`.
- The pull card carries each recommendation's blurb, spaces its categories, hints what to type, and names the model in its preview.
- Modals sit on a darkened screen with dim borders; orange is reserved for focus and motion, red for what failed.
- Task rows show `c cancel` and `d dismiss` only on the row the key acts on, and a running download is never pushed off the strip.
- Long values wrap or elide instead of running past their pane; the header and detail no longer repeat the machine block; the stacked layout gains a `gateway` line.
- A model whose weights are gone reads as `gone` and can be removed or pulled again; an abandoned partial download is removable.

## v1.3.0 - 2026-08-27

The shelf as a screen. `hedos shelf` opens a TUI over everything the command line already does, and keeps on screen what a command line cannot: what is loaded and by whom, how much memory is left, what the gateway served today. The rest of the release hardens what the screen exposed.

- `hedos shelf`: every model with its runtime, store, and size; the selected one's fit, residency, capabilities, and gateway activity; the machine's memory with a bar per loaded model, disk per store, and the gateway's state. Every key is a subcommand (`p` pulls, `w` and `u` warm and unload, `x` removes with a preview, `S` serves) and the footer shows only the keys that apply to the model under the cursor. Pulls download in a task strip while you keep working, with `c` to cancel. It works over ssh and inside tmux, and it remembers the selection between runs.
- A chat pane inside the shelf. `t` turns the shelf into a conversation with the selected model: the reply streams in, keeps its markdown (bold, headings, code blocks left alone), scrolls with the wheel, the arrows, and the page keys, and holds still while more text arrives. `esc` stops a reply, then closes the pane; the model stays warm for whatever comes next.
- Hand-offs. `l` launches a coding harness on the selected model, `T` opens `hedos chat`, `S` runs `hedos serve`: the screen steps aside for anything that needs the terminal and is back the moment it ends, with a row in the task strip saying how it went.
- Line editing everywhere. Every text field in the shelf (the chat prompt, the pull search, the filter) edits like a shell line: Ctrl-A and Ctrl-E, Ctrl-U, Ctrl-W and Option+Delete, the arrows by character and by word, with the cursor kept on screen. Text is measured the way the terminal draws it, so emoji, wide scripts, and combining marks neither overflow nor split.
- Models the Ollama daemon holds now show as warm and unload from hedos. `hedos warm`, `hedos unload`, and `hedos ls` see what the daemon has loaded, and unloading through hedos evicts it there; a daemon too busy to answer is reported instead of being read as "not loaded", so an unload never claims success it cannot see and a removal never deletes a model the daemon still holds.
- Downloads survive a dropped connection. A Hugging Face stream that breaks mid-file is reopened from the bytes already on disk, up to five times with a growing pause, before a pull is failed; the checksum still covers the whole file. Ollama plans carry their `:latest` tag, so a pulled model is recognised as installed.
- Ctrl-C reaches what you meant. A Ctrl-C typed at hedos, or at `hedos chat` and `hedos serve`, no longer reaches the llama-server pool, the Python sidecars, or an `ollama serve` hedos started; they run in their own process groups. The shelf quits on Ctrl-C from every modal and hands the terminal back the way it found it, even after a harness that died in raw mode.
- A running gateway reports the models it holds on `/api/ps`, with when each expires, and the gateway stats record when a model was last seen.

## v1.2.1 - 2026-08-02

A fix for model removal. `hedos rm` reported a model deleted but `hedos ls` kept showing it.

- Removing a model now drops its record from the shelf, not just its files. `rm` deleted the weights (or the Ollama tag) but never removed the record from the registry that `ls` reads, so the model lingered — and a rescan only re-flagged it as missing rather than dropping it. A removed model now leaves the shelf immediately and stays gone.

## v1.2.0 - 2026-07-24

A native runtime for Apple's on-device model (#4). hedos now serves Apple Intelligence directly through the `FoundationModels` framework, so the model built into the machine appears on the shelf and answers chat and completion requests over the gateway with no download and no sidecar.

- The runtime bridges Rust to the framework through a small Swift shim, streaming the model's output back over the gateway in every dialect. When Apple Intelligence is turned off or still downloading, the shelf says so instead of silently omitting the model.
- Tool calling works end to end. Apple's model executes tools during generation, so the runtime captures each call as a structured `tool_call`, ends the turn, and replays the result on the next request as tool-call and tool-output history. Tool parameter schemas port into the framework's dynamic schema — strings, numbers, booleans, enums, arrays, and nested objects with required and optional fields — and a tool whose schema the framework cannot express is dropped from the offer rather than failing the request.
- Apple's on-device model has a fixed 4096-token context window. hedos records that window honestly on the shelf and in `/v1/models`, and warns at pick time when a model's window is too tight for the request ahead of it, instead of letting the run fail deep in the model with a context error.

## v1.1.1 - 2026-07-22

Tool calling now reaches the models served by the Python sidecars (#5). MLX builds from the Hugging Face cache — Llama and Qwen instruct models among them — can drive the coding harnesses and serve tool requests over the gateway, closing the gap where capable weights sat out of `hedos launch` because their runtime never forwarded tools.

- The mlx-lm sidecar renders a request's tools through the model's own chat template, so each model family sees tools in the exact format it was trained on. A template with no tool support gets a generic system-prompt description of the tools instead, and a template that rejects a system role gets it folded into the first user turn — the request degrades gracefully rather than failing.
- Tool calls are parsed back out of the model's reply in each family's own format — Qwen/Hermes `<tool_call>` blocks, Mistral `[TOOL_CALLS]`, Llama's python tag and bare JSON — and served over the gateway as structured calls in every dialect. A reply that contains no call streams as plain text, exactly as before.
- mlx-vlm gets the same treatment, so tool-driving harnesses can also seat vision models.
- Models served by these runtimes now carry the `tools` capability on the shelf and appear in the `hedos launch` picker; an existing shelf picks this up automatically on the next command, no rescan needed.

## v1.1.0 - 2026-07-21

This release turns hedos into a working seat for coding agents and rounds out the command surface. The gateway speaks a third wire format and enforces tool calling per model, `hedos launch` runs a coding harness against a local model with nothing to configure, and four new capabilities land on the command line. The default posture is unchanged: the gateway still binds loopback and trusts every local caller.

- Coding harnesses in one command. `hedos launch` runs Claude Code, OpenCode, Aider, Goose, or Crush against a model on your shelf. The gateway starts inside the same process on a free port, the harness is wired to it, and both stop together; your own harness config is never touched, so running the tool directly afterwards behaves exactly as before. Name the harness and model, or pick each interactively.
- The Anthropic messages dialect. Alongside OpenAI (`/v1`) and Ollama (`/api`), the gateway now serves Anthropic's `/v1/messages`, so Claude Code and anything built on the Anthropic SDK can point at hedos and reach every model you own.
- Tool calling, end to end. hedos reads each model's chat template to decide whether it can call tools, shows that as a capability on the shelf, and the gateway advertises and enforces it: a request that asks a model to use tools is served when the model can and refused with a clear reason when it cannot, instead of failing deep in a runtime.
- `hedos transcribe` turns an audio file into text through a local whisper model — the inverse of `speak`, and the last capability that lacked a first-class command. Point it at a WAV and it streams the transcript back; `--language` and `--translate` map to what the runtime honors.
- `hedos run --image` feeds a local image to a vision (`see`) model, so you can ask a model about a picture from the shell. The picker offers only vision-capable models, and naming a model that cannot see says so up front instead of silently answering blind.
- A fit verdict on the shelf. `hedos ls` now has a FIT column that says whether each model will actually run in this machine's memory — fits, tight, too big, or unknown — using the same assessment the install catalog uses, and `--json` carries it too.
- `hedos stats` reads the gateway's audit log back and reports per-model request counts, the rejection rate, and p50/p90/p99 latency, as a table or under `--json`.
- The Ollama daemon starts itself. A cold connection to Ollama now starts `ollama serve` and waits for it, so a served request or a pull no longer fails just because the daemon was not already running.
- Gated Hugging Face repositories authenticate. hedos reads `HF_TOKEN`, `HF_TOKEN_PATH`, or the token that `huggingface-cli login` writes when it plans and downloads a gated model — the install path used to ignore it and refuse every gated repo. When access is still denied, the error points at the model's terms page instead of only telling you to add a token.
- The interactive `hedos pull` picker is navigable. A blank search shows the models that fit this machine's memory, any query searches Hugging Face, and a "search again" row moves between the two, so switching from recommendations to a search, or trying another query, no longer means re-running the command. No matches or an all-installed recommendation list returns to the prompt instead of ending the command.
- Hardening. Approved runtimes receive an allowlisted environment rather than the full ambient one; a Hugging Face revision that is not a safe path component is rejected before it reaches the filesystem; registry and settings writes serialize through an advisory file lock so two hedos processes cannot clobber each other; the audit log and generated launch configs are written owner-only; and an install request's headers are redacted when it is formatted, so an access token cannot leak into a log or an error string. An off-by-default token-authentication prototype is present but inert — multi-client access remains a later, opt-in decision.
- Faster and lighter. The shelf is served from a shared cached snapshot instead of being cloned per request, one identification cache is reused across resolution passes, provisioned Python bundles are stamped so they are not rechecked every command, and the bundles themselves ship as a single compressed archive that unpacks on first use, shrinking the binary.

## v1.0.0 - 2026-07-20

hedos is now a terminal program. The macOS app is gone, replaced by a single `hedos` binary that does everything headlessly: it finds the models already on your machine, installs new ones, serves them over a local HTTP gateway, and runs chat, speech, and image generation straight from the shell. It works over SSH, in scripts, and on Linux. The Swift app remains on the `macos-app` branch at its final v0.1.4 release and is no longer developed.

- Discovery of every local model, unchanged in spirit from the app. hedos scans the Ollama store, the Hugging Face hub cache, LM Studio's library, and loose GGUF and safetensors files in your folders, then reconciles them into one shelf and resolves each to the runtime that actually fits it. Weights are never moved, copied, or re-downloaded; the records point at where the files already sit, so every other tool on the machine still sees the same models.
- The `hedos` command line covers the whole surface: `scan` and `ls` to see the shelf, `run` and `chat` to talk to a model, `speak` and `image` to generate media, `pull` and `rm` to install and remove, `warm` and `unload` to control residency, and `serve` to start the gateway. Every command takes `--json` for machine-readable output, and every command works non-interactively so scripts behave predictably.
- The commands are interactive when you are. Leave off the model and you get a fuzzy-filterable picker showing the same columns as `ls`, scoped to what the command needs, with models that resolved a runtime listed first so the servable one is the default. Leave off the prompt or the text and hedos asks for it. `speak` offers a voice picker when a model ships several. Run `pull` with no reference and you can search Hugging Face by keyword or pick from a short list of models that fit your machine's memory. In a pipe, a script, or under `--json`, none of this triggers: a missing argument is a plain error instead, so nothing ever blocks waiting on input.
- A loopback gateway speaking both the OpenAI (`/v1`) and Ollama (`/api`) wire formats, so editors, agents, and scripts that already talk to either can point at hedos and reach every model on the shelf. It serves each model's real context length, chat template, and tool-calling dialect rather than a lowest common denominator, and tells you plainly when a model cannot do what a request asks. Authentication is open on loopback, which makes the loopback boundary the security boundary.
- Runtimes for local GGUF through a `llama-server` subprocess, the Ollama daemon, any OpenAI-compatible endpoint, the Python sidecars (mlx-lm, mlx-vlm, speech, embeddings, diffusers, mflux) and whisper, plus the ComfyUI and AUTOMATIC1111 image daemons. The Python runtime bundles ship inside the binary and unpack themselves on first use, so speech and image generation work from a fresh install without hunting for scripts. A memory governor coordinates residency and admission across every engine so two heavy loads cannot oversubscribe the machine.
- Installing and removing models from the terminal. `pull` resolves a Hugging Face repo, an Ollama tag, or a link to either, plans the install before a byte moves, and shows the size and destination for confirmation. Hugging Face downloads land in the standard hub cache with resume and per-file checksum verification; Ollama models pull through the daemon. `rm` previews exactly what would go and asks before deleting, and refuses to delete non-interactively without `-y`.
- One settings file at `~/.config/hedos.toml` and one state directory under `~/.local/share/hedos` for the registry, generated artifacts, job history, and the gateway's audit log. Neither holds your weights. Both follow the XDG variables when set.
- The Apple Foundation and MLX-Swift runtimes from the macOS build are framework-bound and are intentionally not part of this port. A model that would need one still appears on the shelf, and hedos says it cannot serve it here rather than dropping it.
