//! The Ollama daemon adapter: serves chat/vision/embeddings by talking to a
//! local Ollama server over HTTP. `/api/chat` streams newline-delimited JSON
//! that maps to `CapabilityChunk`s; `/api/embed` returns embeddings in one shot.

use std::collections::{BTreeMap, HashSet};

use kernel::capabilities::{CapabilityChunk, GenerationStats, ToolCall};
use kernel::records::{
    BidPreference, Capability, JsonValue, ModelRecord, RunTier, RuntimeId, SourceKind,
};
use kernel::resolution::{IdentifiedModel, ModelFormat, RuntimeBid};
use tokio::sync::mpsc;

use super::{ChunkStream, RuntimeAdapter, RuntimeError};

const DEFAULT_BASE_URL: &str = "http://127.0.0.1:11434";
const NOT_RUNNING_HINT: &str = "Ollama isn't running. Start it with `ollama serve`.";
const MAX_EMBED_BYTES: usize = 32 * 1024 * 1024;
const MAX_ERROR_BYTES: usize = 64 * 1024;

/// `(payload key, ollama option key)` — the request params Ollama honors.
const OPTION_KEYS: [(&str, &str); 11] = [
    ("temperature", "temperature"),
    ("top_p", "top_p"),
    ("top_k", "top_k"),
    ("min_p", "min_p"),
    ("max_tokens", "num_predict"),
    ("context_length", "num_ctx"),
    ("stop", "stop"),
    ("seed", "seed"),
    ("repeat_penalty", "repeat_penalty"),
    ("frequency_penalty", "frequency_penalty"),
    ("presence_penalty", "presence_penalty"),
];

/// Serves models through a local Ollama daemon.
pub struct OllamaAdapter {
    id: RuntimeId,
    base_url: String,
    client: reqwest::Client,
}

impl OllamaAdapter {
    /// An adapter pointed at the default local Ollama (`127.0.0.1:11434`).
    pub fn new() -> Self {
        Self::with_base_url(DEFAULT_BASE_URL)
    }

    /// An adapter pointed at `base_url` (no trailing slash).
    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        Self {
            id: RuntimeId::ollama(),
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            client: reqwest::Client::new(),
        }
    }

    fn invoke_chat(&self, model: &str, payload: JsonValue) -> ChunkStream {
        let (tx, stream) = ChunkStream::channel();
        let client = self.client.clone();
        let base_url = self.base_url.clone();
        let url = format!("{}/api/chat", self.base_url);
        let model = model.to_owned();
        tokio::spawn(async move {
            let response =
                match send_with_autostart(&client, &base_url, &url, || chat_body(&model, &payload))
                    .await
                {
                    Ok(response) => response,
                    Err(err) => {
                        let _ = tx.send(Err(err));
                        return;
                    }
                };
            stream_chat(response, &tx).await;
        });
        stream
    }

    fn invoke_embed(&self, model: &str, payload: JsonValue) -> ChunkStream {
        let (tx, stream) = ChunkStream::channel();
        let client = self.client.clone();
        let base_url = self.base_url.clone();
        let url = format!("{}/api/embed", self.base_url);
        let model = model.to_owned();
        tokio::spawn(async move {
            let response = match send_with_autostart(&client, &base_url, &url, || {
                embed_body(&model, &payload)
            })
            .await
            {
                Ok(response) => response,
                Err(err) => {
                    let _ = tx.send(Err(err));
                    return;
                }
            };
            stream_embed(response, &tx).await;
        });
        stream
    }
}

impl Default for OllamaAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl RuntimeAdapter for OllamaAdapter {
    fn id(&self) -> &RuntimeId {
        &self.id
    }

    fn wires_tools(&self) -> bool {
        true
    }

    fn can_serve(&self, record: &ModelRecord, capability: &Capability) -> bool {
        let served = *capability == Capability::chat()
            || *capability == Capability::complete()
            || *capability == Capability::embed()
            || *capability == Capability::see();
        if !served {
            return false;
        }
        if *capability == Capability::see() && !record.capabilities.contains(&Capability::see()) {
            return false;
        }
        if *capability != Capability::embed() && !record.capabilities.contains(&Capability::chat())
        {
            return false;
        }
        match &record.runtime.id {
            Some(runtime_id) => *runtime_id == self.id,
            None => record.source.kind == SourceKind::ollama(),
        }
    }

