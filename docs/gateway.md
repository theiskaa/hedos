# Gateway

`hedos serve` runs a local HTTP server that speaks the OpenAI, Ollama, and Anthropic dialects. Point any tool that already talks to one of those at it, and it reaches the models on your shelf.

To run a coding harness against it without configuring anything, see [`hedos launch`](cli.md#launch), which serves a gateway for the life of the harness.

## Starting it

```sh
hedos serve            # binds 127.0.0.1:43367
hedos serve -p 8080    # a different port
```

The default port is `43367`, chosen to avoid colliding with Ollama's `11434`. The port also comes from your settings if you set one there. The server prints its base URL on startup and stops cleanly on Ctrl-C, SIGTERM, or SIGHUP, flushing its audit log on the way out, and every model server it started is stopped with it. It takes no new requests once told to stop:

- Ctrl-C waits for the requests still in flight, however long they take. A second Ctrl-C ends them.
- SIGTERM, or SIGHUP from a closed terminal, gives them 5 seconds and then ends them.
- An ended request that had no answer yet gets a `503` saying `the gateway is stopping`, in the dialect of its route. A streamed answer stops where it stands: its body is closed as a complete one, with no closing event (no `data: [DONE]`). Either way the gateway drops its request to the model: a model server still starting for it is stopped, a reply stops generating, and an embeddings batch sends no more of its inputs, with llama-server cancelling the ones it had not started. The audit log records each ended request as `503` with `the gateway is stopping`, a streamed one included, under its client and the model its body named.
- A gateway started ignoring SIGHUP, as `nohup hedos serve` starts it, keeps ignoring it, so it outlives the terminal.

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

The model has to be one that declares the `judge` capability (`hedos ls --capability judge`): a decision GGUF served by llama.cpp (clef, clef-flash, OpenJev, Kev, lev, Laya, Julia-1; llama.cpp 0.6.0 or newer), or a manifest runtime such as `python:laya` or `python:zerank`. A name that does not resolve, `jev-latest` included, is a `404` and a model that only chats is a `400`; both name the models that would do, and nothing else on the shelf answers in their place. A malformed request, or one the model refuses (laya raises when a question's options overrun its 192-token budget for them), is a `400` with the reason in `error.message`, which is where the SDK reads it. A bearer token is ignored like on every other route, so a real TypeSafe key in the environment does no harm. There is no streamed form. The SDK's default timeout is 10 seconds and a cold model takes longer than that to load, so `hedos warm <model>` first.

A decision GGUF reads its whole question in one batch of its window, which is the model's declared context capped at 16384 tokens. A question longer than that is a `400` naming the window (`the question takes 30138 tokens, more than the 16384-token window Clef-Flash-Q4_K_M is served with`). On a llama.cpp older than 0.6.0 the request fails with a message naming the version it needs.

Images go to a judge that sees (clef or OpenJev with its projector) in `images`, a list of image data URLs, or as `image_url` parts of a `state` made of chat messages:

```sh
curl http://127.0.0.1:43367/v1/systemone \
  -d '{
    "model": "Clef-Flash-Q4_K_M",
    "state": "The document accounting received this morning.",
    "questions": {"table": {"type": "noul", "instructions": "Does the image contain a table?"}},
    "images": ["data:image/png;base64,iVBORw0KGgo..."]
  }'
```

`images` that are not data URLs are a `400` before any model is asked, and images to a judge that does not see are a `400` naming the ones that do. A request on this route may be up to 32 MiB.

## Embeddings

`/v1/embeddings`, `/api/embed`, and `/api/embeddings` take any model that embeds (`hedos ls --capability embed`). A GGUF embedder is served by `llama-server`, started for embeddings:

- One request embeds at most 2048 inputs, as OpenAI allows; more is a `400` before any work. The gateway sends a batch to `llama-server` in pieces of at most 64 inputs and about one window of text (4 bytes for each token of the window; a longer input goes alone), so no one piece holds the model for more than a few seconds, and answers with one response, its vectors in input order and its token counts summed.
- A client that goes away ends its batch at once: the piece in flight is dropped, `llama-server` cancels the inputs of it that it had not started (the one it is reading finishes), and nothing more is sent. The audit log records the request as `cancelled` with status `499`, which `hedos stats` does not count as an error.
- The window is the one the model declares, capped at 8192 tokens. An input longer than an encoder's window is a `400` that names both: `the input is 9001 tokens, more than the 8192 this model embeds at once`. A decoder embedder that pools by its last token (Qwen3-Embedding) takes one token less than its window, 8191. Both counts include the model's special tokens. `/v1/models` reports the most one input may hold as `meta.n_ctx`: 8192 for a long encoder, 8191 for Qwen3-Embedding. In a batch, the message names the input by its index: `input 3 is 2102 tokens, more than the 2048 this model embeds at once`.
- The server of an encoder that llama.cpp keeps no cache for (BERT, nomic-bert, jina-bert, modern-bert, gemma-embedding, and similar) embeds several inputs at once, as `llama-server` sets it up. Any other embedder's server embeds one at a time: a decoder embedder (Qwen3-Embedding), and the encoders llama.cpp still gives a cache (`llama-embed`, `t5encoder`). Its inputs share one cache of the window and several at once overflow it, so concurrent requests to it queue.
- An over-long input is never truncated. Ollama's `/api/embed` truncates by default, but `llama-server` adds a model's special tokens (BERT's `[CLS]` and `[SEP]`, the end-of-text token a last-token embedder reads its vector from) only to text it tokenizes itself, so cutting the input to tokens in the gateway and sending it back cannot reproduce what the model expects to read. Split long text to fit the window before embedding it. The `truncate` parameter is refused on every model.

## Fidelity

The gateway serves each model's real behavior, not a lowest common denominator. It reads the model's context length, chat template, and tool-calling dialect and honors them. When a request asks for something a model cannot do (a capability it does not serve, or a parameter the runtime does not honor), the gateway returns a clear error in the dialect you used rather than pretending.

## Concurrency

The number of requests served at once is bounded by the `max_concurrent_inference` setting (4 by default). A request beyond that does not wait: it is refused at once with a `503` saying `too many requests are already running`, in the dialect of its route, and a `Retry-After: 1` header, so a client retries it. This keeps a burst of clients from oversubscribing the machine's memory.
