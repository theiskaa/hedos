# Gateway

`hedos serve` runs a local HTTP server that speaks the OpenAI, Ollama, and Anthropic dialects. Point any tool that already talks to one of those at it, and it reaches the models on your shelf.

To run a coding harness against it without configuring anything, see [`hedos launch`](cli.md#launch), which serves a gateway for the life of the harness.

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
| `GET /api/ps` | List the models held in memory. Each entry also carries hedos's record `id`. |
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

## Anthropic endpoints

Base path `/v1`.

| Endpoint | Purpose |
| --- | --- |
| `POST /v1/messages` | Chat over the Anthropic Messages protocol, with tool use. |

This is the dialect Claude Code speaks. It exists so `hedos launch claude` works, and the base URL carries no version segment because the client appends `/v1/messages` itself:

```sh
curl http://127.0.0.1:43367/v1/messages \
  -H 'content-type: application/json' \
  -d '{
    "model": "qwen2.5",
    "max_tokens": 1024,
    "messages": [{"role": "user", "content": "hi"}]
  }'
```

Streaming responses use Anthropic's own server-sent event grammar: `message_start`, then a `content_block_start` / `content_block_delta` / `content_block_stop` group per content block, then `message_delta` and `message_stop`. There is no `[DONE]` sentinel.

Two limits worth knowing. Thinking is not sent as a `thinking` block, because those carry a signature this gateway cannot issue and clients replay the blocks they receive. And `/v1/messages/count_tokens` is not served, so Claude Code estimates context usage locally rather than asking.

## TypeSafe endpoint

| Endpoint | Purpose |
| --- | --- |
| `POST /v1/systemone` | Answer typed questions (choice, score, noul) about a state. |

This is TypeSafe's System One, served so a TypeSafe SDK reaches a local model with no change on its side. The SDK appends `v1/systemone` to its base URL, so the base URL carries no version segment, and the model is named because the SDK asks for `jev-latest` otherwise:

```sh
export TYPESAFE_BASE_URL=http://127.0.0.1:43367
export TYPESAFE_DEFAULT_MODEL=laya
```

```sh
curl http://127.0.0.1:43367/v1/systemone \
  -d '{
    "model": "laya",
    "questions": {"route": {"type": "choice",
      "instructions": "Which team should handle this?",
      "criteria": {"infra": "servers and networking",
                   "billing": "payments", "design": "visual and UX issues"}}},
    "state": "The server returned 500 three times in a row."
  }'
```

The response is the System One envelope, `{"model", "answers", "usage"}`, exactly as the model wrote it. The `questions` and `state` reach the model as the text you sent, never re-encoded, because the order of a choice's options changes the probabilities it gets.

The model has to be one that declares the `judge` capability (`hedos ls --capability judge`). A name that does not resolve, `jev-latest` included, is a `404` and a model that only chats is a `400`; both name the models that would do, and nothing else on the shelf answers in their place. A malformed request, or one the model refuses (laya raises when a question's options overrun its 192-token budget for them), is a `400` with the reason in `error.message`, which is where the SDK reads it. A bearer token is ignored like on every other route, so a real TypeSafe key in the environment does no harm. There is no streamed form. The SDK's default timeout is 10 seconds and a cold model takes longer than that to load, so `hedos warm <model>` first.

## Fidelity

The gateway serves each model's real behavior, not a lowest common denominator. It reads the model's context length, chat template, and tool-calling dialect and honors them. When a request asks for something a model cannot do (a capability it does not serve, or a parameter the runtime does not honor), the gateway returns a clear error in the dialect you used rather than pretending.

## Concurrency

The number of requests served at once is bounded by the `max_concurrent_inference` setting. Requests beyond that wait for a slot. This keeps a burst of clients from oversubscribing the machine's memory.