    fn bid(&self, _record: &ModelRecord, identified: &IdentifiedModel) -> Option<RuntimeBid> {
        if identified.format != ModelFormat::OllamaStore {
            return None;
        }
        Some(RuntimeBid::new(RunTier::Native, BidPreference::OLLAMA))
    }

    fn invoke(
        &self,
        record: &ModelRecord,
        capability: Capability,
        payload: JsonValue,
    ) -> ChunkStream {
        if capability == Capability::embed() {
            self.invoke_embed(&record.name, payload)
        } else {
            self.invoke_chat(&record.name, payload)
        }
    }

    fn effective_context_window(
        &self,
        record: &ModelRecord,
        requested: Option<i64>,
    ) -> Option<i64> {
        requested.or(record.context_length)
    }

    fn honored_param_keys(
        &self,
        _record: &ModelRecord,
        capability: &Capability,
    ) -> HashSet<String> {
        if *capability != Capability::chat() && *capability != Capability::complete() {
            return HashSet::new();
        }
        // The honored set is exactly what `chat_body` forwards (`OPTION_KEYS`)
        // plus `response_format`, derived so the two can't drift.
        OPTION_KEYS
            .iter()
            .map(|(payload_key, _)| *payload_key)
            .chain(["response_format"])
            .map(str::to_owned)
            .collect()
    }
}

async fn stream_chat(
    mut response: reqwest::Response,
    tx: &mpsc::UnboundedSender<Result<CapabilityChunk, RuntimeError>>,
) {
    if response.status() != reqwest::StatusCode::OK {
        let code = response.status().as_u16();
        let body = capped_body(&mut response, MAX_ERROR_BYTES)
            .await
            .unwrap_or_default();
        let _ = tx.send(Err(RuntimeError::Failed(http_error_message(&body, code))));
        return;
    }

    super::line_stream::read_lines(response, tx, "ollama", |line| forward_line(tx, line)).await;
}

/// Parse one ndjson line and forward its chunks. Returns `false` to stop (an
/// error line, `done`, or the consumer having gone away).
fn forward_line(
    tx: &mpsc::UnboundedSender<Result<CapabilityChunk, RuntimeError>>,
    line: &[u8],
) -> bool {
    let Ok(value) = serde_json::from_slice::<JsonValue>(line) else {
        return true;
    };
    let Some(object) = value.as_object() else {
        return true;
    };

    if let Some(message) = object.get("error").and_then(JsonValue::as_str)
        && !message.is_empty()
    {
        let _ = tx.send(Err(RuntimeError::Failed(format!("ollama: {message}"))));
        return false;
    }

    for call in tool_calls(object) {
        if tx.send(Ok(CapabilityChunk::ToolCall(call))).is_err() {
            return false;
        }
    }

    if object.get("done").and_then(JsonValue::as_bool) == Some(true) {
        let _ = tx.send(Ok(CapabilityChunk::Done(Some(done_stats(object)))));
        return false;
    }

    if let Some(message) = object.get("message").and_then(JsonValue::as_object) {
        if let Some(thinking) = message.get("thinking").and_then(JsonValue::as_str)
            && !thinking.is_empty()
            && tx
                .send(Ok(CapabilityChunk::Thinking(thinking.to_owned())))
                .is_err()
        {
            return false;
        }
        if let Some(content) = message.get("content").and_then(JsonValue::as_str)
            && !content.is_empty()
            && tx
                .send(Ok(CapabilityChunk::Text(content.to_owned())))
                .is_err()
        {
            return false;
        }
    }
    true
}

