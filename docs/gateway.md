# Gateway

The gateway is a local HTTP server that speaks the OpenAI, Ollama, and Anthropic dialects, plus TypeSafe's System One for judges. Point any tool that already talks to one of those at it, and it reaches the models on your shelf. It binds to loopback only.

To run a coding harness against it without configuring anything, use `hedos launch`, which serves a gateway of its own for the life of the harness (see [Coding harnesses](#coding-harnesses)).

## Starting it

```sh
hedos serve            # binds 127.0.0.1:43367
hedos serve -p 8080    # a different port
```

The default port is `43367`, chosen to avoid colliding with Ollama's `11434`. Set `port` under `[gateway]` in your settings to change the default (see [configuration.md](configuration.md)); `-p` overrides both. On startup the server prints its base URL (`gateway listening on http://127.0.0.1:43367/v1`) and notes on stderr that auth is open on loopback. Under `--json` it prints `{"running": true, "port": …, "baseUrl": …}`.

### Stopping it

The gateway stops cleanly on Ctrl-C, SIGTERM, or SIGHUP. It takes no new requests once told to stop, flushes its audit log on the way out, and stops every model server it started.

- **Ctrl-C** waits for the requests still in flight, however long they take. A second Ctrl-C ends them.
- **SIGTERM**, or SIGHUP from a closed terminal, gives them 5 seconds and then ends them.
- **An ended request** that had no answer yet gets a `503` saying `the gateway is stopping`, in the dialect of its route. A streamed answer stops where it stands: its body is closed as a complete one, with no closing event (no `data: [DONE]`). Either way the gateway drops its request to the model: a model server still starting for it is stopped, a reply stops generating, and an embeddings batch sends no more of its inputs, with `llama-server` cancelling the ones it had not started. The audit log records each ended request as `503` with `the gateway is stopping`, a streamed one included, under its client and the model its body named.
- **`nohup hedos serve`** starts the gateway ignoring SIGHUP, and it keeps ignoring it, so it outlives the terminal.

## Endpoints

| Method | Path | Dialect | Notes |
| --- | --- | --- | --- |
| `POST` | `/v1/chat/completions` | OpenAI | Chat, streamed or whole, with tool calling and images. |
| `POST` | `/v1/completions` | OpenAI | Prompt completion. |
| `POST` | `/v1/embeddings` | OpenAI | Embed text. `encoding_format` is `float` or `base64`. |
| `POST` | `/v1/images/generations` | OpenAI | One image per request, returned as `b64_json`. |
| `POST` | `/v1/audio/speech` | OpenAI | Synthesize speech, returned as WAV. |
| `POST` | `/v1/audio/transcriptions` | OpenAI | Transcribe an uploaded WAV file (multipart). |
| `GET` | `/v1/models` | OpenAI | Every ready model, with its context window as `meta.n_ctx`. |
| `POST` | `/v1/messages` | Anthropic | Chat over the Messages protocol, with tool use. |
| `POST` | `/v1/systemone` | TypeSafe | Typed questions (choice, score, noul) to a judge. See [Judges](#judges). |
| `POST` | `/v1/extract` | hedos | Find contacts in text with an extractor. See [Extractors](#extractors). |
| `POST` | `/api/chat` | Ollama | Chat over Ollama's NDJSON protocol, with tools. |
| `POST` | `/api/generate` | Ollama | Prompt generation. |
| `POST` | `/api/embed` | Ollama | Embed text. |
| `POST` | `/api/embeddings` | Ollama | Embed one `prompt` (the legacy endpoint). |
| `GET` | `/api/tags` | Ollama | The ready models that chat. |
| `GET` | `/api/ps` | Ollama | The models held in memory. Each entry also carries hedos's record `id`, and `expires_at` when an idle unload is armed. |
| `GET` | `/api/version` | Ollama | Version handshake for stock clients (reports `0.5.0`). |
| `POST` | `/api/show` | Ollama | Model details handshake. Takes `model` (or `name`) and lists the model's `capabilities` (`completion`, `embedding`, `vision`, `tools`). |

Any other path is a `404` (`no route for /api/pull`), and a known path with the wrong method is a `405`. A request body may be up to 2 MiB, or 32 MiB on `/v1/systemone` and `/v1/audio/transcriptions`; a larger one is a `413`. The body is read as JSON whatever its `content-type` says (transcriptions excepted, which need `multipart/form-data`).

Errors come back in the shape of the route's dialect:

| Dialect | Error body |
| --- | --- |
| OpenAI (and `/v1/systemone`, `/v1/extract`) | `{"error": {"message": "…", "type": "…", "code": "…"}}` |
| Ollama | `{"error": "…"}` |
| Anthropic | `{"type": "error", "error": {"type": "…", "message": "…"}}` |

### Model names

The `model` in a request can be a record id, an alias, or a name. An exact id wins outright. Otherwise the gateway tries, in order, the alias, the exact name, the name ignoring case, and the name ignoring case and a trailing `:latest`, so `Llama3:latest` finds `llama3`. Only ready models count. A name that matches nothing is a `404` (`no ready model matches qwen9`), and one that matches several models at the same step is a `400` that lists their ids, so you can send one of those instead. `/v1/models` and `/api/tags` list each model under its alias when it has one, else its name, which is the name to send back.

## Examples

### OpenAI

```sh
curl http://127.0.0.1:43367/v1/chat/completions \
  -d '{
    "model": "qwen2.5",
    "messages": [{"role": "user", "content": "hi"}]
  }'
```

```json
{
  "id": "chatcmpl-6c1f0d4e9a2b7c3e5f8a1d2b4c6e8f0a",
  "object": "chat.completion",
  "created": 1759838400,
  "model": "qwen2.5",
  "choices": [{
    "index": 0,
    "message": {"role": "assistant", "content": "Hello! How can I help you today?"},
    "finish_reason": "stop"
  }],
  "usage": {"prompt_tokens": 9, "completion_tokens": 10, "total_tokens": 19}
}
```

With `"stream": true` the answer arrives as server-sent events, the shape OpenAI clients expect. The last chunk carries the `finish_reason`, and `data: [DONE]` closes the stream:

```text
data: {"id":"chatcmpl-…","object":"chat.completion.chunk","created":1759838400,"model":"qwen2.5","choices":[{"index":0,"delta":{"role":"assistant","content":"Hello"},"finish_reason":null}]}

data: {"id":"chatcmpl-…","object":"chat.completion.chunk","created":1759838400,"model":"qwen2.5","choices":[{"index":0,"delta":{"content":"!"},"finish_reason":null}]}

data: {"id":"chatcmpl-…","object":"chat.completion.chunk","created":1759838400,"model":"qwen2.5","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}

data: [DONE]
```

### Ollama

```sh
curl http://127.0.0.1:43367/api/chat \
  -d '{
    "model": "qwen2.5",
    "messages": [{"role": "user", "content": "hi"}],
    "stream": false
  }'
```

```json
{
  "model": "qwen2.5",
  "created_at": "2026-10-07T12:00:00Z",
  "message": {"role": "assistant", "content": "Hello! How can I help you today?"},
  "done": true,
  "done_reason": "stop",
  "total_duration": 412000000,
  "prompt_eval_count": 9,
  "eval_count": 10
}
```

Like Ollama itself, `/api/chat` and `/api/generate` stream unless told `"stream": false`. A stream is newline-delimited JSON, one object per line with `"done": false`, ending in a line with `"done": true`, which is what stock Ollama clients read.

### Anthropic

This is the dialect Claude Code speaks, and it exists so `hedos launch claude` works. The base URL carries no version segment, because the client appends `/v1/messages` itself:

```sh
curl http://127.0.0.1:43367/v1/messages \
  -H 'content-type: application/json' \
  -d '{
    "model": "qwen2.5",
    "max_tokens": 1024,
    "messages": [{"role": "user", "content": "hi"}]
  }'
```

```json
{
  "id": "msg_3a9e1c5b7d2f4a6c8e0b1d3f5a7c9e2b",
  "type": "message",
  "role": "assistant",
  "model": "qwen2.5",
  "content": [{"type": "text", "text": "Hello! How can I help you today?"}],
  "stop_reason": "end_turn",
  "stop_sequence": null,
  "usage": {"input_tokens": 9, "output_tokens": 10}
}
```

A few things differ from Anthropic's own API:

- Thinking is not sent as a `thinking` block, because those carry a signature this gateway cannot issue and clients replay the blocks they receive.
- `/v1/messages/count_tokens` is not served, so Claude Code estimates context usage locally rather than asking.
- Image blocks are not read. A turn that held only content the gateway cannot read, a pasted screenshot for instance, stays in the conversation as a placeholder (`[unsupported content: image]`) rather than vanishing.
- Unknown top-level fields are ignored rather than refused, since Claude Code sends fields such as `thinking`, `context_management`, `output_config`, and `cache_control` to every model it does not recognize.

## Pointing tools at it

Every client needs only a base URL. The gateway ignores API keys (see [Auth and loopback](#auth-and-loopback)), but most SDKs refuse to start without one, so set any placeholder.

| Client | Base URL | Typical setting |
| --- | --- | --- |
| OpenAI SDKs and OpenAI-compatible tools | `http://127.0.0.1:43367/v1` | `OPENAI_BASE_URL`, or `base_url=` in code |
| Ollama clients | `http://127.0.0.1:43367` | `OLLAMA_HOST` |
| Anthropic SDKs | `http://127.0.0.1:43367` (no `/v1`) | `ANTHROPIC_BASE_URL` |
| TypeSafe SDKs | `http://127.0.0.1:43367` (no `/v1`) | `TYPESAFE_BASE_URL` and `TYPESAFE_DEFAULT_MODEL` |

```python
from openai import OpenAI

client = OpenAI(base_url="http://127.0.0.1:43367/v1", api_key="hedos")
reply = client.chat.completions.create(
    model="qwen2.5",
    messages=[{"role": "user", "content": "hi"}],
)
```

An Ollama client gets the routes in the table above and nothing else. Managing models through the Ollama API (`/api/pull`, `/api/delete`, `/api/create`) is not served; use `hedos pull` and `hedos rm` instead.

### Coding harnesses

`hedos launch <harness>` starts a gateway on a free port in its own process, wires the harness to it, and stops the gateway when the harness exits, passing on the harness's exit code. There is nothing to start first and no port to collide with. Name the model with `-m`, or pick one; pass arguments through to the harness after `--`.

| Harness | Dialect | How it is wired |
| --- | --- | --- |
| `claude` (Claude Code) | Anthropic | `ANTHROPIC_BASE_URL`, `ANTHROPIC_AUTH_TOKEN`, `ANTHROPIC_MODEL`, and `CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS=1` |
| `opencode` | OpenAI | an inline config in `OPENCODE_CONFIG_CONTENT`, so your own `opencode.json` is neither read nor written |
| `aider` | OpenAI | `OPENAI_API_BASE`, `OPENAI_API_KEY`, and `--model openai/<model>` |
| `goose` | OpenAI | `GOOSE_PROVIDER=openai`, `OPENAI_HOST`, and `OPENAI_BASE_PATH=v1/chat/completions` |
| `crush` | OpenAI | a generated `crush.json`, named by `CRUSH_GLOBAL_CONFIG` |

Every harness but aider drives the model through tool calls, so the picker offers only models with the `tools` capability for them. See [cli.md](cli.md#hedos-launch) for the full `hedos launch` reference.

## Judges

`POST /v1/systemone` is TypeSafe's System One, served so a TypeSafe SDK reaches a local model with no change on its side. The SDK appends `v1/systemone` to its base URL, so the base URL carries no version segment, and the model must be named, because the SDK asks for `jev-latest` otherwise:

```sh
export TYPESAFE_BASE_URL=http://127.0.0.1:43367
export TYPESAFE_DEFAULT_MODEL=laya
```

The model has to be one that declares the `judge` capability (`hedos ls --capability judge`): a decision GGUF served by llama.cpp (clef, clef-flash, OpenJev, Kev, lev, Laya, Julia-1; llama.cpp 0.6.0 or newer), or a manifest runtime such as `python:laya` or `python:zerank`. See [models.md](models.md#judges) for what each one is.

### The request

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

| Field | Required | What it holds |
| --- | --- | --- |
| `model` | yes | A judge's name, alias, or id. |
| `questions` | yes | An object of at least one question, keyed by an id of your choosing. Each has a `type` (`choice`, `score`, or `noul`) and `instructions`. A `choice` needs `criteria` to choose between: an object of label to description, or a list of labels. A `score` needs a list of levels as `criteria`. A `noul`'s criteria are optional. |
| `state` | yes | What the questions are about: any JSON, `null` included. It may also be a list of chat messages, or an object holding them under `messages`. |
| `images` | no | A list of image data URLs (`data:image/...;base64,...`), for a judge that sees. `null` means none. |

The `questions` and `state` reach the model as the text you sent, never re-encoded, because the order of a choice's options changes the probabilities it gets.

Images go to a judge that sees (clef or OpenJev with its projector) in `images`, or as `image_url` parts of a `state` made of chat messages:

```sh
curl http://127.0.0.1:43367/v1/systemone \
  -d '{
    "model": "Clef-Flash-Q4_K_M",
    "state": "The document accounting received this morning.",
    "questions": {"table": {"type": "noul", "instructions": "Does the image contain a table?"}},
    "images": ["data:image/png;base64,iVBORw0KGgo..."]
  }'
```

### The answer

The response is the System One envelope, `{"model", "answers", "usage"}`, exactly as the model wrote it, with an `x-typesafe-request-id` header. Each question's answer sits under its id in `answers`. A typical one looks like this (the probabilities are illustrative):

```json
{
  "model": "laya",
  "answers": {
    "route": {
      "choice": "infra",
      "probabilities": {"infra": 0.9037, "billing": 0.0466, "design": 0.0498},
      "confidence": 0.8
    }
  },
  "usage": {"input_tokens": 58, "output_tokens": 0}
}
```

A `score` answers with its expected `score` and a probability per level (`{"score": 0.8, "probabilities": {"0": 0.2, "1": 0.8}}`), and a `noul` with the probability that it holds (`{"noul": 0.73}`). `confidence` appears when the model reports one.

### What it refuses

| Status | When |
| --- | --- |
| `404` | The name does not resolve, `jev-latest` included. The message names the judges that would do. |
| `400` | The name resolves to a model that does not answer typed questions, a chat model for instance. The message names the judges that would do. Nothing else on the shelf answers in their place. |
| `400` | The request is malformed: a missing field, an unknown question type, a choice without criteria, an image that is not a data URL. These are refused before any model is asked. |
| `400` | The model refuses the question, for example when laya finds a question's options overrun its 192-token budget for them. The reason is in `error.message`, which is where the SDK reads it. |
| `400` | The question does not fit the window (see below), or carries images to a judge that does not see; the latter names the judges that do. |
| `500` | The model's reply was not a System One envelope. |

A bearer token is ignored, as on every other route, so a real TypeSafe key in the environment does no harm. There is no streamed form.

A decision GGUF reads its whole question in one batch of its window, which is the model's declared context capped at 16384 tokens. A question longer than that is a `400` naming the window (`the question takes 30138 tokens, more than the 16384-token window Clef-Flash-Q4_K_M is served with`). On a llama.cpp older than 0.6.0 the request fails with a message naming the version it needs.

The SDK's default timeout is 10 seconds and a cold model takes longer than that to load, so run `hedos warm <model>` first. With a gateway running, `hedos warm` asks a judge its probe on this route, so the gateway's own copy is the one loaded.

## Extractors

`POST /v1/extract` hands a text to an extractor such as [Tessera](models.md#extractors) and returns what it found. Neither OpenAI nor Ollama has a route for it, so the request and the answer are Tessera's own: the body is the request `tessera json` reads, plus the `model` to ask, and the answer is the one JSON object Tessera writes, byte for byte.

```sh
curl http://127.0.0.1:43367/v1/extract -d '{
  "model": "tessera",
  "operation": "contacts",
  "text": "Jordan Lee, 123 Main St, Bismarck, ND 58501, (701) 555-0142, jordan@acme.example"
}'
```

| Field | Value |
| --- | --- |
| `model` | The extractor to ask; required. |
| `operation` | `detect`, `contacts`, or `address`; required. |
| `text` | The document, or for `address` the one address; required. |
| `kinds` | Any of `person`, `org`, `address`, `email`, `phone`; every kind when left out. |
| `country_hint` | Region codes such as `["US"]`; when left out, the region the model was trained for. |
| `include_uncertain` | Return low-confidence results too; `false` by default. |
| `offsets` | `utf8` bytes (the default) or `utf16` code units for every `start` and `end`. |

The answer has `model` and `operation`, then `entities` for `detect`, `contacts` and `unassigned` for `contacts`, or `address` with its `components` for `address`. Each entity carries its `kind`, `text`, `start` and `end`, `confidence`, `review_recommended`, its `source`, and a `normalized` form for phones (E.164) and emails.

| Status | When |
| --- | --- |
| `404` | The name does not resolve. The message names the extractors that would do. |
| `400` | The name resolves to a model that does not extract, or the body is not a JSON object with a `model`. |
| `400` | The extractor refuses the request: an unknown field or value, an `address` request without the `address` kind, a text too large. Its reason is in `error.message`. |
| `500` | The model could not be read, or its reply was not one JSON object. |

An extractor reads its model for each request and keeps nothing between them, so there is nothing to warm and the first request is as quick as the rest.

## Tool calling

All three chat dialects carry tools. A request that offers tools to a model without the `tools` capability is a `400` (`<model> does not support tool calling`); `hedos ls --capability tools` lists the ones that have it.

- **OpenAI.** Send `tools` and, optionally, `tool_choice` (a string or an object; `"none"` removes the tools entirely). Earlier turns carry `tool_calls` on assistant messages and `tool_call_id` on `tool` messages; a call's `arguments` may be a JSON-encoded string or an object. The answer's `message.tool_calls` holds the calls, with `finish_reason` `tool_calls` and `content` `null` when there is no text. Streamed, each call arrives whole in one chunk, with its `index`.
- **Ollama.** Send `tools`. Calls come back in `message.tool_calls` with their `arguments` as an object, and `done_reason` stays `stop`, as Ollama reports it.
- **Anthropic.** Send `tools` and, optionally, `tool_choice`: `auto`, `any` (a tool must be called), `tool` with a `name`, or `none` (the model never sees the tools). Calls come back as `tool_use` blocks with `stop_reason` `tool_use`, which wins over any other stop reason so an agent loop keeps going. Streamed, each call is one `content_block_start`, one `input_json_delta` holding its whole input, and a `content_block_stop`. `tool_result` blocks in later turns become tool messages for the model.

## Streaming

| Dialect | Default | Format | Ends with |
| --- | --- | --- | --- |
| OpenAI | whole answer; `"stream": true` streams | server-sent events, `data:` frames | a chunk carrying `finish_reason`, then `data: [DONE]` |
| Ollama | streams; `"stream": false` for one object | newline-delimited JSON | a line with `"done": true` and the stats |
| Anthropic | whole answer; `"stream": true` streams | server-sent events with `event:` names | `message_delta`, then `message_stop`; no `[DONE]` sentinel |

- **OpenAI.** `stream_options.include_usage` adds a usage frame (empty `choices`, then `usage`) before `[DONE]`. A model's reasoning streams as `reasoning_content` in the delta.
- **Ollama.** `"think": true` asks a model to think, and its thinking streams in `message.thinking`.
- **Anthropic.** The grammar is `message_start`, then a `content_block_start` / `content_block_delta` / `content_block_stop` group per content block, then `message_delta` and `message_stop`.

A streamed chat on `/v1/chat/completions`, `/api/chat`, or `/v1/messages` may run for 10 minutes. Past that it ends with an in-band error, `the request timed out after 600s`. A failure after the stream has begun is also sent in-band, since the status line has already gone: an error frame followed by `[DONE]` on OpenAI, an `{"error": …}` line on Ollama, an `error` event on Anthropic. Image generation is cut off at 10 minutes and speech at 5, each with a `504`.

## Request details

### Parameters

The gateway serves each model's real behaviour rather than a lowest common denominator. It reads the model's context length, chat template, and tool-calling dialect and honours them. When a request asks for something the model cannot do, it gets a clear error in its own dialect rather than a quiet approximation.

- **OpenAI chat** accepts `temperature`, `top_p`, `max_tokens` (or `max_completion_tokens`), `stop` (up to 4), `seed`, `frequency_penalty`, `presence_penalty`, `response_format` (`text`, `json_object`, or `json_schema`), `tools`, `tool_choice`, `stream`, `stream_options`, and `user`. `n` above 1 is refused, and an empty `logit_bias` is tolerated. Any other key is a `400` with code `unsupported_parameter`. `/v1/completions` takes the same sampling keys around a `prompt`, and refuses `best_of` above 1.
- **Ollama** accepts `stream`, `think`, `format` (`"json"` or a JSON schema), and `options` on both `/api/chat` and `/api/generate`, plus `tools` and `keep_alive` (accepted and ignored) on `/api/chat`. Any other top-level key is refused. The `options` it reads are `temperature`, `top_p`, `top_k`, `min_p`, `num_predict`, `num_ctx`, `seed`, `repeat_penalty`, `frequency_penalty`, `presence_penalty`, and `stop`; any other option is refused.
- **Anthropic** reads `max_tokens`, `top_k`, `temperature`, `top_p`, `stop_sequences`, `system` (a string or text blocks), `tools`, and `tool_choice`.
- **Every dialect** then checks the sampling parameters against the runtime serving the model. One the runtime does not honour is a `400` naming it: `the parameter 'top_k' is not supported by the llama-cpp runtime serving this model`.

### Images in chat

A model that sees (`hedos ls --capability see`) reads images sent as `image_url` content parts on `/v1/chat/completions`, or as base64 `images` on an `/api/chat` message. OpenAI image URLs must be base64 `data:` URIs: the gateway fetches nothing off the machine. Every image counts toward the 2 MiB body limit.

### Images, speech, and transcription

- **`/v1/images/generations`** needs a `model` and a `prompt`, passes `size` through, and returns one image as `b64_json`. `n` above 1 and any `response_format` other than `b64_json` are refused, since images never leave the machine as URLs.
- **`/v1/audio/speech`** needs a `model` and an `input`, takes an optional `voice` (the model's first voice when omitted) and `speed`, and returns `audio/wav`. A `response_format` other than `wav` is refused.
- **`/v1/audio/transcriptions`** takes a `multipart/form-data` upload with a `model` and a `file` that is a RIFF WAVE file, and answers `{"text": …}`, or plain text with `response_format=text`. `language`, `prompt`, `temperature`, and `timestamp_granularities` are refused rather than ignored.

### Embeddings

`/v1/embeddings`, `/api/embed`, and `/api/embeddings` take any model that embeds (`hedos ls --capability embed`). `/v1/embeddings` and `/api/embed` take one text or a list; the legacy `/api/embeddings` takes a single `prompt`. Token-array input, `dimensions`, and (on the Ollama routes) `truncate` are refused. A model with no embeddings runtime on this machine is a `501`.

A GGUF embedder is served by `llama-server`, started for embeddings:

- One request embeds at most 2048 inputs, as OpenAI allows; more is a `400` before any work. The gateway sends a batch to `llama-server` in pieces of at most 64 inputs and about one window of text (4 bytes for each token of the window; a longer input goes alone), so no one piece holds the model for more than a few seconds, and answers with one response, its vectors in input order and its token counts summed.
- A client that goes away ends its batch at once: the piece in flight is dropped, `llama-server` cancels the inputs of it that it had not started (the one it is reading finishes), and nothing more is sent. The audit log records the request as `cancelled` with status `499`, which `hedos stats` does not count as an error.
- The window is the one the model declares, capped at 8192 tokens. An input longer than an encoder's window is a `400` that names both: `the input is 9001 tokens, more than the 8192 this model embeds at once`. A decoder embedder that pools by its last token (Qwen3-Embedding) takes one token less than its window, 8191. Both counts include the model's special tokens. `/v1/models` reports the most one input may hold as `meta.n_ctx`: 8192 for a long encoder, 8191 for Qwen3-Embedding. In a batch, the message names the input by its index: `input 3 is 2102 tokens, more than the 2048 this model embeds at once`.
- The server of an encoder that llama.cpp keeps no cache for (BERT, nomic-bert, jina-bert, modern-bert, gemma-embedding, and similar) embeds several inputs at once, as `llama-server` sets it up. Any other embedder's server embeds one at a time: a decoder embedder (Qwen3-Embedding), and the encoders llama.cpp still gives a cache (`llama-embed`, `t5encoder`). Its inputs share one cache of the window and several at once overflow it, so concurrent requests to it queue.
- An over-long input is never truncated. Ollama's `/api/embed` truncates by default, but `llama-server` adds a model's special tokens (BERT's `[CLS]` and `[SEP]`, the end-of-text token a last-token embedder reads its vector from) only to text it tokenizes itself, so cutting the input to tokens in the gateway and sending it back cannot reproduce what the model expects to read. Split long text to fit the window before embedding it.

## Concurrency

The number of inference requests served at once is bounded by `max_concurrent_inference` under `[gateway]` in your settings (4 by default). Listing and handshake routes do not count. A request beyond the bound does not wait: it is refused at once with a `503` saying `too many requests are already running`, in the dialect of its route, and a `Retry-After: 1` header, so a client retries it. This keeps a burst of clients from oversubscribing the machine's memory.

A request can also be turned away because the memory governor would have to wait for another model to make room (`the machine is busy with another model`, `Retry-After: 1`), or, for image generation, because 4 jobs are already queued (`Retry-After: 5`). Both are `503`s.

## Auth and loopback

The gateway binds to `127.0.0.1` and treats every local caller as trusted. It does not require a token, and it ignores one sent as `Authorization: Bearer …` or `x-api-key`. This keeps local tools frictionless on a single-user machine.

It also means the loopback boundary is the security boundary: anything that can reach the port can use every model on your shelf. Do not bind it to a public interface or place a proxy in front of it. See [SECURITY.md](../SECURITY.md).

## The audit log and `hedos stats`

Every request is recorded, one JSON object per line, in `audit.jsonl` under the data directory's `gateway/` folder (`~/.local/share/hedos/gateway/`, or `$XDG_DATA_HOME/hedos/gateway/`). The folder is readable by your account only. The log rotates at 5 MiB, keeping two older files beside it (`audit.1.jsonl`, `audit.2.jsonl`).

```json
{"ts":"2026-10-07T18:07:06Z","client":"local","clientName":"local","method":"POST","route":"/v1/chat/completions","model":"57f8c00299d1d336","capability":"chat","outcome":"ok","status":200,"durationMs":514}
```

- `model` is the record id of the model that served it, and `capability` what it was asked to do. A refused request has neither, except one the gateway's stop ended, which names the model its body asked for.
- `outcome` is `ok`, `cancelled` (its client went away, status `499`), or why it failed: `bad_request`, `unauthorized`, `forbidden`, `not_found`, `not_supported`, `saturated`, `timeout`, or `error`.
- `detail` appears on a server error (its message), a cancelled request (why it ended, which its client is never sent), and a request the gateway's stop ended (`the gateway is stopping`).
- The log records no token counts.

`hedos stats` reads every generation of the log back and reports the total request count, how many were rejected (any outcome but `ok` and `cancelled`), and per model, by record id, the request count, the error count and rate, the p50, p90, and p99 latency of its successful requests, and when it was last seen. A refused request has no model, so it counts in the totals but under no model's row. `--json` prints the full summary. With no log yet, it says so and exits `0`.

## Troubleshooting

**`could not bind 127.0.0.1:43367`.** Another gateway (or another program) holds the port. Stop it, or start this one with `-p`.

**`no ready model matches …` (404).** The name matches no ready model. Check `hedos ls` or `GET /v1/models` for the name to send. A model whose weights are gone, or that no runtime could resolve, is not ready.

**`… matches more than one model` (400).** Two models answer to the name at the same step. Send one of the ids the message lists.

**`the parameter '…' is not supported` (400, `unsupported_parameter`).** The dialect or the model's runtime does not honour that parameter. Drop it from the request.

**`… does not support tool calling` (400).** Pick a model with tools (`hedos ls --capability tools`), or send no tools.

**`413 request body too large`.** The body is over 2 MiB (32 MiB on `/v1/systemone` and transcriptions). Base64 images count toward it.

**`503` with `Retry-After`.** Too many requests are running, or the machine is busy with another model. Retry, raise `max_concurrent_inference`, or look at the governor settings in [configuration.md](configuration.md).

**An Ollama client cannot pull or delete.** Only the routes in the table are served. Use `hedos pull` and `hedos rm`.

**A TypeSafe SDK times out on the first call.** The model is loading. Run `hedos warm <model>` first.

**A decision model fails with a message about llama.cpp 0.6.0.** The `llama-server` on your `PATH` is too old to serve decision models. Upgrade it (`brew upgrade llama.cpp`).

**A `500` with a runtime's message.** The runtime's own error is passed through (a stopped daemon, a model out of memory) because it is the only thing that explains the failure. The audit log keeps the same text under `detail`.
