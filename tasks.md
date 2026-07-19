# Tasks

Backlog of deferred work for the Rust rewrite (`rust` branch). Written GitHub-issue
style so each can be lifted into a real issue.

---

## 1. Rust ⇄ Swift FFI bridge for Apple's on-device model runtime (AppleFoundation)

**Labels:** `runtime`, `ffi`, `macos`, `enhancement`, `blocked-by-platform`

### Summary
Port `AppleFoundationAdapter` by building a thin Rust → Swift FFI bridge into Apple's
`FoundationModels` framework, so the Rust runtime can serve Apple's built-in
(Apple Intelligence) on-device model as a `RuntimeAdapter`.

### Motivation / context
The Swift kernel served Apple's system model through
`Runtimes/AppleFoundation/{AppleFoundationAdapter,AppleFoundationBackend,SystemFoundationBackend}.swift`.
That model and the API that drives it live entirely inside Apple's Swift-only
`FoundationModels` framework — there is no C, HTTP, or subprocess surface to talk to.
During the Rust port this adapter was **excluded** (documented in
`internal/rust-migration-log.md`) because Rust cannot reach a Swift framework directly.
This task is the way to un-exclude it.

### Proposed approach
Introduce a small, macOS-only Swift shim compiled to a C-ABI library that Rust links
against and calls over FFI:

- **`hedos-apple-bridge` (Swift package/target):** a thin Swift file that imports
  `FoundationModels`, exposes a flat **C ABI** (`@_cdecl` functions), and hides all
  Swift types behind opaque handles + C callbacks. Surface it needs to expose:
  availability probe, load/prepare, a streaming generate that pushes text chunks to a
  Rust callback, cancel, and unload.
- **Build:** a `build.rs` that compiles/links the Swift shim (via `swiftc` or SwiftPM)
  and links `FoundationModels`; gate the whole thing behind a `cfg(target_os = "macos")`
  + a Cargo feature (e.g. `apple-foundation`) so non-macOS builds are unaffected.
- **`AppleFoundationAdapter` (Rust):** implements `RuntimeAdapter`, wraps the FFI in
  safe Rust (each `unsafe` block gets a `// SAFETY:` note), bridges the C callback into
  the crate's `ChunkStream`, and honors the governor/consent flow like the other
  adapters. Streaming maps the callback pushes into `CapabilityChunk::Text` + `Done`.
- Keep the seam **injectable** (a backend trait) so the adapter is testable with a fake
  backend on all platforms, exactly like `WhisperBackend`/`MissingWhisperBackend`.

### Scope
- **In:** the FFI shim, the build wiring, the Rust adapter, a `MissingAppleBackend`
  fallback for non-macOS / unavailable, unit tests against a fake backend.
- **Out:** shipping/signing/entitlements packaging; multi-model selection beyond what
  `FoundationModels` exposes.

### Acceptance criteria
- On macOS with Apple Intelligence available, the adapter serves `chat`/`complete`
  end-to-end and streams tokens.
- On non-macOS (and macOS without the model), it degrades to an `Unavailable` runtime
  with a clear hint — build still compiles and all existing tests stay green.
- `cargo build`/`test`/`clippy -D warnings`/`fmt --check` pass on Linux (feature off)
  and macOS (feature on).
- Full 2-agent review (Rust-idiom + adversarial FFI/cancel-safety) applied.

### Risks / open questions
- FFI + Swift-runtime interop is fragile (ABI stability, Swift runtime linkage, async
  bridging). The `@_cdecl` + opaque-handle + callback shape is the least-brittle option.
- `FoundationModels` is macOS-version-gated; needs a runtime availability check, not
  just a compile-time one.
- Decide: static-link the Swift shim vs. `dlopen` a small dylib at runtime (the latter
  keeps the Rust binary buildable without the Swift toolchain present).

### References
- Swift: `Sources/HedosKernel/Runtimes/AppleFoundation/*.swift` (rev `f85874f`).
- Rust sibling patterns: `runtime/src/adapters/whisper/backend.rs` (injectable backend),
  `runtime/src/adapters/mlx_audio.rs` (adapter shape).

---

## 2. mlx-swift in-process adapter over the Apple bridge (MlxSwift)

**Labels:** `runtime`, `ffi`, `macos`, `enhancement`, `blocked-by-platform`

**Depends on:** #1 (the Rust ⇄ Swift FFI bridge) — reuse the same bridge mechanism.

### Summary
Port `MlxSwiftAdapter` by adding an in-process **mlx-swift** path over the FFI bridge
built in **task #1**, giving MLX-format models a no-Python, in-process serving option
alongside the existing Python `mlx-lm`/`mlx-vlm` sidecars.

### Motivation / context
The Swift kernel had `Runtimes/MlxSwift/{MlxSwiftAdapter,MlxSwiftEngine}.swift`, which
run MLX models **in-process** via the Swift `mlx-swift` package. The Rust port already
serves the *same* MLX models through the ported Python sidecars (`mlx_lm`, `mlx_vlm`,
`mlx_audio`), so `MlxSwiftAdapter` was **excluded as redundant** — no capability is
lost, only the in-process (no-Python-required, lower-overhead) path.

This task restores that in-process path once the FFI bridge from **#1** exists, since
`mlx-swift` is another Swift-only Apple-Silicon framework reached the same way.

### Proposed approach
- Extend the **#1** bridge (or add a parallel Swift target using the same `@_cdecl` +
  opaque-handle + callback conventions) to import `mlx-swift` / `mlx-lm-swift` and expose
  load / streaming-generate / cancel / unload.
- **`MlxSwiftAdapter` (Rust):** implements `RuntimeAdapter`; its `bid` competes with the
  Python mlx sidecars — decide the preference ordering (in-process should generally win
  when available, since it avoids spawning Python). Reuse the governor/think-split
  streaming plumbing the sidecar adapters already use.
- Keep an injectable backend seam + a `Missing`/unavailable fallback, feature-gated to
  macOS + Apple Silicon.

### Scope
- **In:** the mlx-swift FFI surface (built on #1's mechanism), the Rust adapter, its bid
  vs. the Python sidecars, fake-backend tests.
- **Out:** anything that belongs to #1 (the bridge/build scaffolding itself).

### Acceptance criteria
- On Apple Silicon, MLX chat/vision models can be served in-process (no Python) via this
  adapter, and it out-prefers the Python sidecar when both can serve the model.
- Non-macOS / non-Apple-Silicon degrades to `Unavailable`; build + tests unaffected.
- Gates green on Linux (off) and macOS (on); full 2-agent review applied.

### Risks / open questions
- Bid/priority interaction with the existing `mlx-lm`/`mlx-vlm` adapters — must not
  double-serve or livelock the auction.
- mlx-swift version/model-format compatibility with the same weights the Python path
  reads.

### References
- Swift: `Sources/HedosKernel/Runtimes/MlxSwift/*.swift` (rev `f85874f`).
- Depends on the bridge from **task #1**.
- Rust siblings: `runtime/src/adapters/mlx_lm.rs`, `mlx_vlm.rs`.