fn tool_calls(object: &BTreeMap<String, JsonValue>) -> Vec<ToolCall> {
    let Some(entries) = object
        .get("message")
        .and_then(JsonValue::as_object)
        .and_then(|message| message.get("tool_calls"))
        .and_then(JsonValue::as_array)
    else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|entry| {
            let function = entry.as_object()?.get("function")?.as_object()?;
            let name = function.get("name").and_then(JsonValue::as_str)?;
            if name.is_empty() {
                return None;
            }
            let arguments = function
                .get("arguments")
                .filter(|args| matches!(args, JsonValue::Object(_)))
                .cloned()
                .unwrap_or_else(empty_object);
            Some(ToolCall::new(name, arguments))
        })
        .collect()
}

fn done_stats(object: &BTreeMap<String, JsonValue>) -> GenerationStats {
    let millis = |key: &str| {
        object
            .get(key)
            .and_then(JsonValue::as_i64)
            .map(|ns| ns / 1_000_000)
    };
    GenerationStats {
        prompt_tokens: object.get("prompt_eval_count").and_then(JsonValue::as_i64),
        completion_tokens: object.get("eval_count").and_then(JsonValue::as_i64),
        duration_ms: millis("total_duration"),
        load_ms: millis("load_duration"),
        finish_reason: object
            .get("done_reason")
            .and_then(JsonValue::as_str)
            .map(str::to_owned),
        ..Default::default()
    }
}

async fn stream_embed(
    mut response: reqwest::Response,
    tx: &mpsc::UnboundedSender<Result<CapabilityChunk, RuntimeError>>,
) {
    let status = response.status();
    let bytes = match capped_body(&mut response, MAX_EMBED_BYTES).await {
        Ok(bytes) => bytes,
        Err(err) => {
            let _ = tx.send(Err(err));
            return;
        }
    };
    if status != reqwest::StatusCode::OK {
        let _ = tx.send(Err(RuntimeError::Failed(http_error_message(
            &bytes,
            status.as_u16(),
        ))));
        return;
    }
    match parse_embed(&bytes) {
        Ok((vectors, prompt_tokens)) => {
            for vector in vectors {
                if tx.send(Ok(CapabilityChunk::Vector(vector))).is_err() {
                    return;
                }
            }
            let stats = prompt_tokens.map(|tokens| GenerationStats {
                prompt_tokens: Some(tokens),
                ..Default::default()
            });
            let _ = tx.send(Ok(CapabilityChunk::Done(stats)));
        }
        Err(err) => {
            let _ = tx.send(Err(err));
        }
    }
}

async fn post_json(
    client: &reqwest::Client,
    url: &str,
    body: Vec<u8>,
) -> Result<reqwest::Response, RuntimeError> {
    client
        .post(url)
        .header("content-type", "application/json")
        .body(body)
        .send()
        .await
        .map_err(|err| {
            if err.is_connect() {
                RuntimeError::Unavailable(NOT_RUNNING_HINT.to_owned())
            } else {
                RuntimeError::Failed(format!("ollama: {err}"))
            }
        })
}

/// Send the built body; if the runtime is unreachable, run `recover` (which
/// starts the daemon) and, if it reports success, send once more.
///
/// `build_body` is a closure rather than a prebuilt `Vec` so the common path —
/// the daemon already running — never copies the request body (an embedding
/// batch can be tens of megabytes). The body is only rebuilt on the cold-start
/// retry, at most once per call; concurrent requests against a down daemon each
/// try the start, and the losers exit on the port bind. `send` and `recover`
/// are injected so the retry logic is testable without a live server or a
/// spawned process.
async fn retry_after_recovery<T, B, S, SF, R, RF>(
    build_body: B,
    send: S,
    recover: R,
) -> Result<T, RuntimeError>
where
    B: Fn() -> Result<Vec<u8>, RuntimeError>,
    S: Fn(Vec<u8>) -> SF,
    SF: std::future::Future<Output = Result<T, RuntimeError>>,
    R: FnOnce() -> RF,
    RF: std::future::Future<Output = bool>,
{
    match send(build_body()?).await {
        Err(RuntimeError::Unavailable(hint)) => {
            if recover().await {
                send(build_body()?).await
            } else {
                Err(RuntimeError::Unavailable(hint))
            }
        }
        other => other,
    }
}

