//! The OpenAI-compatible endpoint adapter: serves chat/completion by streaming
//! from a remote server speaking the OpenAI `/v1/chat/completions` API (OpenAI
//! itself, or any compatible server — vLLM, LM Studio's server, llama-server, …).
//! Server-sent-event `data:` lines are parsed into `CapabilityChunk`s.
//!
//! The API key comes from an injected [`SecretStore`] (default: an environment
//! variable); a macOS Keychain-backed store is deferred, as is the registry
//! reachability marking.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Instant;

use kernel::capabilities::{CapabilityChunk, GenerationStats, ToolCall};
use kernel::records::{BidPreference, Capability, JsonValue, ModelRecord, RunTier, RuntimeId};
use kernel::resolution::{IdentifiedModel, ModelFormat, RuntimeBid};
use tokio::sync::mpsc;

use super::{ChunkStream, RuntimeAdapter, RuntimeError};

const DEFAULT_IN_FLIGHT_LIMIT: usize = 4;

/// The request params an OpenAI endpoint honors (forwarded verbatim). Shared with
/// the `llama-server` adapter, which proxies through the same request body.
pub(crate) const OPTION_KEYS: [&str; 8] = [
    "temperature",
    "top_p",
    "max_tokens",
    "stop",
    "seed",
    "frequency_penalty",
    "presence_penalty",
    "response_format",
];

/// A source of per-endpoint API keys, keyed by the normalized base URL (the
/// "account"). Returns `None` when no key is configured (an unauthenticated
/// server).
pub trait SecretStore: Send + Sync {
    /// The API key for `account`, if one is stored.
    fn get(&self, account: &str) -> Option<String>;
}

/// The default secret store: a single API key from `HEDOS_OPENAI_API_KEY`,
/// applied to every endpoint. (Per-endpoint secure storage is deferred with the
/// Keychain port.)
pub struct EnvSecretStore;

impl SecretStore for EnvSecretStore {
    fn get(&self, _account: &str) -> Option<String> {
        std::env::var("HEDOS_OPENAI_API_KEY")
            .ok()
            .filter(|key| !key.is_empty())
    }
}

/// A per-base in-flight request limiter, so a burst of requests can't overwhelm a
/// single remote server. `acquire` fails once `limit` requests to that base are
/// already running.
pub struct EndpointConcurrencyGate {
    limit: usize,
    counts: Mutex<HashMap<String, usize>>,
}

impl EndpointConcurrencyGate {
    /// A gate allowing `limit` concurrent requests per base URL.
    pub fn new(limit: usize) -> Self {
        Self {
            limit,
            counts: Mutex::new(HashMap::new()),
        }
    }

    /// The process-shared gate — every adapter built with the default constructor
    /// shares it, so one server's in-flight budget is enforced across all of them
    /// (a process-wide singleton).
    pub fn shared() -> Arc<EndpointConcurrencyGate> {
        static SHARED: LazyLock<Arc<EndpointConcurrencyGate>> =
            LazyLock::new(|| Arc::new(EndpointConcurrencyGate::default()));
        Arc::clone(&SHARED)
    }

    /// Reserve a slot for `base`, or `None` if the per-base limit is already
    /// reached. The returned guard releases the slot when dropped.
    pub fn acquire(self: &Arc<Self>, base: &str) -> Option<GateGuard> {
        let mut counts = self.counts.lock().ok()?;
        let count = counts.entry(base.to_owned()).or_insert(0);
        if *count >= self.limit {
            return None;
        }
        *count += 1;
        Some(GateGuard {
            gate: Arc::clone(self),
            base: base.to_owned(),
        })
    }

    fn release(&self, base: &str) {
        if let Ok(mut counts) = self.counts.lock()
            && let Some(count) = counts.get_mut(base)
        {
            if *count <= 1 {
                counts.remove(base);
            } else {
                *count -= 1;
            }
        }
    }
}

impl Default for EndpointConcurrencyGate {
    fn default() -> Self {
        Self::new(DEFAULT_IN_FLIGHT_LIMIT)
    }
}

