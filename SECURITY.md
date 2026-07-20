# Security Policy

hedos runs models on your own machine, installs weights from remote sources, serves your shelf over a local HTTP endpoint, and can run community runtime definitions that launch processes on your behalf. Those capabilities make responsible disclosure genuinely valuable, and reports are appreciated.

## Reporting a vulnerability

**Please do not open a public issue for a security problem.**

Report privately through GitHub's [private vulnerability reporting](https://github.com/theiskaa/hedos/security/advisories/new), the "Report a vulnerability" button under the repository's **Security** tab. If you cannot use that, email **me@theiskaa.com** with the details.

A useful report includes:

- the version or commit you tested,
- your operating system and architecture,
- a clear description of the issue and its impact,
- the steps to reproduce it (a minimal proof of concept helps),
- and any thoughts on a fix, if you have them.

Please give a reasonable window to investigate and address the issue before any public disclosure. You will get an acknowledgement, updates as the fix progresses, and credit in the release notes if you would like it.

## The trust model, so expectations are clear

A few design choices matter for how you deploy hedos:

- **The gateway is open on loopback.** `hedos serve` binds to `127.0.0.1` and treats every local caller as trusted. It does not require a token. This is deliberate for a single-user machine, where the gateway exists so your own tools can reach your own models. It also means anything that can reach the port can use every model on your shelf. Binding it to a public interface, or putting a reverse proxy in front of it, exposes your shelf to whoever can reach that address. That is your configuration choice, not a hedos vulnerability, but it is the sharpest edge, so treat the loopback boundary as the security boundary.
- **Community runtimes run processes.** A runtime manifest you install into `runtimes.d` can define a command or a Python sidecar that hedos launches to serve a model. hedos records a consent hash and asks for host-execution approval before running one, but an approved runtime runs with your privileges. Only install runtime definitions you trust.
- **Weights are read, not written.** hedos never moves, copies, or re-downloads your existing model files. Discovery and serving only ever read them. Installs write into the standard cache layouts (the Ollama store, the Hugging Face hub cache), and removal deletes only what the deletion preview reports.

## Areas of particular interest

If you are looking for where the sharp edges are, these are the surfaces where a vulnerability would matter most:

- **The gateway.** The loopback HTTP server, its request parsing, and the boundary between a request and the runtime it reaches. A crash, a hang, or a way to reach a model or a filesystem path a request should not, all matter.
- **Untrusted-input parsers.** The WAV, GGUF, safetensors, multipart, and Hugging Face API decoders all read data hedos did not produce. A panic, an out-of-bounds read, or unbounded memory growth on crafted input is in scope.
- **The install path.** URL construction, `Range` resume, and the SHA-256 verification against LFS oids. Any way to make hedos write outside the target cache directory, or accept a file that fails verification, is a serious bug.
- **Runtime execution.** The command and sidecar adapters that launch processes from a runtime manifest, and the consent and approval gates in front of them. A path that runs a command or a Python process without the corresponding approval is in scope.

## What is not a vulnerability

- The gateway is bound to `127.0.0.1` and is open to local callers by design. Exposing it beyond your machine is your configuration choice.
- A model producing wrong, offensive, or low-quality output is model behavior, not a security issue.
- Running a runtime you explicitly approved is the product working as designed.
