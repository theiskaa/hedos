# The local endpoint

Hedos can serve your shelf to the rest of your machine over HTTP. The gateway is a **loopback-only** server (bound to `127.0.0.1`, default port `43367`) that speaks two familiar dialects — OpenAI-compatible under `/v1` and Ollama-compatible under `/api`. Anything on your Mac that already talks to either API can point at Hedos and reach the models you own, tools included. Nothing is exposed off the machine.

## Enabling it

Turn the gateway on in Settings → Gateway, where you also set the port and create **client tokens**. Every request must present a token; there is no anonymous access. A token looks like `hd_<id>.<secret>` and is sent as either an `Authorization: Bearer <token>` or an `x-api-key: <token>` header. Only a hash of the secret is stored — keep the token when it is shown, it is not recoverable.

A token can be **scoped** to specific models and capabilities when you create it. An unscoped token reaches the whole ready shelf; a scoped one is an exact allowlist, and a request outside it is refused with a 403.

## Talking to it

Base URL is `http://127.0.0.1:<port>`. The OpenAI surface:

```sh
# List the ready models
curl http://127.0.0.1:43367/v1/models \
  -H "Authorization: Bearer hd_xxx.yyy"

# Chat completion (add "stream": true for SSE)
curl http://127.0.0.1:43367/v1/chat/completions \
  -H "Authorization: Bearer hd_xxx.yyy" \
  -H "Content-Type: application/json" \
  -d '{"model": "qwen2.5", "messages": [{"role": "user", "content": "hello"}]}'
```

The Ollama surface mirrors the shapes those clients expect:

```sh
curl http://127.0.0.1:43367/api/tags -H "Authorization: Bearer hd_xxx.yyy"

curl http://127.0.0.1:43367/api/chat \
  -H "Authorization: Bearer hd_xxx.yyy" \
  -d '{"model": "qwen2.5", "messages": [{"role": "user", "content": "hello"}]}'
```

The `model` field is resolved against your ready models by id first, then alias, then name (case-insensitive), then a normalized tag — so `llama3:latest` finds a model named `llama3`. If a name is ambiguous, the response is a 400 listing the candidate ids so you can pick one.

## What it serves

The full route table:

- **OpenAI** — `POST /v1/chat/completions`, `POST /v1/completions`, `GET /v1/models`, `POST /v1/embeddings`, `POST /v1/audio/speech`, `POST /v1/audio/transcriptions`, `POST /v1/images/generations`
- **Ollama** — `POST /api/chat`, `POST /api/generate`, `POST /api/embed`, `POST /api/embeddings`, `GET /api/tags`, `GET /api/version`, `POST /api/show`

Chat streams over SSE on the OpenAI surface and NDJSON on the Ollama surface. Tool calls ride the chat routes on both dialects — OpenAI takes function arguments as a JSON string, Ollama as an object. Image inputs are accepted where a model can see: on the OpenAI surface as `image_url` `data:` URIs, on the Ollama surface as base64 `images` on a message. Inference routes share a concurrency cap (four by default); a request over the cap gets a `503` with `Retry-After` rather than being queued forever.

## Good to know

The gateway is deliberately narrow, and it tells you the truth at its edges rather than papering over them:

- **Nothing is fetched off your machine.** Image inputs on the OpenAI surface must be `data:` URIs — the gateway will not reach out for a remote `http(s)` image URL. This is by design, not a gap.
- **Ollama's finish reason is always `stop`.** That surface stays faithful to what stock Ollama clients expect, so a response cut off at the token limit reads the same as one that finished naturally. The OpenAI surface reports the real finish reason (`length` on truncation, `tool_calls` when a tool call streamed).
- **The OpenAI Responses API (`/v1/responses`) is not served.** Use `/v1/chat/completions`, which covers the same ground for local models.