/// Post to the Ollama daemon, auto-starting it on a cold connection.
async fn send_with_autostart<B>(
    client: &reqwest::Client,
    base_url: &str,
    url: &str,
    build_body: B,
) -> Result<reqwest::Response, RuntimeError>
where
    B: Fn() -> Result<Vec<u8>, RuntimeError>,
{
    retry_after_recovery(
        build_body,
        |body| post_json(client, url, body),
        || ensure_daemon(client, base_url),
    )
    .await
}

/// Whether a local Ollama daemon is answering, starting it first if it isn't but
/// the binary is installed. Returns `false` when Ollama isn't installed or the
/// daemon didn't come up, so the caller keeps the "isn't running" error rather
/// than hanging.
async fn ensure_daemon(client: &reqwest::Client, base_url: &str) -> bool {
    if crate::install::ollama::daemon_reachable(client, base_url).await {
        return true;
    }
    // vars_os rather than vars: vars() panics on a non-Unicode variable (legal
    // on Unix), and this runs inside library code. A variable that isn't UTF-8
    // is dropped from the daemon's inherited environment instead.
    let environment: std::collections::HashMap<String, String> = std::env::vars_os()
        .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)))
        .collect();
    crate::install::ollama::start_daemon(client, base_url, &environment)
        .await
        .is_ok()
}

async fn capped_body(
    response: &mut reqwest::Response,
    cap: usize,
) -> Result<Vec<u8>, RuntimeError> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|err| RuntimeError::Failed(format!("ollama: {err}")))?
    {
        bytes.extend_from_slice(&chunk);
        if bytes.len() > cap {
            return Err(RuntimeError::Failed(format!(
                "ollama sent a response larger than {cap} bytes"
            )));
        }
    }
    Ok(bytes)
}

fn chat_body(model: &str, payload: &JsonValue) -> Result<Vec<u8>, RuntimeError> {
    let object = payload
        .as_object()
        .ok_or_else(|| RuntimeError::Failed("chat payload must be an object".to_owned()))?;

    let messages = if let Some(existing) = object.get("messages") {
        existing.clone()
    } else if let Some(prompt) = object.get("prompt").and_then(JsonValue::as_str) {
        JsonValue::Array(vec![super::object_of([
            ("role", string("user")),
            ("content", string(prompt)),
        ])])
    } else {
        return Err(RuntimeError::Failed(
            "chat payload must carry a messages array or a prompt".to_owned(),
        ));
    };

    let mut body = BTreeMap::new();
    body.insert("model".to_owned(), string(model));
    body.insert("messages".to_owned(), wire_messages(&messages));
    body.insert("stream".to_owned(), JsonValue::Bool(true));

    if let Some(JsonValue::Array(tools)) = object.get("tools")
        && !tools.is_empty()
    {
        let wrapped = tools
            .iter()
            .map(|tool| {
                super::object_of([("type", string("function")), ("function", tool.clone())])
            })
            .collect();
        body.insert("tools".to_owned(), JsonValue::Array(wrapped));
    }

    let mut options = BTreeMap::new();
    for (payload_key, option_key) in OPTION_KEYS {
        if let Some(value) = object.get(payload_key)
            && *value != JsonValue::Null
        {
            options.insert(option_key.to_owned(), value.clone());
        }
    }
    if !options.is_empty() {
        body.insert("options".to_owned(), JsonValue::Object(options));
    }

    if let Some(response_format) = object.get("response_format").and_then(JsonValue::as_object)
        && let Some(kind) = response_format.get("type").and_then(JsonValue::as_str)
    {
        if kind == "json_object" {
            body.insert("format".to_owned(), string("json"));
        } else if kind == "json_schema"
            && let Some(schema) = response_format
                .get("json_schema")
                .and_then(JsonValue::as_object)
                .and_then(|wrapper| wrapper.get("schema"))
        {
            body.insert("format".to_owned(), schema.clone());
        }
    }

    // A present-but-null `thinking` suppresses the tool-content default — so this
    // is deliberately nested, not a `&&` let-chain that would fall through.
    if let Some(thinking) = object.get("thinking") {
        if *thinking != JsonValue::Null {
            body.insert("think".to_owned(), thinking.clone());
        }
    } else if carries_tool_content(object) {
        body.insert("think".to_owned(), JsonValue::Bool(false));
    }

    serde_json::to_vec(&JsonValue::Object(body))
        .map_err(|err| RuntimeError::Failed(err.to_string()))
}

