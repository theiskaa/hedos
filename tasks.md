# Tasks

Deferred work for the Rust rewrite on the `rust` branch.

## Bridge Rust to Apple's on-device model (AppleFoundation)

The old Swift kernel could serve Apple's built-in model (the Apple Intelligence one)
through the AppleFoundation adapter. That model only exists behind Apple's Swift-only
`FoundationModels` framework, so there's no HTTP or subprocess we can talk to from Rust.
We skipped it during the port for that reason. This task is about un-skipping it.

The plan is to write a tiny Swift shim that imports `FoundationModels` and exposes a
flat C ABI (opaque handles + callbacks, `@_cdecl` functions), compile it to a small
library, and call it from Rust over FFI. On the Rust side that becomes a normal
`RuntimeAdapter`: availability check, load, a streaming generate that forwards tokens
into our `ChunkStream`, cancel, unload. All of it gated behind a macOS-only cargo
feature and a `build.rs`, with a fake backend so it stays testable on Linux (same shape
as `WhisperBackend`/`MissingWhisperBackend`).

Worth deciding early: static-link the Swift shim vs `dlopen` a dylib at runtime. The
dylib route keeps the Rust binary buildable on machines without the Swift toolchain.

Done when:

- [ ] On a Mac with the model available, chat/complete streams end to end.
- [ ] Everywhere else it reports itself unavailable with a clear message, and the build
  and existing tests are unaffected (feature off on Linux).
- [ ] build/test/clippy/fmt pass with the feature on and off.
- [ ] Reviewed for FFI and cancel safety.

Swift reference: `Sources/HedosKernel/Runtimes/AppleFoundation/*.swift` at `f85874f`.

## In-process mlx-swift over the same bridge (MlxSwift)

Depends on the AppleFoundation bridge above, since mlx-swift is another Swift-only
Apple framework we'd reach the same way.

Right now MLX models already run in Rust through the Python `mlx-lm`/`mlx-vlm` sidecars,
so nothing is broken. What the Swift app also had was `MlxSwiftAdapter`, which runs the
same models in-process via Swift with no Python involved. That's the only thing we lost
by skipping it: the no-Python, lower-overhead path.

Once the bridge from the first task exists, reuse its mechanism to import mlx-swift and
add an in-process `RuntimeAdapter` for MLX chat/vision models. The one thing to get
right is the bid: when both this and the Python sidecar can serve a model, the in-process
one should usually win (it avoids spawning Python), but they must not fight over the same
model or double-serve it.

Done when:

- [ ] MLX models can be served in-process on Apple Silicon with no Python.
- [ ] It out-prefers the Python sidecar when both can run the model, cleanly.
- [ ] Falls back to unavailable off Apple Silicon; build and tests unaffected.
- [ ] Reviewed.

Swift reference: `Sources/HedosKernel/Runtimes/MlxSwift/*.swift` at `f85874f`.

## Tool calling through the Python sidecars (mlx-lm)

Every coding harness except aider drives the model through tool calls, so `hedos launch`
only offers tool-capable models. No model served by a Python sidecar qualifies today,
because the mlx-lm sidecar doesn't wire tools: `runtime/runtimes/python-mlx-lm/main.py`
renders only the message list into the prompt and never forwards a `tools` array or parses
tool calls back out. So capable instruct models (Llama-3.2-Instruct, Qwen mlx builds) can't
be used with a harness even though the weights support it. This is a runtime-integration
gap, not a model limit, and it can be closed entirely in code we own, with no change to
mlx-lm or transformers.

Two halves, both in the sidecar. Inbound is small: `load()` hands us the model's real
HuggingFace tokenizer, and `apply_chat_template` takes a standard `tools=` kwarg (we already
pin `transformers<5`, which has it), so passing the request's `tools` through lets the
model's own chat template render them into the prompt. Outbound is the real work: mlx emits
plain text, so the sidecar buffers the output and parses any tool call into structured
calls before emitting. Each model family uses its own format (Qwen/Hermes
`<tool_call>{...}</tool_call>`, Llama 3.1/3.2 raw JSON or `<|python_tag|>`, Mistral
`[TOOL_CALLS]`), so this is a best-effort per-family parser plus a bare-JSON fallback; tool
calls are parsed at end-of-turn rather than streamed, and unrecognized output degrades to
plain text rather than failing.

The Rust side already models tool-call chunks (ollama and openai emit them), so the
kernel/gateway path is ready; check that the sidecar frame protocol (`runtime/src/sidecar/`,
the `send_json` events in `main.py`) can carry a `tool_call` event and add one on both sides
if not. Once it works, add `python:mlx-lm` (and mlx-vlm, given the same treatment) to
`runtime/src/resolution.rs::runtime_wires_tools`, so those models start showing the `tools`
capability and appear in the launch picker.

Done when:

- [ ] `hedos launch opencode -m <an mlx instruct model>` completes a tool-driven turn (a
  file read or edit), not just chat.
- [ ] A model whose template lacks tool markers still shows no `tools` and stays excluded,
  so the runtime gate and the template gate compose.
- [ ] Plain-text replies still stream; only tool-call turns buffer.
- [ ] build/test/clippy/fmt pass.
- [ ] Reviewed.

No Swift reference: the Swift app served MLX in-process through `MlxSwiftAdapter` rather
than a Python sidecar, so this is new work in the sidecar rather than a port.
