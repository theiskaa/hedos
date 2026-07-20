# Contributing to hedos

Thanks for your interest in hedos. This guide covers how to build it, the conventions the code follows, and how to get a change merged.

## Building and testing

hedos is a Cargo workspace. You need a recent Rust toolchain with edition 2024 support, and `python3` on your `PATH` if you want to exercise the Python sidecar runtimes.

```sh
cargo build                              # compile the workspace
cargo test                               # run the full suite
cargo clippy --all-targets -- -D warnings   # lint
cargo fmt --check                        # format check
```

All four of these should pass before you open a pull request. `cargo fmt` (without `--check`) applies the formatting.

## How the code is organized

The workspace has four crates, and dependencies only ever flow in one direction: `kernel` to `runtime` to `gateway` to `cli`. Nothing points back up.

- **`kernel`** is pure logic: records, the registry, discovery, install and removal planning, resolution, and settings. It has no async and no framework, only data and the filesystem. This is where behavior lives so that it can run and be tested headlessly.
- **`runtime`** adds the async layer: engine selection, the `llama-server` subprocess pool, the Python sidecar supervisor, the memory governor, and the install and removal services.
- **`gateway`** is the loopback HTTP server built on axum, speaking the OpenAI and Ollama dialects.
- **`cli`** is the `hedos` binary, a thin shell over the three crates below it.

The guiding principle is kernel-first, shell-thin. If you find yourself writing logic in a command, ask whether it belongs in a crate below. The CLI and gateway should mostly translate inputs, call down, and format outputs.

## Code style

- Small modules with clear names carry the design. Use `pub(crate)` and module privacy to mark boundaries, and do not over-expose.
- No decorative comments. A comment should teach something the code cannot: a rationale, an invariant, the meaning of a magic value. Skip comments that restate the obvious.
- Public items get `///` docs, and modules get a `//!` header.
- Prefer `&str` over `String` in function parameters.
- Use `thiserror` for library errors and `anyhow` for application-level plumbing.
- No `unwrap()` or `expect()` in library code. They are fine in tests.
- Any `unsafe` block needs a `// SAFETY:` comment explaining why it is sound.

## Tests

New behavior comes with tests. The kernel and runtime crates keep unit tests inline and integration tests under `tests/`. The gateway tests exercise handlers against a mock port and the full server over real HTTP. Aim to cover the behavior, not just the happy path.

## Commits and pull requests

Commit messages are single-line and describe the change, not the process:

```
type(scope): what changed
```

Types are `feat`, `fix`, `refactor`, `chore`, `docs`, `test`, `perf`, and `ci`. Scopes are `repo`, `kernel`, `runtime`, `gateway`, and `cli`.

For a pull request, describe what changed and why, keep it focused, and make sure the four checks above are green. If your change touches behavior that other tools rely on (the gateway dialects, the discovery layout, the install cache format), call that out.

## Weights are never touched

One rule holds everywhere: hedos never moves, copies, or re-downloads a user's model weights. Discovery and serving only read them. If a change would write to, relocate, or duplicate existing model files, it is almost certainly wrong.

## Conduct and security

Participation is governed by the [Code of Conduct](CODE_OF_CONDUCT.md). If you find a security issue, please use the private channel described in [SECURITY.md](SECURITY.md) rather than a public issue.
