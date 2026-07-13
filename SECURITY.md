# Security Policy

Hedos runs models on your own machine and, when you allow it, lets a model read files, write files, and run commands inside a folder you choose — and it can serve your shelf over a local HTTP endpoint. Those capabilities make responsible disclosure genuinely valuable, and reports are appreciated.

## Reporting a vulnerability

**Please do not open a public issue for a security problem.**

Report privately through GitHub's [private vulnerability reporting](https://github.com/theiskaa/hedos/security/advisories/new) — the "Report a vulnerability" button under the repository's **Security** tab. If you can't use that, email **me@theiskaa.com** with details.

A useful report includes:

- the version or commit you tested,
- your macOS version and Mac chip,
- a clear description of the issue and its impact,
- the steps to reproduce it (a minimal proof of concept helps),
- and any thoughts on a fix, if you have them.

Please give a reasonable window to investigate and address the issue before any public disclosure. You'll get an acknowledgement, updates as the fix progresses, and credit in the release notes if you'd like it.

## Areas of particular interest

If you're looking for where the sharp edges are, these are the surfaces where a vulnerability would matter most:

- **The place boundary** — the canonicalized path prefix that confines a conversation's file access to its folder. Any way to read, write, or execute *outside* the granted place is a serious bug.
- **Consent** — the per-action approval for writing files and running commands. Any path that mutates the disk or runs a command without the corresponding consent is in scope.
- **The gateway** — the loopback HTTP endpoint, its token authentication, and its scopes. Auth bypass, scope escape, or reaching a model a token isn't scoped for all matter here.
- **Sidecar and command sandboxing** — the isolation around the Python runtimes and any command the harness runs.

## What is not a vulnerability

- The gateway is bound to `127.0.0.1` and requires a token. Exposing it beyond your machine — for example by putting a reverse proxy in front of it — is your configuration choice, not a Hedos vulnerability.
- A model producing wrong, offensive, or low-quality output is a model behavior, not a security issue.
- Actions a model takes *within a place you granted and consent you gave* are the product working as designed.
