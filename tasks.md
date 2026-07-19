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