/// Releases its slot on the gate when dropped (on completion, cancel, or error).
pub struct GateGuard {
    gate: Arc<EndpointConcurrencyGate>,
    base: String,
}

impl Drop for GateGuard {
    fn drop(&mut self) {
        self.gate.release(&self.base);
    }
}

/// Serves models through a remote OpenAI-compatible endpoint.
pub struct OpenAiEndpointAdapter {
    id: RuntimeId,
    secrets: Arc<dyn SecretStore>,
    gate: Arc<EndpointConcurrencyGate>,
    client: reqwest::Client,
}

impl OpenAiEndpointAdapter {
    /// An adapter reading keys from the environment with the default in-flight
    /// limit.
    pub fn new() -> Self {
        Self::with_secrets(Arc::new(EnvSecretStore))
    }

    /// An adapter with an explicit secret store, over the process-shared gate.
    pub fn with_secrets(secrets: Arc<dyn SecretStore>) -> Self {
        Self::with(secrets, EndpointConcurrencyGate::shared())
    }

    /// An adapter with an explicit secret store and in-flight gate.
    pub fn with(secrets: Arc<dyn SecretStore>, gate: Arc<EndpointConcurrencyGate>) -> Self {
        Self {
            id: RuntimeId::openai_endpoint(),
            secrets,
            gate,
            client: reqwest::Client::new(),
        }
    }
}

impl Default for OpenAiEndpointAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl RuntimeAdapter for OpenAiEndpointAdapter {
    fn id(&self) -> &RuntimeId {
        &self.id
    }

    fn can_serve(&self, record: &ModelRecord, capability: &Capability) -> bool {
        let served = *capability == Capability::chat() || *capability == Capability::complete();
        served && record.runtime.id.as_ref() == Some(&self.id)
    }

    fn bid(&self, _record: &ModelRecord, identified: &IdentifiedModel) -> Option<RuntimeBid> {
        if identified.format != ModelFormat::Endpoint {
            return None;
        }
        Some(RuntimeBid::new(RunTier::Remote, BidPreference::ENDPOINT))
    }

    fn invoke(
        &self,
        record: &ModelRecord,
        _capability: Capability,
        payload: JsonValue,
    ) -> ChunkStream {
        let (tx, stream) = ChunkStream::channel();
        let base = normalized_base(&record.source.path);
        // The wire model name is the endpoint's own id (the repo), not the display
        // name; fall back to the name when no repo is set.
        let model = record
            .source
            .repo
            .clone()
            .unwrap_or_else(|| record.name.clone());
        let key = self.secrets.get(&base);
        let client = self.client.clone();
        let gate = Arc::clone(&self.gate);

        tokio::spawn(async move {
            let Some(_guard) = gate.acquire(&base) else {
                let _ = tx.send(Err(RuntimeError::Unavailable(
                    "too many requests are already in flight to this server; retry shortly"
                        .to_owned(),
                )));
                return;
            };
            let body = match request_body(&model, &payload) {
                Ok(body) => body,
                Err(err) => {
                    let _ = tx.send(Err(err));
                    return;
                }
            };
            stream_completions(&client, &base, key.as_deref(), body, &tx).await;
        });
        stream
    }

    fn supports_tools(&self, _record: &ModelRecord) -> bool {
        true
    }

    fn honored_param_keys(
        &self,
        _record: &ModelRecord,
        capability: &Capability,
    ) -> HashSet<String> {
        if *capability != Capability::chat() && *capability != Capability::complete() {
            return HashSet::new();
        }
        OPTION_KEYS.iter().map(|key| (*key).to_owned()).collect()
    }
}

