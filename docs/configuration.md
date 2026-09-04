# Configuration

hedos reads one settings file and keeps its state in one data directory. Both follow the XDG conventions, and both can be relocated with environment variables.

## The settings file

Settings live at `~/.config/hedos.toml`, or `$XDG_CONFIG_HOME/hedos.toml` when that variable is set. The file is optional and hand-editable. Loading is tolerant: a missing file, a malformed file, or a malformed table falls back to defaults for just that scope, so a typo never wipes every setting.

It has a few tables. The ones that affect the headless build are below.

```toml
[models]
# Extra folders to scan for local models, beyond the standard stores.
watched_folders = []
# Extra Hugging Face hub-cache roots to scan.
hf_cache_roots = []
# How long to keep an idle model warm: "five-minutes", "fifteen-minutes",
# "one-hour", or "never".
keep_warm = "five-minutes"
# How the governor makes room: "strict-single" keeps one heavy model resident;
# "budgeted" keeps as many as fit a budget.
eviction = "strict-single"
# An explicit RAM budget in megabytes for the budgeted policy. 0 means unset.
ram_budget_mb = 0

[chat]
# A default system prompt applied when a request and a record set none.
default_system_prompt = ""

[gateway]
# The loopback port the gateway binds. `hedos serve -p` overrides this.
port = 43367
# The number of inference requests served at once.
max_concurrent_inference = 4

[pull]
# How many pulls transfer at once; the rest queue for a free slot. The cap
# holds across terminals.
max_concurrent = 2
# Whether a pull whose worker died (a closed laptop, a killed terminal) is
# started again when the shelf next opens, or when you pull it again. A pull
# you paused stays paused.
auto_resume = true
# How long a half-downloaded file is kept once nothing is fetching it, in
# hours. A paused pull's bytes live this long, so it is also how long a pause
# is worth resuming.
partial_age_hours = 24
# How long a failing transfer keeps retrying, in minutes, before it is left
# interrupted for you to resume.
retry_window_minutes = 120
# How many ended pulls keep their record for `hedos pull ls` and the pulls
# screen. Older ones are dropped when the shelf opens or a pull starts;
# `hedos pull clean` drops them sooner. 0 keeps none.
keep_ended = 20

[advanced]
# How many finished jobs to keep in history.
job_history_limit = 50
```

Anything you leave out uses its default.

## The data directory

State lives under `~/.local/share/hedos`, or `$XDG_DATA_HOME/hedos` when that variable is set. hedos creates it on first use. It contains:

- `registry/` the model registry.
- `artifacts/` generated outputs (speech, images) and their provenance.
- `history/` the job history.
- `gateway/` the gateway's rotating audit log.
- `workdirs/` and `env/` scratch and environment directories for the Python sidecar runtimes.

None of this holds your model weights. The registry only records where the weights already sit.

## Environment variables

hedos reads these from the environment:

- `XDG_CONFIG_HOME` and `XDG_DATA_HOME` relocate the settings file and the data directory.
- `HOME` is the fallback base for both when the XDG variables are unset, and the base for tilde expansion in paths.
- `HF_TOKEN`, `HF_TOKEN_PATH`, or the token `huggingface-cli login` writes (under `$HF_HOME`, default `~/.cache/huggingface`) authenticates gated Hugging Face repositories during `hedos pull`.
- `HF_HUB_CACHE` and `HF_HOME` locate the Hugging Face cache that hedos scans and installs into. If neither is set, it uses `~/.cache/huggingface/hub`.
- `HEDOS_OPENAI_API_KEY` provides the API key the OpenAI-endpoint runtime uses.
- `OLLAMA_MODELS` relocates the Ollama store that discovery scans.

The Ollama and image daemons are reached over HTTP on their standard local ports.
