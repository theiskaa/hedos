# Models

hedos treats the models on your machine as one shelf, no matter where they came from or what they do. This page covers how it finds them, how each one is served, how to install new ones and remove old ones, and the one rule that holds through all of it: hedos never moves, copies, or rewrites your weights. It reads them where they are, and deletes them only when you ask it to with `hedos rm`.

## Where models are found

`hedos scan` looks in the places models actually live:

| Store | Where | What it reads |
| --- | --- | --- |
| Ollama | `$OLLAMA_MODELS`, else `~/.ollama/models` | Each tag's manifest and the weight, template, projector, and parameter blobs it names. |
| Hugging Face hub cache | `$HF_HUB_CACHE`, else `$HF_HOME/hub`, else `~/.cache/huggingface/hub`, plus each root in `hf_cache_roots` | Each `models--<org>--<repo>` directory: its blobs, its current snapshot, and its refs. |
| LM Studio | `~/.lmstudio/models` and `~/.cache/lm-studio/models` | GGUF files (projectors aside), labelled by their `<publisher>/<model>` folder. |
| Loose files | `~/Downloads`, `~/Models`, plus each folder in `watched_folders`, and up to two folders down | GGUF files (shards grouped into one model), a folder holding `config.json` and `.safetensors` files as one model, and a GGML `.bin` as a whisper model. |
| Built in | the operating system | Apple Intelligence, on a Mac where it is enabled and ready. |

The machine has exactly one Hugging Face hub cache, the one the hub's own tools use: `HF_HUB_CACHE` or `HF_HOME` says where it is, replacing the default rather than adding to it. A cache elsewhere that should also be swept goes in the `hf_cache_roots` setting (hedos uses its `hub` or `huggingface/hub` subfolder when one exists), so what a pull writes and what a scan reads never come apart. Both settings live in `[models]`; see [configuration.md](configuration.md).

For each model it finds, the scan reads the format, works out the modality and capabilities, and reconciles the result into the registry. A model that moved is migrated to its new record, a model whose weights vanished is marked missing, and duplicates are noted. Nothing is copied or relocated. The registry records point at the files where they already are, so every other tool still sees the same models.

A model on the shelf is in one of three states: **ready** (resolved to a runtime, weights on disk), **unresolved** (no runtime here can serve it), or **missing** (its weights are gone). Only ready models are served.

## How a runtime is resolved

Discovery also resolves each model to a runtime, so the shelf tells you not just what you have but how each model will actually be served.

1. **Identify.** hedos reads the model's header or config: its format, modality, capabilities, context length, chat template, senses, and tool-calling dialect.
2. **Bid.** Every runtime wired into this build looks at that identification and bids on the models it can serve.
3. **Rank.** The lowest bid wins, ties broken by runtime id. The runners-up are kept on the record as alternatives.

The bids, from most to least preferred:

| Bid | Runtimes |
| --- | --- |
| 10 | llama.cpp, whisper.cpp, OpenAI-compatible endpoint |
| 14 | mlx-vlm |
| 15 | Apple Intelligence |
| 20 | Ollama |
| 25, 26 | mflux, then diffusers |
| 27, 28 | ComfyUI, then AUTOMATIC1111 |
| 30 | mlx-audio |
| 32 | the embeddings sidecar |
| 40 | mlx-lm |
| 100 | manifest runtimes |

In practice few runtimes bid on the same model, since each bids only on its own formats: a GGUF goes to llama.cpp, a model in the Ollama store to Ollama, an MLX checkpoint to mlx-lm or mlx-vlm. A model no runtime bids on stays unresolved, and a model whose weights are gone is not re-resolved.

## Runtimes at a glance

A model resolves to whichever of these fits it, and each serves only when its backend is present:

| Runtime (id) | Serves | Needs |
| --- | --- | --- |
| llama.cpp (`llama-cpp`) | GGUF chat models, embedders, and decision models, with sight when a projector is present | `llama-server` on `PATH`; llama.cpp 0.6.0 or newer for decision models |
| Ollama (`ollama`) | Models the Ollama daemon manages | The Ollama daemon running |
| OpenAI-compatible endpoint (`generic:openai-server`) | Remote models reached by URL and key | A reachable server, with its API key (if it needs one) in `HEDOS_OPENAI_API_KEY` |
| mlx-lm (`python:mlx-lm`) | MLX text models | [`uv`](https://astral.sh/uv) |
| mlx-vlm (`python:mlx-vlm`) | MLX vision-language models | `uv` |
| mlx-audio (`python:mlx-audio`) | Speech synthesis | `uv` |
| embeddings (`python:embeddings`) | Safetensors embedders | `uv` |
| diffusers (`python:diffusers`) | Diffusers image pipelines | `uv` |
| mflux (`python:mflux`) | The FLUX pipelines it supports, ahead of diffusers | `uv` |
| whisper.cpp (`whisper-cpp`) | Transcription, from GGUF or GGML `.bin` weights | `uv` |
| ComfyUI (`comfyui`), AUTOMATIC1111 (`a1111`) | Image models the running daemon serves | The daemon running |
| Apple Intelligence (`apple-foundation`) | Apple's on-device model, with tool calls | A Mac where the model is enabled and ready, plus the bridge library (below) |
| Manifest runtimes (`python:laya`, `python:zerank`, or your own) | Whatever the manifest declares | Approval with `hedos runtimes approve <id>`, and `uv` |

The Python sidecars provision their own environment through `uv` on first use; their runtime code ships inside the binary. A manifest runtime runs code on the host, so it neither bids on a model nor serves one until you approve it, and the approval is bound to a hash of its files: editing any of them asks for it again. hedos ships `python:laya` and `python:zerank` this way, and reads your own from `runtimes.d` in the data directory. `hedos runtimes` lists them and where each approval stands.

### GGUF models on llama.cpp

`llama-server` runs as a subprocess for `.gguf` files: chat models, the models that only embed, and the decision models.

- **Embedders** answer on `/v1/embeddings` and `/api/embed` from a server started for embeddings. A GGUF embeds when its header pools its output to one vector (mean, cls, or last), whatever its architecture: the encoders (BERT, nomic-bert, jina, and similar) and the decoders converted to embed (Qwen3-Embedding) alike, and such a model never chats. One whose header ranks (a reranker such as Qwen3-Reranker), and an encoder that names no pooling (a diffusion pipeline's text encoder, or a reranker converted without its pooling), are not embedders and stay unresolved. See [gateway.md](gateway.md#embeddings) for how batches and windows are served.
- **Decision models** are the GGUFs whose header names a decision type (`{arch}.decision.type`): clef and clef-flash, OpenJev, Kev, lev, Laya and Julia-1, as ggml-org publishes them. Each is a judge, whatever its architecture or chat template says, and answers typed questions on `/v1/systemone` and with `hedos run`, never chat. They need llama.cpp 0.6.0 or newer; an older `llama-server` fails with a message saying so. A decision server holds its declared window up to 16384 tokens.
- **Sight.** A GGUF sees when a multimodal projector (an `mmproj` file) sits beside it, or anywhere in its snapshot, that encodes images and outputs the model's own embedding width, as llama.cpp checks it by its tensors. A projector of another width (another model's, kept in the same folder) and one that only encodes audio do not count. Its server is launched with the projector, taking the one named for the weights' quantization when there are several and the smallest otherwise, and never one cut short or unreadable, as an interrupted download leaves it. Among decision models, clef and OpenJev read images; the others read text only, projector or not.

### Apple Intelligence

Apple's on-device model is served through a Swift bridge on Macs where the model is enabled and ready. It appears on the shelf as a built-in model and serves tool calls like any other. On Linux, or on a Mac without it, it simply never shows up.

> **The Apple bridge is a separate library, and installers do not carry it.** Unlike every other runtime, Apple Intelligence needs a companion dynamic library, `libhedos_apple_shim.dylib`. The build script compiles it during a source build whenever the building SDK carries the `FoundationModels` framework (a recent Xcode on macOS 26+). Installs that copy only the binary, such as `cargo install`, Homebrew, and the prebuilt release archives, leave the library behind, so a model that a previous scan recorded still lists on the shelf, but serving it fails with *"Apple's model needs the Apple Intelligence bridge, which is not built into this binary."*
>
> hedos looks for the library at `HEDOS_APPLE_SHIM` when that is set, then next to the running binary, then at the path the build wrote it to (which only exists on the machine that built the binary). To enable it after a binary-only install, build the library from source and place it beside the binary:
>
> ```sh
> cargo build --release
> cp "$(ls -t target/release/build/hedos-runtime-*/out/libhedos_apple_shim.dylib | head -1)" \
>    "$(dirname "$(command -v hedos)")/"
> ```
>
> The library and the binary must come from compatible builds (the bridge carries an ABI version the binary checks); a mismatched or absent library is skipped, and Apple Intelligence reports itself unavailable rather than crashing.

### MLX-Swift

The MLX-Swift runtime from the original macOS build is framework-bound and is not part of this headless port; its models are served by the MLX sidecars instead. A model that would need it still appears on the shelf, but hedos will tell you it cannot serve it here rather than dropping it.

## Capabilities

Each model declares what it can be asked to do. `hedos ls` shows them, and `hedos ls --capability <name>` lists only the models that have one. The gateway and the CLI refuse a request a model's capabilities do not cover, rather than guessing.

| Capability | Meaning | Where it is used |
| --- | --- | --- |
| `chat` | Holds a conversation | `/v1/chat/completions`, `/api/chat`, `/v1/messages`; `hedos chat`, `hedos run` |
| `complete` | Continues a prompt | `/v1/completions`, `/api/generate` |
| `tools` | Calls tools | Tools on the chat routes; every harness but aider needs it |
| `see` | Reads images | Image parts in chat; `hedos run --image` |
| `embed` | Turns text into vectors | `/v1/embeddings`, `/api/embed`, `/api/embeddings` |
| `judge` | Answers typed questions | `/v1/systemone`; `hedos run` |
| `image` | Generates images | `/v1/images/generations`; `hedos image` |
| `speak` | Synthesizes speech | `/v1/audio/speech`; `hedos speak` |
| `transcribe` | Turns speech into text | `/v1/audio/transcriptions`; `hedos transcribe` |

## Fit and memory

The FIT column of `hedos ls` says how a model will sit in this machine's memory. hedos estimates what the model needs as its serving size plus a quarter (working memory beyond the raw weights), then compares that with the machine's total RAM:

| Verdict | `hedos ls` shows | Estimated need |
| --- | --- | --- |
| Runs well | `fits` | under 75% of RAM |
| Tight fit | `tight` | 75% to 95% of RAM |
| Too large | `too big` | 95% of RAM or more |
| Unknown | a dash | no size to judge |

A model whose weights are gone reads `gone` instead. `hedos ls --json` carries the verdict as `fit`: `runs_well`, `tight_fit`, `too_large`, or `null`.

The serving size is what serving the model loads, which is not always what it takes on disk: a Hugging Face repo that holds several quantizations, or blobs from older revisions, serves one weight set (with its projector and config). The same verdict drives the "too big" count of `hedos scan` and the recommendations `hedos pull` offers.

While serving, a memory governor decides which models stay loaded. Two settings in `[models]` shape it: `keep_warm` (how long an idle model stays loaded) and `eviction` (`strict-single` keeps one heavy model resident, `budgeted` keeps as many as fit `ram_budget_mb`). See [configuration.md](configuration.md).

## Installing with `hedos pull`

`hedos pull <reference>` resolves a reference and plans the install before anything downloads.

### References

| You type | It is |
| --- | --- |
| `gemma3:4b`, `qwen2.5` | An Ollama tag. A bare name gets `:latest`. |
| `ollama.com/library/gemma3`, `registry.ollama.ai/...` | An Ollama link. |
| `Qwen/Qwen2.5-0.5B-Instruct-GGUF` | A Hugging Face repo (`org/model`). |
| `https://huggingface.co/org/model`, `hf.co/org/model/tree/main` | A Hugging Face link. |

hedos infers the provider from the shape: an `org/model` with no `:tag` is a Hugging Face repo, and anything else that reads as a tag is Ollama. `--from ollama` or `--from hf` forces it. Since a bare word is a valid Ollama tag, a model named after a `pull` subcommand is written `hedos pull -- ls`.

Run `hedos pull` with no reference in a terminal to search Hugging Face by keyword, or leave the search blank to pick from a short list of models that fit your machine's RAM.

### The plan

Before a byte moves, hedos resolves the plan (the name, the destination, and the size) and, in a terminal, asks you to confirm it; from a script it starts without asking. Pulling a model that is already being fetched joins that download instead of starting a second one, and pulling one that stopped part-way carries on from the bytes on disk.

The download itself runs in a worker process of its own, so closing the terminal does not stop it. `hedos pull ls` shows what is running, `hedos pull pause` and `resume` stop and restart a pull without losing the bytes already fetched, and `-d` starts a download without following it. The worker scans when it finishes, so the model reaches the shelf whether anything is watching or not. The full set of `pull` subcommands is in [cli.md](cli.md#hedos-pull).

### Where it lands

Installs write into each platform's native layout:

- **Ollama** models pull through the daemon's own API. If Ollama is installed but not running, hedos starts the daemon first.
- **Hugging Face** models download into the standard hub cache: content-addressed blobs, a snapshot directory of symlinks, and a ref pointing at the revision. Downloads resume with HTTP `Range`, and each file stored in LFS is verified with SHA-256 against the hash its listing names.

hedos picks which files of a Hugging Face repo to fetch. For GGUF it takes one complete quantization, preferring Q4_K_M, then Q4_0, Q5_K_M, Q6_K, Q8_0, and F16 (else the smallest), along with any `mmproj` projector and small companion files. Otherwise it takes the safetensors (or PyTorch) weight set and the config and tokenizer files beside it. Documentation, images, and exports nothing here runs (ONNX, OpenVINO, Core ML, Flax, TensorFlow) are skipped.

hedos owns no weights directory of its own, so the moment an install finishes, every other tool sees the model too. Installs do not touch the registry directly; the scan that follows discovers the result.

### Gated repositories

A gated Hugging Face repository needs a token with access to it. hedos reads `HF_TOKEN` (or `HUGGING_FACE_HUB_TOKEN`) from the environment, else the token file `huggingface-cli login` writes (`$HF_TOKEN_PATH`, else `$HF_HOME/token`). You also have to accept the model's terms on its Hugging Face page. Without access, the pull stops at the plan, before anything downloads, with a message saying what to do. A download that would not fit on the disk is refused too, naming the size it needs and the space available.

## Removing

Removal is symmetric with install. `hedos rm <model>` first shows a deletion preview: how many items would go and the estimated size. In a terminal it then asks for a yes/no confirmation (the default is no) and removes nothing unless you agree; in a script or pipe it removes nothing unless you pass `-y`.

- File-backed models are deleted from disk, permanently (they do not go to the trash).
- Ollama models delete through the daemon, which hedos starts first if Ollama is installed but not running.

The preview is honest about what remains. If duplicate copies of the same weights exist elsewhere on the machine, removing one does not remove the others, and the shelf will still show them. `hedos scan` lists the copies that can go because another model keeps everything they hold, with what removing each one frees. The exact rules for what counts as a copy, and how shared files, hard links, and symlinks are counted, are under `hedos scan` in [cli.md](cli.md#when-a-model-is-offered-for-removal).

## Warm and unload

`hedos warm <model>` loads a model with a tiny request, so the next real request starts warm, and reports whether it is resident afterwards. A model is warm where it is served: when a gateway is running on the configured port (or the one `--port` names), the model is loaded there rather than in the command's own process, which would exit and take the loaded model with it. The probe fits the model, so a judge is asked the smallest well-formed typed question rather than greeted.

`hedos unload <model>` evicts a model from residency and reports the result, asking the Ollama daemon to unload it too when the daemon holds it. Omit the model to pick from the ones currently warm. An idle model is also unloaded on its own once `keep_warm` runs out.

## Judges

A judge is a model that answers typed questions about a state rather than chatting: given a state and questions of three types (`choice`, `score`, and `noul`, a yes-or-no), it returns probabilities. These are TypeSafe's System One questions. Judges declare the `judge` capability (`hedos ls --capability judge`), and come in two kinds:

- **Decision GGUFs**, served by llama.cpp 0.6.0 or newer: clef and clef-flash, OpenJev, Kev, lev, Laya, and Julia-1. Each reads its whole question in one batch of its window (its declared context, capped at 16384 tokens). clef and OpenJev read images when a projector sits beside them.
- **Manifest runtimes**: `python:laya`, an encoder with a decision head that answers in one forward pass, detected by its `rl_agent_config.json`; and `python:zerank`, a reranker that scores each option as a document against the question, detected by its chat template. Their probabilities differ in kind: zerank's are relevance, not laya's calibrated belief. Both run only once approved (`hedos runtimes approve python:laya`).

Ask a judge with `hedos run <judge>`, passing the question as JSON (`{"state": ..., "questions": {...}}`) or composing it in a terminal; on the shelf's [try screen](shelf.md#judges-on-the-try-screen); or over the gateway's `/v1/systemone`, which is what a TypeSafe SDK calls. [gateway.md](gateway.md#judges) has the request and answer format.