fn carries_tool_content(object: &BTreeMap<String, JsonValue>) -> bool {
    if let Some(JsonValue::Array(tools)) = object.get("tools")
        && !tools.is_empty()
    {
        return true;
    }
    let Some(JsonValue::Array(messages)) = object.get("messages") else {
        return false;
    };
    messages.iter().any(|message| {
        let Some(fields) = message.as_object() else {
            return false;
        };
        fields.contains_key("tool_calls")
            || fields.get("role").and_then(JsonValue::as_str) == Some("tool")
    })
}

fn wire_messages(messages: &JsonValue) -> JsonValue {
    let JsonValue::Array(entries) = messages else {
        return messages.clone();
    };
    JsonValue::Array(
        entries
            .iter()
            .map(|entry| {
                let Some(original) = entry.as_object() else {
                    return entry.clone();
                };
                let mut fields = original.clone();
                if let Some(JsonValue::Array(calls)) = fields.get("tool_calls") {
                    let reshaped = calls
                        .iter()
                        .map(|call| {
                            let Some(parts) = call.as_object() else {
                                return call.clone();
                            };
                            super::object_of([(
                                "function",
                                super::object_of([
                                    (
                                        "name",
                                        parts.get("name").cloned().unwrap_or_else(|| string("")),
                                    ),
                                    (
                                        "arguments",
                                        parts
                                            .get("arguments")
                                            .cloned()
                                            .unwrap_or_else(empty_object),
                                    ),
                                ]),
                            )])
                        })
                        .collect();
                    fields.insert("tool_calls".to_owned(), JsonValue::Array(reshaped));
                }
                fields.remove("tool_call_id");
                JsonValue::Object(fields)
            })
            .collect(),
    )
}

fn embed_body(model: &str, payload: &JsonValue) -> Result<Vec<u8>, RuntimeError> {
    let input = payload
        .as_object()
        .and_then(|object| object.get("input"))
        .filter(|input| **input != JsonValue::Null)
        .ok_or_else(|| RuntimeError::Failed("embed payload must carry an input".to_owned()))?;
    let body = super::object_of([("model", string(model)), ("input", input.clone())]);
    serde_json::to_vec(&body).map_err(|err| RuntimeError::Failed(err.to_string()))
}

fn parse_embed(bytes: &[u8]) -> Result<(Vec<Vec<f64>>, Option<i64>), RuntimeError> {
    let value: JsonValue = serde_json::from_slice(bytes)
        .map_err(|_| RuntimeError::Failed("ollama embed response was not understood".to_owned()))?;
    let object = value.as_object().ok_or_else(|| {
        RuntimeError::Failed("ollama embed response was not understood".to_owned())
    })?;
    if let Some(message) = object.get("error").and_then(JsonValue::as_str) {
        return Err(RuntimeError::Failed(format!("ollama: {message}")));
    }
    let understood = || RuntimeError::Failed("ollama embed response was not understood".to_owned());
    // Strict: a non-array row or a non-numeric element fails the whole response
    // rather than being dropped.
    let mut vectors: Vec<Vec<f64>> = Vec::new();
    if let Some(rows) = object.get("embeddings").and_then(JsonValue::as_array) {
        for row in rows {
            let values = row.as_array().ok_or_else(understood)?;
            let vector = values
                .iter()
                .map(|value| value.as_f64().ok_or_else(understood))
                .collect::<Result<Vec<f64>, _>>()?;
            vectors.push(vector);
        }
    }
    if vectors.is_empty() {
        return Err(understood());
    }
    let prompt_tokens = object.get("prompt_eval_count").and_then(JsonValue::as_i64);
    Ok((vectors, prompt_tokens))
}

