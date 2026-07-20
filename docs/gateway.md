# Gateway

`hedos serve` runs a local HTTP server that speaks the OpenAI and Ollama dialects. Point any tool that already talks to one of those at it, and it reaches the models on your shelf.

## Starting it

```sh
hedos serve            # binds 127.0.0.1:43367
hedos serve -p 8080    # a different port
```

The default port is `43367`, chosen to avoid colliding with Ollama's `11434`. The port also comes from your settings if you set one there. The server prints its base URL on startup and stops cleanly on Ctrl-C, flushing its audit log on the way out.

## Authentication

The gateway binds to `127.0.0.1` and treats every local caller as trusted. It does not require a token. This keeps local tools frictionless on a single-user machine. It also means the loopback boundary is the security boundary: anything that can reach the port can use every model on your shelf. Do not bind it to a public interface or place a proxy in front of it. See [SECURITY.md](../SECURITY.md).

## OpenAI endpoints

Base path `/v1`.

| Endpoint | Purpose |
| --- | --- |
| `POST /v1/chat/completions` | Chat, streaming or unary, with tool calling. |
| `POST /v1/completions` | Prompt completion. |
| `POST /v1/embeddings` | Embed text into vectors. |
| `POST /v1/images/generations` | Generate an image, returned as base64. |
| `POST /v1/audio/speech` | Synthesize speech to WAV audio. |
| `POST /v1/audio/transcriptions` | Transcribe an uploaded audio file. |
| `GET /v1/models` | List the models this gateway can reach. |

Example:

```sh
curl http://127.0.0.1:43367/v1/chat/completions \
  -d '{
    "model": "qwen2.5",
    "messages": [{"role": "user", "content": "hi"}],
    "stream": true
  }'
```

Streaming responses use server-sent events, the same shape OpenAI clients expect. Set `"stream": false` for a single JSON response.

## Ollama endpoints

Base path `/api`.

| Endpoint | Purpose |
| --- | --- |
| `POST /api/chat` | Chat over the Ollama NDJSON protocol. |
| `POST /api/generate` | Prompt generation, Ollama-style. |
| `POST /api/embed` | Embed text. |
| `POST /api/embeddings` | Embed text (legacy endpoint). |
| `GET /api/tags` | List models, Ollama-style. |
| `GET /api/version` | Version handshake for stock clients. |
| `POST /api/show` | Model details handshake. |

Example:

```sh
curl http://127.0.0.1:43367/api/chat \
  -d '{
    "model": "qwen2.5",
    "messages": [{"role": "user", "content": "hi"}]
  }'
```

Ollama streaming responses are newline-delimited JSON, one object per line, which is what stock Ollama clients read.

## Fidelity

The gateway serves each model's real behavior, not a lowest common denominator. It reads the model's context length, chat template, and tool-calling dialect and honors them. When a request asks for something a model cannot do (a capability it does not serve, or a parameter the runtime does not honor), the gateway returns a clear error in the dialect you used rather than pretending.

## Concurrency

The number of requests served at once is bounded by the `max_concurrent_inference` setting. Requests beyond that wait for a slot. This keeps a burst of clients from oversubscribing the machine's memory.