/// Stream an OpenAI-compatible `/v1/chat/completions` response, parsing SSE lines
/// into capability chunks. Shared with the local `llama-server` adapter, which
/// speaks the same wire protocol.
pub(crate) async fn stream_completions(
    client: &reqwest::Client,
    base: &str,
    key: Option<&str>,
    body: Vec<u8>,
    tx: &mpsc::UnboundedSender<Result<CapabilityChunk, RuntimeError>>,
) {
    let url = format!("{base}/v1/chat/completions");
    let mut builder = client
        .post(&url)
        .header("content-type", "application/json")
        .body(body);
    if let Some(key) = key.filter(|key| !key.is_empty()) {
        builder = builder.header("authorization", format!("Bearer {key}"));
    }
    let response = match builder.send().await {
        Ok(response) => response,
        Err(err) => {
            let error = if err.is_connect() || err.is_timeout() {
                RuntimeError::Unavailable("the configured server isn't reachable".to_owned())
            } else {
                RuntimeError::Failed(format!("endpoint: {err}"))
            };
            let _ = tx.send(Err(error));
            return;
        }
    };

    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        let _ = tx.send(Err(RuntimeError::Unavailable(
            "the server refused the API key".to_owned(),
        )));
        return;
    }
    if status != reqwest::StatusCode::OK {
        let _ = tx.send(Err(RuntimeError::Unavailable(format!(
            "the server answered with HTTP {}",
            status.as_u16()
        ))));
        return;
    }

    let mut parser = OpenAiStreamParser::new();
    super::line_stream::read_lines(response, tx, "endpoint", |line| {
        forward_line(&mut parser, tx, line)
    })
    .await;
}

/// Parse one SSE line and forward its chunks. Returns `false` to stop (the stream
/// signalled `[DONE]`, or the consumer went away).
fn forward_line(
    parser: &mut OpenAiStreamParser,
    tx: &mpsc::UnboundedSender<Result<CapabilityChunk, RuntimeError>>,
    line: &[u8],
) -> bool {
    let Ok(text) = std::str::from_utf8(line) else {
        return true;
    };
    for chunk in parser.parse(text) {
        let done = matches!(chunk, CapabilityChunk::Done(_));
        if tx.send(Ok(chunk)).is_err() {
            return false;
        }
        if done {
            return false;
        }
    }
    true
}

/// Normalize a server base URL: add a scheme if missing, strip trailing slashes,
/// and drop a trailing `/v1` (it is re-added per request path).
fn normalized_base(raw: &str) -> String {
    let mut base = raw.trim().to_owned();
    if !base.contains("://") {
        base = format!("http://{base}");
    }
    while base.ends_with('/') {
        base.pop();
    }
    if base.to_lowercase().ends_with("/v1") {
        base.truncate(base.len() - 3);
        while base.ends_with('/') {
            base.pop();
        }
    }
    base
}

/// Build the `/v1/chat/completions` request body: a streaming request carrying the
/// model, the wire-shaped messages (or a prompt), any tools, and the honored
/// sampling options.
pub(crate) fn request_body(model: &str, payload: &JsonValue) -> Result<Vec<u8>, RuntimeError> {
    let JsonValue::Object(object) = payload else {
        return Err(RuntimeError::Failed(
            "chat payload must be an object".to_owned(),
        ));
    };
    let mut body: BTreeMap<String, JsonValue> = BTreeMap::new();
    body.insert("model".to_owned(), JsonValue::String(model.to_owned()));
    body.insert("stream".to_owned(), JsonValue::Bool(true));
    body.insert(
        "stream_options".to_owned(),
        object_of([("include_usage", JsonValue::Bool(true))]),
    );

    if let Some(messages) = object.get("messages") {
        body.insert("messages".to_owned(), wire_messages(messages));
    } else if let Some(JsonValue::String(prompt)) = object.get("prompt") {
        body.insert(
            "messages".to_owned(),
            JsonValue::Array(vec![object_of([
                ("role", JsonValue::String("user".to_owned())),
                ("content", JsonValue::String(prompt.clone())),
            ])]),
        );
    } else {
        return Err(RuntimeError::Failed(
            "chat payload must carry a messages array".to_owned(),
        ));
    }

    if let Some(JsonValue::Array(tools)) = object.get("tools")
        && !tools.is_empty()
    {
        let wrapped = tools
            .iter()
            .map(|tool| {
                object_of([
                    ("type", JsonValue::String("function".to_owned())),
                    ("function", tool.clone()),
                ])
            })
            .collect();
        body.insert("tools".to_owned(), JsonValue::Array(wrapped));
    }
    if let Some(tool_choice) = object.get("tool_choice")
        && *tool_choice != JsonValue::Null
    {
        body.insert("tool_choice".to_owned(), tool_choice.clone());
    }
    for key in OPTION_KEYS {
        if let Some(value) = object.get(key)
            && *value != JsonValue::Null
        {
            body.insert(key.to_owned(), value.clone());
        }
    }
    serde_json::to_vec(&JsonValue::Object(body))
        .map_err(|err| RuntimeError::Failed(format!("encoding request: {err}")))
}

