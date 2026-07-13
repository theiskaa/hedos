# Contributing to Hedos

Thank you for considering a contribution. Hedos is open source, top to bottom, and shaped by the people who rely on it. This document covers how to report problems, how to open a pull request, and the few conventions the codebase holds firmly.

New to open source? GitHub's [open source guides](https://opensource.guide/how-to-contribute/) are a good starting point.

## Reporting a problem

Before opening an issue, check whether it has [already been reported](https://github.com/theiskaa/hedos/issues). If an open issue already covers it, add a comment there instead of filing a duplicate; if a *closed* issue looks related, open a new one and link to it.

Be as descriptive as you can — what you did, what you expected, what happened. Because Hedos discovers models across the machine, it helps enormously to say **how the model was installed** (Ollama, Hugging Face cache, LM Studio, a loose file), your macOS version, and your Mac's chip. Screenshots of the shelf or the conversation are welcome.

For anything security-sensitive, do **not** open a public issue — see [SECURITY.md](SECURITY.md).

## Building and testing

Hedos is a SwiftPM package that runs on macOS 26 (Tahoe) on Apple Silicon and builds with the macOS 26 SDK toolchain on Swift 6.1. You need `python3` on your `PATH`; the test suite drives stdlib-only fake sidecars and needs no network.

```sh
make build     # swift build
make test      # swift test
make app       # assemble dist/Hedos.app
make run       # build the bundle and open it
```

**`swift build` and `swift test` must both pass before you commit.** A change that doesn't build, or that breaks a test, isn't ready.

## Conventions

A handful of rules keep the codebase coherent. PRs are expected to follow them.

- **Kernel-first, UI-thin.** Logic lives in `Sources/HedosKernel`, where it runs headlessly and is tested in `Tests/HedosKernelTests`. `Sources/Hedos` is a thin SwiftUI shell over it. New behavior gets a kernel test; new UI stays a shell.
- **Weights are never moved, copied, or re-downloaded.** Records point at where other tools already put the files. Discovery reads; it never relocates.
- **Every change is a vertical slice that runs.** No speculative subsystems with no caller — a subsystem arrives with the first feature that needs it.
- **State never lies.** Where a model can't do something, the shelf says so plainly rather than pretending or silently dropping the request. Keep that honesty when you touch capability, resolution, or the wire.

### Commits

Single-line [Conventional Commits](https://www.conventionalcommits.org/), nothing more:

```
type(scope): what changed
```

Types: `feat`, `fix`, `refactor`, `chore`, `docs`, `test`, `perf`, `ci`. Scopes in use include `repo`, `kernel`, `app`, `discovery`, `registry`, `runtime`, `gateway` — extend as new subsystems appear. No body, no title/body split.

Examples: `feat(discovery): add hf-cache scanner`, `fix(gateway): tolerate missing client store`, `refactor(kernel): split records module`.

## Opening a pull request

A PR doesn't have to be finished work — opening one early is a fine way to get feedback. Just mark it a work in progress and keep pushing commits.

1. [Fork the repository](https://help.github.com/articles/fork-a-repo/) and clone your fork.
2. Create a branch from `main`.
3. Make your change, keeping `swift build` and `swift test` green.
4. Push, and [open a pull request](https://github.com/theiskaa/hedos/pulls) against `main`. Reference any related issues (e.g. "Resolves #42"), describe what changed and why, and include screenshots for UI changes.
5. Expect review. You may be asked to adjust the approach or explain your reasoning — that's the process working, not a rejection.

By contributing, you agree that your contributions are licensed under the project's [MIT License](LICENSE).

Welcome aboard.
