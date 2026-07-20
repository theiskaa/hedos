# Architecture

hedos is a Cargo workspace of four crates. Dependencies flow in exactly one direction, and logic sits as low as it can, so that the parts a person touches stay thin and the parts that carry behavior stay testable without any UI or network.

```
kernel  ->  runtime  ->  gateway  ->  cli
```

Nothing points back up the chain.

## The crates

### `kernel`

Pure, synchronous logic and nothing else. It has the model record and the registry, the discovery scanners that read the machine's model stores, the install and removal planners, the resolution engine that decides which runtime fits a model, the settings model, the artifact and job types, and the runtime-manifest data model. There is no async here and no framework, only data and the filesystem. Because it is pure, it runs headlessly and is covered by ordinary unit tests. A package named `core` would collide with the standard library, so this crate is named `kernel`.

### `runtime`

The async layer, built on tokio. It holds the execution adapters (a `llama-server` subprocess pool for local GGUF, the Ollama proxy, OpenAI-compatible endpoints, the Python sidecars, whisper, and the image daemons), the sidecar supervisor that manages those child processes, and the memory governor that coordinates how many models are resident at once. It also has the install and removal services and the settings store. Two pieces tie it together:

- **`facade`** exposes the `Kernel` type, the single async entry point. It owns the registry, governor, scheduler, and adapters, and its methods (`invoke`, `submit`, `discover`, `shelf`, `voices`, and so on) apply the shared prompt, parameter, and context policy before driving a request to an adapter or through the job scheduler.
- **`boot`** is the composition root. `build_kernel` assembles a production `Kernel` from a data directory and settings: it opens the stores, detects the machine's memory for the governor, and wires the full built-in adapter set. The CLI and any future front end call this so they never have to assemble the engine themselves.

### `gateway`

The loopback HTTP server, built on axum. It speaks the OpenAI (`/v1`) and Ollama (`/api`) dialects. A `wire` layer decodes each dialect's request into the kernel's shape and encodes the kernel's output back. Handlers resolve and authorize a model, guard its parameters, and stream the result. A `KernelGateway` bridge presents the runtime `Kernel` behind the gateway's own port trait, so the HTTP layer depends on an interface rather than the concrete kernel. Authentication is open on loopback, and a rotating JSONL audit log records each served request.

### `cli`

The `hedos` binary. It parses arguments, calls `boot::build_kernel`, drives one command, formats the output, and maps any error to an exit code. The commands are thin: they translate flags into a kernel call and a runtime stream into terminal output. Shared pieces (the session that opens a kernel and resolves a model name, the output helpers, interrupt handling) live in a small support module.

## How a request flows

A `hedos run gemma3 "hi"` goes like this: the command opens a kernel through `boot`, lists the shelf (discovering if it is empty), resolves the name to a record, builds a chat payload, and calls `kernel.invoke`. The kernel resolves the record to an adapter, passes it through the governor for admission, and returns a stream of chunks. The command prints each text chunk as it arrives.

A gateway request follows the same spine. An HTTP request reaches the router, which authenticates it, caps concurrency, and dispatches to a handler. The handler decodes the dialect, resolves and authorizes the model, and calls the same `invoke` through the `KernelGateway` bridge. The chunks stream back out in the dialect the client used.

The two front ends share one engine. That is the point of keeping the composition root and all the behavior in `runtime` and `kernel`: the CLI and the gateway are two thin shells over the same core.

## The governor

Every engine funnels through the memory governor. It coordinates residency (which models stay loaded and for how long), admission (whether a new request has room), and a fair gate that keeps two heavy loads from oversubscribing memory. A single governor is shared across the kernel and every adapter, so a model loaded to serve one request is accounted for when the next arrives.

## The one invariant

Across all of this, hedos never moves, copies, or re-downloads a user's model weights. Discovery and serving only read them. Installs write into the standard cache layouts, and removal deletes only what its preview reports. If a change would write to or relocate existing weights, it is wrong.