/// Reshape internal messages into OpenAI wire form: tool calls become
/// `{type:function, function:{name, arguments:<json string>}}`, an empty assistant
/// `content` alongside tool calls becomes `null`, and `tool_name` becomes `name`.
fn wire_messages(messages: &JsonValue) -> JsonValue {
    let JsonValue::Array(entries) = messages else {
        return messages.clone();
    };
    let mapped = entries
        .iter()
        .map(|entry| {
            let JsonValue::Object(fields) = entry else {
                return entry.clone();
            };
            let mut fields = fields.clone();
            if let Some(JsonValue::Array(calls)) = fields.get("tool_calls") {
                let wired = calls
                    .iter()
                    .map(|call| {
                        let JsonValue::Object(parts) = call else {
                            return call.clone();
                        };
                        // A missing arguments object becomes `{}`, not `null` —
                        // some servers reject a `"null"` arguments string.
                        let arguments = parts
                            .get("arguments")
                            .cloned()
                            .unwrap_or_else(|| JsonValue::Object(BTreeMap::new()));
                        let arguments = serde_json::to_string(&arguments).unwrap_or_default();
                        object_of([
                            (
                                "id",
                                parts
                                    .get("id")
                                    .cloned()
                                    .unwrap_or(JsonValue::String(String::new())),
                            ),
                            ("type", JsonValue::String("function".to_owned())),
                            (
                                "function",
                                object_of([
                                    (
                                        "name",
                                        parts
                                            .get("name")
                                            .cloned()
                                            .unwrap_or(JsonValue::String(String::new())),
                                    ),
                                    ("arguments", JsonValue::String(arguments)),
                                ]),
                            ),
                        ])
                    })
                    .collect();
                fields.insert("tool_calls".to_owned(), JsonValue::Array(wired));
                if fields.get("content") == Some(&JsonValue::String(String::new())) {
                    fields.insert("content".to_owned(), JsonValue::Null);
                }
            }
            if let Some(tool_name) = fields.remove("tool_name") {
                fields.insert("name".to_owned(), tool_name);
            }
            JsonValue::Object(fields)
        })
        .collect();
    JsonValue::Array(mapped)
}

fn object_of<const N: usize>(pairs: [(&str, JsonValue); N]) -> JsonValue {
    JsonValue::Object(
        pairs
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect(),
    )
}

/// An accumulating parser for the OpenAI SSE stream. Tool-call fragments arrive
/// split across `delta.tool_calls` events keyed by index and are reassembled on
/// the finish/`[DONE]` boundary.
struct OpenAiStreamParser {
    started: Instant,
    prompt_tokens: Option<i64>,
    completion_tokens: Option<i64>,
    finish_reason: Option<String>,
    tool_fragments: BTreeMap<i64, ToolFragment>,
}

#[derive(Default)]
struct ToolFragment {
    id: Option<String>,
    name: String,
    arguments: String,
}

impl OpenAiStreamParser {
    fn new() -> Self {
        Self {
            started: Instant::now(),
            prompt_tokens: None,
            completion_tokens: None,
            finish_reason: None,
            tool_fragments: BTreeMap::new(),
        }
    }

