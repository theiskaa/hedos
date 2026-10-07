# Architecture

hedos is a Cargo workspace of four crates. Dependencies flow in one direction only, and logic sits as low as it can. The parts a person touches stay thin, and the parts that carry behaviour can be tested without a terminal, a network, or a model.

```
kernel  ->  runtime  ->  gateway  ->  cli
```

Nothing points back up the chain. The crates sit at the repository root. They are published as `hedos-kernel`, `hedos-runtime` and `hedos-gateway` (the bare names are taken on crates.io), and the binary's package is `hedos`.

## The crates

### `kernel`

Pure, synchronous logic and the filesystem. No async runtime, no HTTP, no UI. It holds:

- the model record and the registry that persists it,
- the discovery scanners that read each model store (Ollama, the Hugging Face cache, LM Studio, loose GGUF and safetensors files),
- identification: what format a model is, what it can do, its context length, chat template and tool-calling dialect,
- the install planning (reading a reference, choosing files, the pull records on disk) and the removal planning,
- fit verdicts and the parameter and context policy,
- the capability types (chat, tools, decisions), the artifact and job types, and the runtime-manifest data model.

Because it is pure, it runs headlessly and is covered by ordinary unit tests. A package named `core` would collide with the standard library, so this crate is named `kernel`.

### `runtime`

The async layer, built on tokio. It turns the kernel's records into running models.

- **Adapters**, one per way of serving a model: a `llama-server` subprocess pool for local GGUF, the Ollama proxy, OpenAI-compatible endpoints, the Python sidecars (mlx-lm, mlx-vlm, mlx-audio, embeddings, diffusers, mflux, whisper), the image daemons (ComfyUI, AUTOMATIC1111), Apple Intelligence, and the manifest runtimes.
- **The resolution auction**, which asks every adapter to bid on each model and writes the winner (and the runners-up) onto its record.
- **The sidecar supervisor**, which starts, watches and stops the Python child processes, and the environment manager, which builds each one's Python environment with `uv`.
- **The memory governor**, which decides what may load and what must make room.
- The install and removal services, the pull worker, the job scheduler, and the settings store (`hedos.toml`).

Two pieces tie it together:

- **`facade`** exposes the `Kernel` type, the single async entry point. It owns the registry, governor, job scheduler, artifact store and adapters. Its methods (`invoke`, `submit`, `discover`, `shelf`, `voices`, and so on) apply the shared prompt, parameter and context policy before handing a request to an adapter or to the job scheduler.
- **`boot`** is the composition root. `build_kernel` assembles a production `Kernel` from a data directory and settings: it opens the stores, detects the machine's memory for the governor, unpacks the shipped Python runtimes, and wires every built-in adapter plus the manifest runtimes. Every front end calls it, so none of them assembles the engine itself.

### `gateway`

The loopback HTTP server, built on axum. It speaks four dialects: OpenAI (`/v1`), Ollama (`/api`), Anthropic Messages (`/v1/messages`), and TypeSafe's typed decisions (`/v1/systemone`).

- A **wire** layer decodes each dialect's request into the kernel's shape and encodes the kernel's output back.
- The **router** authenticates a request, matches its route, caps concurrent inference, runs the handler, and writes an audit entry.
- **Handlers** resolve the requested model name against the shelf, reject parameters the model's runtime would silently ignore, and stream the result.
- A **`GatewayPort`** trait is everything a handler needs from the engine. `KernelGateway` implements it over the runtime `Kernel`, so the HTTP layer depends on an interface rather than the concrete engine, and handlers can be tested against a double.

Authentication is open on loopback: every local caller is trusted. A rotating JSONL audit log records each served request.

### `cli`

The `hedos` binary. It parses arguments, opens a session (`boot::build_kernel` over the detected data directory and settings), drives one command, formats the output, and maps any error to an exit code.

The commands are thin: they turn flags into a kernel call and a stream of chunks into terminal output. Shared pieces live in a support module: the session that opens a kernel and resolves a model name, output and table helpers, interrupt handling, the harness wiring for `hedos launch`, and the pull client.

## How a request flows

### From the CLI

`hedos run gemma3 "hi"`:

1. The command opens a session: `boot::build_kernel` builds the engine from the data directory and `hedos.toml`.
2. It lists the shelf, running discovery first if the shelf is empty.
3. It resolves `gemma3` to a record: an exact id, then an exact name (ignoring case), then a unique substring.
4. It builds a chat payload and calls `kernel.invoke`.
5. The facade finds the adapter for the record's resolved runtime, merges the record's parameters and the system prompt, and clamps the request to the model's context window.
6. The adapter serves it (starting `llama-server`, a sidecar, or the Ollama daemon if needed) and returns a stream of chunks.
7. The command prints each text chunk as it arrives.

