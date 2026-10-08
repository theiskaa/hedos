# Configuration

hedos reads one settings file and keeps its state in one data directory. Both follow the XDG conventions, and both can be moved with environment variables. Neither holds your model weights: hedos only records where they already sit.

| What | Where |
|---|---|
| Settings | `~/.config/hedos.toml`, or `$XDG_CONFIG_HOME/hedos.toml` |
| State | `~/.local/share/hedos`, or `$XDG_DATA_HOME/hedos` |

## The settings file

The file is optional and hand-editable. hedos never needs it to exist: anything you leave out uses its default.

Loading is tolerant. A missing file or a file that is not valid TOML falls back to the defaults. A table that fails to decode (a wrong type, or a value hedos does not know) falls back to its defaults on its own, so a typo in `[pull]` never resets `[models]`. One thing to know: if hedos later saves the file (`hedos runtimes approve` does), it writes the defaults it fell back to, which drops the other keys of that broken table.

Writes go through a temporary file and a rename, guarded by a short-held lock on a `hedos.toml.lock` file next to it, so the shelf, the CLI and a running gateway never clobber each other's changes.

Settings are read when a command starts, so a running `hedos serve` or `hedos shelf` keeps the values it started with. Restart it after an edit.

### Settings

#### `[models]`

| Key | Type | Default | Meaning |
|---|---|---|---|
| `watched_folders` | array of strings | `[]` | Extra folders scanned for loose GGUF and safetensors files, beyond `~/Downloads` and `~/Models`. Use absolute paths: a leading `~` is not expanded here. |
| `hf_cache_roots` | array of strings | `[]` | Extra Hugging Face hub caches to scan, beyond the machine's own. A leading `~` is expanded. For each root, hedos looks at `<root>/hub`, then `<root>/huggingface/hub`, then the root itself. |
| `keep_warm` | string | `"five_minutes"` | How long an idle model stays loaded: `"five_minutes"`, `"fifteen_minutes"`, `"one_hour"`, or `"never"` (unload as soon as it is idle). |
| `eviction` | string | `"strict_single"` | How the memory governor makes room. `"strict_single"` keeps at most one heavy model (1 GiB or more) resident at a time. `"budgeted"` evicts the oldest resident models until the new one fits the RAM budget. |
| `ram_budget_mb` | integer | `0` | The RAM budget in MiB for the `"budgeted"` policy. `0` means unset, and the budget is then 80% of the machine's memory. |
| `approved_host_runtimes` | array of strings | `[]` | The manifest runtimes you approved to run on this machine. Managed by `hedos runtimes approve` and `revoke`; see [Manifest runtimes and consent](#manifest-runtimes-and-consent). |
| `approved_host_runtime_hashes` | table | `{}` | The content hash of each approved runtime, as it was when you approved it. Managed by the same commands. |

#### `[chat]`

| Key | Type | Default | Meaning |
|---|---|---|---|
| `default_system_prompt` | string | `""` | A system prompt applied when neither the request nor the model's record sets one. Empty means none. |

#### `[gateway]`

| Key | Type | Default | Meaning |
|---|---|---|---|
| `port` | integer | `43367` | The loopback port `hedos serve` binds. `hedos serve -p <port>` overrides it for one run. The shelf and `hedos warm` also look for a running gateway on this port. |
| `max_concurrent_inference` | integer | `4` | How many inference requests the gateway serves at once (at least 1). `hedos launch` uses the same cap for its private gateway. |

The gateway always binds `127.0.0.1`.

#### `[pull]`

| Key | Type | Default | Meaning |
|---|---|---|---|
| `max_concurrent` | integer | `2` | How many pulls transfer at once (1 to 64). The rest queue for a free slot and show `waiting for a free slot` in `hedos pull ls`. The cap holds across terminals. |
| `auto_resume` | boolean | `true` | Whether a pull whose worker died (a closed laptop, a killed terminal) starts again when the shelf next opens. A pull you paused stays paused until you resume it or pull the model again, whatever this says. |
| `partial_age_hours` | integer | `24` | How long, in hours, a half-downloaded file left behind by another pull is kept before an install of the same repo tidies it (1 hour to 1 year). A paused pull's own bytes are kept for as long as it is paused. |
| `retry_window_minutes` | integer | `120` | How long, in minutes, a failing transfer keeps retrying before it is left interrupted for you to resume (1 to 1440). |
| `register_timeout_seconds` | integer | `120` | How long, in seconds, a pull whose bytes have all landed waits for the scan that puts the model on the shelf (5 to 3600). Past it the pull settles as done and the next scan picks the model up. |
| `keep_ended` | integer | `20` | How many ended pulls keep their record for `hedos pull ls` and the pulls screen (0 to 10000). Older ones are dropped when the shelf opens or a pull starts. `hedos pull clean --keep <n>` overrides it for one run. `0` keeps none. |

Values outside a range are clamped to it.

#### `[advanced]`

| Key | Type | Default | Meaning |
|---|---|---|---|
| `job_history_limit` | integer | `50` | How many finished jobs (image generation and the like) to keep in history. |

#### Keys hedos writes but does not read

When hedos saves the file it writes every key it knows, so a saved file also carries `[models] approved_network_runtimes` and `approved_network_runtime_hashes`, `[chat] default_model_id`, `show_stats`, `send_with_enter` and `default_bench`, a `[voice]` table (`default_voice`, `speed`, `auto_speak`), and `[gateway] enabled`, `host` and `max_connections`. The current build does not act on any of them. In particular, changing `host` does not move the gateway off `127.0.0.1`.

### An example `hedos.toml`

```toml
[models]
# Folders to scan for loose GGUF and safetensors files. Absolute paths.
watched_folders = ["/Volumes/models/gguf"]
# Another Hugging Face cache to sweep, besides the machine's own.
hf_cache_roots = ["~/work/hf-cache"]
# Keep a model loaded for an hour after its last request.
keep_warm = "one_hour"
# Let several models stay resident, up to 24 GiB between them.
eviction = "budgeted"
ram_budget_mb = 24576

[chat]
default_system_prompt = "Answer briefly."

[gateway]
port = 43367
max_concurrent_inference = 4

[pull]
max_concurrent = 2
auto_resume = true
partial_age_hours = 24
retry_window_minutes = 120
register_timeout_seconds = 120
keep_ended = 20

[advanced]
job_history_limit = 50
```

Every value here is optional. A file with only `[models] keep_warm = "one_hour"` is complete.

## The data directory

State lives under `~/.local/share/hedos`, or `$XDG_DATA_HOME/hedos` when that variable is set to an absolute path. If neither `XDG_DATA_HOME` nor `HOME` is set, hedos falls back to a `hedos-data` directory in the current directory. hedos creates what it needs on first use.

| Path | What it holds |
|---|---|
| `registry/` | The model registry (`models.json`): one record per model, pointing at where its weights sit. |
| `artifacts/` | Generated outputs (speech, images) and their provenance. |
| `history/` | The job history, capped by `advanced.job_history_limit`. |
| `pulls/` | One directory per pull: what it fetches, where it stands, and its history. The pull workers, the CLI and the shelf coordinate through it. |
| `gateway/` | The gateway's audit log, `audit.jsonl`, one JSON line per request. It rotates at 5 MiB. `hedos stats` and the shelf read it. |
| `bundles/` | The Python runtime code that ships inside the binary, unpacked on start. `bundles/Runtimes/` also holds the shipped runtime manifests. |
| `env/` | The Python environments the sidecar runtimes build with `uv`, one per runtime. `env/manifests/` holds the ones for manifest runtimes. |
| `workdirs/` | Scratch directories for the sidecar runtimes. |
| `runtimes.d/` | Your own runtime manifests. See below. |
| `launch/` | The harness configuration `hedos launch` generates. Your own harness config is never touched. |
| `ui/` | What the shelf screen remembers between runs. |

None of this holds model weights. Removing the whole directory loses the registry, generated artifacts, history and pull records, but no model: the next `hedos scan` finds them all again.

## Environment variables

### Where hedos keeps things

| Variable | Effect |
|---|---|
| `XDG_CONFIG_HOME` | Moves the settings file to `$XDG_CONFIG_HOME/hedos.toml`. |
| `XDG_DATA_HOME` | Moves the data directory to `$XDG_DATA_HOME/hedos`. Ignored unless it is an absolute path. |
| `HOME` | The base for both when the XDG variables are unset, for the default model stores, and for `~` in paths. |
| `APPDATA` | The settings directory when neither `XDG_CONFIG_HOME` nor `HOME` is set. |

### Where your models are

| Variable | Effect |
|---|---|
| `OLLAMA_MODELS` | Moves the Ollama store that discovery scans. Default `~/.ollama/models`. |
| `HF_HUB_CACHE` | The Hugging Face hub cache that hedos scans and installs into. |
| `HF_HOME` | The Hugging Face home. Without `HF_HUB_CACHE`, the hub cache is `$HF_HOME/hub`. Default `~/.cache/huggingface`. |
| `HF_TOKEN`, `HUGGING_FACE_HUB_TOKEN` | The token `hedos pull` uses for gated Hugging Face repositories. |
| `HF_TOKEN_PATH` | Where to read the token from when neither variable above is set. Default `$HF_HOME/token`, which is where `huggingface-cli login` writes it. |

hedos sees exactly one Hugging Face cache from the environment, the same rule the Hugging Face tools follow. Add other caches with `models.hf_cache_roots`. LM Studio's library is read from `~/.lmstudio/models` and `~/.cache/lm-studio/models`.

### Runtimes

| Variable | Effect |
|---|---|
| `PATH` | Where hedos finds `llama-server`, the coding harnesses for `hedos launch`, and (after a few standard install locations) `ollama` and `uv`. |
| `HEDOS_OPENAI_API_KEY` | The API key the OpenAI-compatible endpoint runtime sends, to every endpoint. |
| `HEDOS_APPLE_SHIM` | The full path to `libhedos_apple_shim.dylib`, the Apple Intelligence bridge. Tried before the copy next to the `hedos` binary. |

hedos looks for `ollama` in `/usr/local/bin`, `/opt/homebrew/bin` and `~/.local/bin` before `PATH`, and for `uv` in `~/.local/bin`, `/opt/homebrew/bin` and `/usr/local/bin` before `PATH`.

The Python sidecars do not inherit your whole environment. They get an allowlist (`PATH`, `HOME`, `USER`, `LOGNAME`, `SHELL`, the locale and time zone, the temp and proxy variables, and the platform's library path), plus what the runtime itself declares. Credentials in your shell are not passed on.

### The shelf screen

| Variable | Effect |
|---|---|
| `HEDOS_MOTION` | `off`, `0`, `false` or `no` turns the shelf's animation off: every movement shows its final frame. `slow` plays every movement ten times slower. Anything else, or unset, animates normally. |
| `COLORTERM` | `truecolor` or `24bit` draws the shelf in 24-bit colour. Otherwise colours are mapped to the 256-colour palette. |
| `HEDOS_THEME` | `light` or `dark` says which kind of background the terminal has, and the shelf draws on it even when the terminal does not answer the colour query. Anything else, or unset, goes by the terminal's answer; a terminal that does not answer gets the shelf's own painted dark ground. See [Colour](shelf.md#colour). |

The Ollama daemon and the image daemons (ComfyUI, AUTOMATIC1111) are reached over HTTP on their standard local ports. If Ollama is installed but not running, hedos starts it.

### The install script

`curl -fsSL https://hedos.ai/install | bash` installs the binary into `~/.local/bin`. Set `HEDOS_BIN_DIR` to install somewhere else. The `hedos` binary itself does not read this variable.

## Manifest runtimes and consent

Besides its built-in runtimes, hedos can serve a model through a runtime described by a manifest: a TOML file that says which models it handles, what it can do, and how to run it. Some ship inside the binary (the decision runtimes `python:laya` and `python:zerank`, for example). You can add your own under `runtimes.d/` in the data directory.

A manifest runtime runs code on your machine, as you, unsandboxed. So it stays inert until you approve it.

```sh
hedos runtimes                         # every manifest runtime and its approval
hedos runtimes approve python:laya     # show what it runs, then ask to confirm
hedos runtimes revoke python:laya      # take the approval back
```

```
RUNTIME        SERVES      CONSENT         MODELS
python:laya    chat,judge  needs approval  laya
python:zerank  chat,judge  needs approval  -
```

`hedos ls` also points out a model that waits on an approval:

```
laya can run on python:laya, which needs your approval: `hedos runtimes approve python:laya`
```

`approve` shows the runtime's files, what it runs and installs, the paths and network access it declares, the models it would serve, and its hash, then asks you to confirm (`-y` skips the question). It records the id in `models.approved_host_runtimes` and the hash in `models.approved_host_runtime_hashes`. What a manifest declares is its own account and is not enforced.

The hash covers the manifest and every file beside it. If any of them changes, the approval stops counting and `hedos runtimes` shows the runtime as `changed since approval` until you approve it again.

Approvals are read when a process starts. After approving or revoking, restart a running `hedos serve` (the command reminds you when one is running).

### Writing a manifest

An entry in `runtimes.d/` is either a bare `*.toml` file or a directory holding a `manifest.toml` beside its files. A runtime with an `[env]` (a Python environment) or a `[serve]` (a long-running process) must be a directory. A one-shot command can be a bare file:

```toml
id           = "my-tool"
capabilities = ["chat"]
execution    = "sync"
detect       = { extension = "gguf" }

[invoke]
command = "my-tool --model {model} --prompt {prompt}"
```

A few rules hedos checks when it loads them:

- An id may use letters, digits, dots, underscores, colons and hyphens. Ids of built-in and shipped runtimes are reserved, and a duplicate id keeps the first one.
- A manifest declares exactly one of `[invoke]` or `[serve]`.
- A manifest needs a `detect` rule (a file `extension`, or a marker `file`, optionally with `contains`) to ever match a model. A manifest runtime bids last, so it only serves models no built-in runtime takes.
- A manifest with a `[vm]` section is reported as an issue and not run: this build cannot start a VM.

Problems show up as `issue:` lines in `hedos scan`.