fn http_error_message(body: &[u8], code: u16) -> String {
    // The body may be multi-line ndjson; take the first line carrying an error.
    for line in body.split(|&byte| byte == b'\n') {
        let message = serde_json::from_slice::<JsonValue>(line)
            .ok()
            .and_then(|value| {
                value
                    .as_object()
                    .and_then(|object| object.get("error"))
                    .and_then(JsonValue::as_str)
                    .filter(|message| !message.is_empty())
                    .map(str::to_owned)
            });
        if let Some(message) = message {
            return format!("ollama: {message}");
        }
    }
    format!("ollama returned HTTP {code}")
}

fn string(value: &str) -> JsonValue {
    JsonValue::String(value.to_owned())
}

fn empty_object() -> JsonValue {
    JsonValue::Object(BTreeMap::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn ollama_wires_tools() {
        // Resolution leans on this to fold the `tools` capability onto
        // Ollama-served models whose templates declare them.
        assert!(OllamaAdapter::new().wires_tools());
    }

    /// Count how many times the body is built and sent, so the retry can be
    /// distinguished from the first attempt.
    struct Calls {
        built: Cell<usize>,
        sent: Cell<usize>,
    }

    impl Calls {
        fn new() -> Self {
            Self {
                built: Cell::new(0),
                sent: Cell::new(0),
            }
        }
        fn build(&self) -> Result<Vec<u8>, RuntimeError> {
            self.built.set(self.built.get() + 1);
            Ok(vec![1, 2, 3])
        }
    }

    #[tokio::test]
    async fn a_successful_send_never_recovers_or_rebuilds() {
        let calls = Calls::new();
        let result: Result<&str, RuntimeError> = retry_after_recovery(
            || calls.build(),
            |_body| {
                calls.sent.set(calls.sent.get() + 1);
                async { Ok("ok") }
            },
            || async {
                panic!("recovery must not run when the first send succeeds");
            },
        )
        .await;
        assert_eq!(result.unwrap(), "ok");
        assert_eq!(calls.built.get(), 1);
        assert_eq!(calls.sent.get(), 1);
    }

    #[tokio::test]
    async fn an_unreachable_send_recovers_then_rebuilds_and_sends_again() {
        let calls = Calls::new();
        let result: Result<&str, RuntimeError> = retry_after_recovery(
            || calls.build(),
            |_body| {
                let attempt = calls.sent.get() + 1;
                calls.sent.set(attempt);
                async move {
                    if attempt == 1 {
                        Err(RuntimeError::Unavailable("down".to_owned()))
                    } else {
                        Ok("ok")
                    }
                }
            },
            || async { true },
        )
        .await;
        assert_eq!(result.unwrap(), "ok");
        // Built once per send: the retry re-serializes rather than cloning.
        assert_eq!(calls.built.get(), 2);
        assert_eq!(calls.sent.get(), 2);
    }

    #[tokio::test]
    async fn a_failed_recovery_keeps_the_unavailable_error_and_does_not_resend() {
        let calls = Calls::new();
        let result: Result<&str, RuntimeError> = retry_after_recovery(
            || calls.build(),
            |_body| {
                calls.sent.set(calls.sent.get() + 1);
                async { Err(RuntimeError::Unavailable("Ollama isn't running".to_owned())) }
            },
            || async { false },
        )
        .await;
        match result {
            Err(RuntimeError::Unavailable(hint)) => assert!(hint.contains("isn't running")),
            other => panic!("expected the unavailable error, got {other:?}"),
        }
        assert_eq!(calls.sent.get(), 1);
    }

    #[tokio::test]
    async fn a_non_connection_error_is_not_retried() {
        let calls = Calls::new();
        let result: Result<&str, RuntimeError> = retry_after_recovery(
            || calls.build(),
            |_body| {
                calls.sent.set(calls.sent.get() + 1);
                async { Err(RuntimeError::Failed("bad request".to_owned())) }
            },
            || async { panic!("recovery must not run for a non-connection failure") },
        )
        .await;
        assert!(matches!(result, Err(RuntimeError::Failed(_))));
        assert_eq!(calls.sent.get(), 1);
    }
}