### From the gateway

A `POST /v1/chat/completions` follows the same spine:

1. axum turns the HTTP request into a gateway request and hands it to the router.
2. The router authenticates it, matches the route, and checks the inference cap (`gateway.max_concurrent_inference`). A saturated gateway refuses the request with a `Retry-After` of one second rather than queueing it.
3. The handler decodes the dialect, resolves the model name, and guards its parameters.
4. It calls the same `invoke` through `KernelGateway`.
5. The chunks stream back out, encoded in the dialect the client used, and the request is written to the audit log.

```
  hedos run            HTTP client
      |                     |
      |               axum server
      |                     |
      |               router: auth, route, inference cap, audit
      |                     |
      |               handler: decode dialect, resolve model, guard params
      |                     |
      |               KernelGateway (GatewayPort)
      |                     |
      +---------+-----------+
                |
         Kernel facade: record, adapter, prompt + params, context clamp
                |
             adapter
      +---------+---------+-----------+-------------+
      |         |         |           |             |
 llama-server  Ollama  Python      endpoint,     manifest
    pool       daemon  sidecars    Apple, image  runtimes
                       (governor)  daemons
```

The two front ends share one engine. That is the point of keeping the composition root and all the behaviour in `runtime` and `kernel`: the CLI and the gateway are two thin shells over the same core.

## Discovery and resolution

`hedos scan` (and the first command on an empty shelf) runs every scanner over the machine's model stores, reconciles what they find into the registry, and then runs the resolution auction over the whole registry.

In the auction, each adapter looks at a model's identification (its format, files and capabilities) and bids if it can serve it. Each bid carries a tier (native, managed, or remote) and a preference. The lowest preference wins, ties broken by runtime id, and the winner is written onto the record along with the runners-up. Manifest runtimes bid last, so they only take models no built-in runtime serves. A model nobody bids on stays on the shelf without a runtime, and `hedos ls` shows a dash in its runtime column.

Discovery and resolution happen under one registry lock, so the shelf is never seen half-reconciled.

## Runtimes and sidecars

Every built-in adapter is registered whether or not its backend is installed. A capability only actually serves when its backend is there: a `llama-server` binary, the Ollama daemon, `uv` for the Python runtimes, a running image daemon.

- **`llama-server`** runs one server per model, reused across requests. A cold server is polled on `/health` until it is ready.
- **The Python sidecars** are long-running child processes speaking a framed protocol over stdin and stdout. Their code ships inside the binary and is unpacked into the data directory on start. Each runtime gets its own Python environment, built by `uv` from a lockfile and rebuilt only when the lockfile changes. A sidecar inherits only an allowlisted environment, so the credentials in your shell are not passed to it.
- **Manifest runtimes** are sidecars or one-shot commands described by a TOML manifest. They run code on the host, so they need your approval first. See [Manifest runtimes and consent](configuration.md#manifest-runtimes-and-consent).
- **Ollama, OpenAI-compatible endpoints, the image daemons and Apple Intelligence** are reached over HTTP or a native bridge; hedos does not host their weights itself.

The MLX-Swift runtime from the original macOS build is framework-bound and out of this port; its models are served by the MLX sidecars instead.

## The governor

The memory governor coordinates residency (which models stay loaded, and for how long), admission (whether a new load has room), and a gate that keeps two heavy loads from oversubscribing memory at the same moment. One governor is shared by the Python sidecars, whisper, the manifest runtimes and the job scheduler, so a model loaded for one request is accounted for when the next arrives. `llama-server` keeps its own pool of servers, and Ollama and remote endpoints manage their own memory.

Its policy comes from `[models]` in `hedos.toml`: `keep_warm`, `eviction`, and `ram_budget_mb`. See [configuration.md](configuration.md#models).

## The shelf screen

`hedos shelf` is a ratatui screen in the `cli` crate, and it is as thin as the commands. Its state is reduced from key and timer events without touching the engine. The effects that need the engine (warming, pulling, removing, scanning, a chat turn) run as tasks over the same session and `Kernel` the commands use. Anything that needs the whole terminal, such as `hedos chat`, a coding harness, or `hedos serve`, is a hand-off: the screen steps aside, runs it, and comes back. Its verbs are the ones the subcommands expose. See [shelf.md](shelf.md).

## The one invariant

Across all of this, hedos never moves, copies, or re-downloads a model's weights. Discovery and serving only read them. Installs write into each platform's own layout (the Ollama store through its daemon, the standard Hugging Face hub cache), and removal deletes only what its preview reports. If a change would write to or relocate existing weights, it is wrong.