    fn parse(&mut self, line: &str) -> Vec<CapabilityChunk> {
        let trimmed = line.trim();
        let Some(payload) = trimmed.strip_prefix("data:") else {
            return Vec::new();
        };
        let payload = payload.trim();
        if payload == "[DONE]" {
            let duration_ms = self.started.elapsed().as_millis() as i64;
            let mut chunks = self.flush_tool_calls();
            chunks.push(CapabilityChunk::Done(Some(GenerationStats {
                prompt_tokens: self.prompt_tokens,
                completion_tokens: self.completion_tokens,
                duration_ms: Some(duration_ms),
                finish_reason: self.finish_reason.clone(),
                ..GenerationStats::default()
            })));
            return chunks;
        }
        let Ok(JsonValue::Object(object)) = serde_json::from_str::<JsonValue>(payload) else {
            return Vec::new();
        };

        if let Some(JsonValue::Object(usage)) = object.get("usage") {
            if let Some(tokens) = usage.get("prompt_tokens").and_then(JsonValue::as_i64) {
                self.prompt_tokens = Some(tokens);
            }
            if let Some(tokens) = usage.get("completion_tokens").and_then(JsonValue::as_i64) {
                self.completion_tokens = Some(tokens);
            }
        }

        let mut chunks = Vec::new();
        let Some(JsonValue::Array(choices)) = object.get("choices") else {
            return chunks;
        };
        let Some(JsonValue::Object(choice)) = choices.first() else {
            return chunks;
        };
        if let Some(JsonValue::Object(delta)) = choice.get("delta") {
            if let Some(reasoning) = delta.get("reasoning_content").and_then(JsonValue::as_str)
                && !reasoning.is_empty()
            {
                chunks.push(CapabilityChunk::Thinking(reasoning.to_owned()));
            }
            if let Some(content) = delta.get("content").and_then(JsonValue::as_str)
                && !content.is_empty()
            {
                chunks.push(CapabilityChunk::Text(content.to_owned()));
            }
            if let Some(JsonValue::Array(calls)) = delta.get("tool_calls") {
                for entry in calls {
                    self.accumulate(entry);
                }
            }
        }
        if let Some(reason) = choice.get("finish_reason").and_then(JsonValue::as_str)
            && !reason.is_empty()
        {
            self.finish_reason = Some(reason.to_owned());
            chunks.extend(self.flush_tool_calls());
        }
        chunks
    }

    fn accumulate(&mut self, entry: &JsonValue) {
        let JsonValue::Object(entry) = entry else {
            return;
        };
        let index = entry.get("index").and_then(JsonValue::as_i64).unwrap_or(0);
        let fragment = self.tool_fragments.entry(index).or_default();
        if let Some(id) = entry.get("id").and_then(JsonValue::as_str)
            && !id.is_empty()
        {
            fragment.id = Some(id.to_owned());
        }
        if let Some(JsonValue::Object(function)) = entry.get("function") {
            if let Some(name) = function.get("name").and_then(JsonValue::as_str)
                && !name.is_empty()
            {
                fragment.name.push_str(name);
            }
            if let Some(arguments) = function.get("arguments").and_then(JsonValue::as_str) {
                fragment.arguments.push_str(arguments);
            }
        }
    }

    fn flush_tool_calls(&mut self) -> Vec<CapabilityChunk> {
        let fragments = std::mem::take(&mut self.tool_fragments);
        fragments
            .into_values()
            .filter(|fragment| !fragment.name.is_empty())
            .map(|fragment| {
                let arguments = parse_tool_arguments(&fragment.arguments);
                let call = match fragment.id {
                    Some(id) => ToolCall::with_id(id, fragment.name, arguments),
                    None => ToolCall::new(fragment.name, arguments),
                };
                CapabilityChunk::ToolCall(call)
            })
            .collect()
    }
}

/// Parse accumulated tool-call arguments: a JSON object if it parses as one, an
/// empty object when blank, else the raw string wrapped under `_raw`.
fn parse_tool_arguments(raw: &str) -> JsonValue {
    if let Ok(JsonValue::Object(map)) = serde_json::from_str::<JsonValue>(raw) {
        return JsonValue::Object(map);
    }
    if raw.trim().is_empty() {
        return JsonValue::Object(BTreeMap::new());
    }
    object_of([("_raw", JsonValue::String(raw.to_owned()))])
}
