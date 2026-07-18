//! Tests for the OpenAI endpoint adapter, driven by a mock HTTP server (a `tokio`
//! TCP listener) streaming canned server-sent events.

use std::sync::{Arc, Mutex};

use kernel::capabilities::CapabilityChunk;
use kernel::records::{
    Capability, ExecutionMode, JsonValue, Modality, ModelRecord, ModelSource, RuntimeId, SourceKind,
};
use kernel::resolution::{IdentifiedModel, ModelFormat};
use runtime::adapters::{
    ChunkStream, EndpointConcurrencyGate, OpenAiEndpointAdapter, RuntimeAdapter, RuntimeError,
    SecretStore,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::task::AbortHandle;

struct MockServer {
    base_url: String,
    last_request: Arc<Mutex<Option<Vec<u8>>>>,
    accept: AbortHandle,
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.accept.abort();
    }
}

async fn mock(
    handler: impl Fn(&str, &[u8]) -> (u16, String) + Send + Sync + 'static,
) -> MockServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let last_request = Arc::new(Mutex::new(None));
    let recorded = Arc::clone(&last_request);
    let handler = Arc::new(handler);
    let accept = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let handler = Arc::clone(&handler);
            let recorded = Arc::clone(&recorded);
            tokio::spawn(async move {
                if let Some((path, body)) = read_request(&mut stream).await {
                    *recorded.lock().expect("lock") = Some(body.clone());
                    let (status, response) = handler(&path, &body);
                    let head = format!(
                        "HTTP/1.1 {status} OK\r\nContent-Length: {}\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
                        response.len()
                    );
                    let _ = stream.write_all(head.as_bytes()).await;
                    let _ = stream.write_all(response.as_bytes()).await;
                    let _ = stream.flush().await;
                }
            });
        }
    })
    .abort_handle();
    MockServer {
        base_url: format!("http://{addr}"),
        last_request,
        accept,
    }
}

async fn read_request(stream: &mut tokio::net::TcpStream) -> Option<(String, Vec<u8>)> {
    let mut buffer = Vec::new();
    let mut tmp = [0u8; 4096];
    let headers_end = loop {
        let n = stream.read(&mut tmp).await.ok()?;
        if n == 0 {
            return None;
        }
        buffer.extend_from_slice(&tmp[..n]);
        if let Some(pos) = find(&buffer, b"\r\n\r\n") {
            break pos;
        }
    };
    let headers = String::from_utf8_lossy(&buffer[..headers_end]);
    let path = headers
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/")
        .to_owned();
    let content_length = headers
        .lines()
        .find_map(|line| {
            let line = line.to_ascii_lowercase();
            line.strip_prefix("content-length:")
                .and_then(|value| value.trim().parse::<usize>().ok())
        })
        .unwrap_or(0);
    let mut body = buffer[headers_end + 4..].to_vec();
    while body.len() < content_length {
        let n = stream.read(&mut tmp).await.ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&tmp[..n]);
    }
    Some((path, body))
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn record(base: &str) -> ModelRecord {
    let mut rec = ModelRecord::new(
        "My Server Model",
        Modality::text(),
        vec![Capability::chat()],
        ModelSource::new(SourceKind::endpoint(), base),
    );
    rec.source.repo = Some("gpt-x".to_owned());
    rec.runtime.id = Some(RuntimeId::openai_endpoint());
    rec
}

fn chat_payload() -> JsonValue {
    object([(
        "messages",
        JsonValue::Array(vec![object([
            ("role", JsonValue::String("user".to_owned())),
            ("content", JsonValue::String("hi".to_owned())),
        ])]),
    )])
}

fn object<const N: usize>(pairs: [(&str, JsonValue); N]) -> JsonValue {
    JsonValue::Object(pairs.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
}

async fn collect(mut stream: ChunkStream) -> (Vec<CapabilityChunk>, Option<RuntimeError>) {
    let mut chunks = Vec::new();
    let mut error = None;
    while let Some(item) = stream.recv().await {
        match item {
            Ok(chunk) => chunks.push(chunk),
            Err(err) => error = Some(err),
        }
    }
    (chunks, error)
}

struct StaticSecret(&'static str);
impl SecretStore for StaticSecret {
    fn get(&self, _account: &str) -> Option<String> {
        Some(self.0.to_owned())
    }
}

#[tokio::test]
async fn streams_text_and_final_usage() {
    let sse = "data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\n\
               data: {\"choices\":[{\"delta\":{\"content\":\" world\"}}]}\n\n\
               data: {\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":2}}\n\n\
               data: [DONE]\n\n";
    let server = mock(move |_path, _body| (200, sse.to_owned())).await;
    let adapter = OpenAiEndpointAdapter::new();

    let stream = adapter.invoke(
        &record(&server.base_url),
        Capability::chat(),
        chat_payload(),
    );
    let (chunks, error) = collect(stream).await;
    assert!(error.is_none());
    assert_eq!(chunks[0], CapabilityChunk::Text("Hello".to_owned()));
    assert_eq!(chunks[1], CapabilityChunk::Text(" world".to_owned()));
    match chunks.last() {
        Some(CapabilityChunk::Done(Some(stats))) => {
            assert_eq!(stats.prompt_tokens, Some(5));
            assert_eq!(stats.completion_tokens, Some(2));
        }
        other => panic!("expected Done with stats, got {other:?}"),
    }

    // The request carried the repo as the wire model name and a streaming flag.
    let body = server.last_request.lock().unwrap().clone().unwrap();
    let sent: JsonValue = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        sent.as_object().unwrap().get("model"),
        Some(&JsonValue::String("gpt-x".to_owned()))
    );
    assert_eq!(
        sent.as_object().unwrap().get("stream"),
        Some(&JsonValue::Bool(true))
    );
}

#[tokio::test]
async fn accumulates_tool_call_fragments() {
    // The tool call arrives split across two deltas, then a finish_reason flush.
    let sse = "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"get_\"}}]}}]}\n\n\
               data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"name\":\"weather\",\"arguments\":\"{\\\"city\\\":\\\"NYC\\\"}\"}}]}}]}\n\n\
               data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n\
               data: [DONE]\n\n";
    let server = mock(move |_path, _body| (200, sse.to_owned())).await;
    let adapter = OpenAiEndpointAdapter::new();

    let (chunks, error) = collect(adapter.invoke(
        &record(&server.base_url),
        Capability::chat(),
        chat_payload(),
    ))
    .await;
    assert!(error.is_none());
    let call = chunks.iter().find_map(|chunk| match chunk {
        CapabilityChunk::ToolCall(call) => Some(call),
        _ => None,
    });
    let call = call.expect("a tool call");
    assert_eq!(call.id, "call_1");
    assert_eq!(call.name, "get_weather");
    assert_eq!(
        call.arguments.as_object().unwrap().get("city"),
        Some(&JsonValue::String("NYC".to_owned()))
    );
}

#[tokio::test]
async fn surfaces_reasoning_as_thinking() {
    let sse = "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"let me think\"}}]}\n\n\
               data: {\"choices\":[{\"delta\":{\"content\":\"answer\"}}]}\n\n\
               data: [DONE]\n\n";
    let server = mock(move |_path, _body| (200, sse.to_owned())).await;
    let adapter = OpenAiEndpointAdapter::new();
    let (chunks, _) = collect(adapter.invoke(
        &record(&server.base_url),
        Capability::chat(),
        chat_payload(),
    ))
    .await;
    assert_eq!(
        chunks[0],
        CapabilityChunk::Thinking("let me think".to_owned())
    );
    assert_eq!(chunks[1], CapabilityChunk::Text("answer".to_owned()));
}

#[tokio::test]
async fn a_refused_key_is_unavailable() {
    let server = mock(|_path, _body| (401, "{\"error\":\"bad key\"}".to_owned())).await;
    let adapter = OpenAiEndpointAdapter::new();
    let (_chunks, error) = collect(adapter.invoke(
        &record(&server.base_url),
        Capability::chat(),
        chat_payload(),
    ))
    .await;
    assert!(matches!(error, Some(RuntimeError::Unavailable(_))));
}

#[tokio::test]
async fn a_server_error_is_unavailable() {
    let server = mock(|_path, _body| (500, "boom".to_owned())).await;
    let adapter = OpenAiEndpointAdapter::new();
    let (_chunks, error) = collect(adapter.invoke(
        &record(&server.base_url),
        Capability::chat(),
        chat_payload(),
    ))
    .await;
    assert!(matches!(error, Some(RuntimeError::Unavailable(_))));
}

#[tokio::test]
async fn an_injected_key_still_completes_the_request() {
    // The mock can't observe headers, but an injected secret must not break the
    // request path (the Bearer header is added when a key is present).
    let server = mock(|_path, _body| (200, "data: [DONE]\n\n".to_owned())).await;
    let adapter = OpenAiEndpointAdapter::with_secrets(Arc::new(StaticSecret("sk-test")));
    let (chunks, error) = collect(adapter.invoke(
        &record(&server.base_url),
        Capability::chat(),
        chat_payload(),
    ))
    .await;
    assert!(error.is_none());
    assert!(matches!(chunks.last(), Some(CapabilityChunk::Done(_))));
}

#[test]
fn bid_is_only_for_the_endpoint_format() {
    let adapter = OpenAiEndpointAdapter::new();
    let endpoint = IdentifiedModel::new(
        ModelFormat::Endpoint,
        Some(Modality::text()),
        vec![Capability::chat()],
        ExecutionMode::Stream,
    );
    let rec = ModelRecord::new(
        "m",
        Modality::text(),
        Vec::new(),
        ModelSource::new(SourceKind::endpoint(), "http://x"),
    );
    assert!(adapter.bid(&rec, &endpoint).is_some());

    let gguf = IdentifiedModel::new(ModelFormat::Gguf, None, Vec::new(), ExecutionMode::Stream);
    assert!(adapter.bid(&rec, &gguf).is_none());
}

#[test]
fn can_serve_requires_the_endpoint_runtime_and_a_chat_capability() {
    let adapter = OpenAiEndpointAdapter::new();
    let mut rec = ModelRecord::new(
        "m",
        Modality::text(),
        vec![Capability::chat()],
        ModelSource::new(SourceKind::endpoint(), "http://x"),
    );
    // No runtime pinned → not served.
    assert!(!adapter.can_serve(&rec, &Capability::chat()));
    rec.runtime.id = Some(RuntimeId::openai_endpoint());
    assert!(adapter.can_serve(&rec, &Capability::chat()));
    // Embeddings aren't served by this adapter.
    assert!(!adapter.can_serve(&rec, &Capability::embed()));
}

#[test]
fn honored_params_cover_the_sampling_options() {
    let adapter = OpenAiEndpointAdapter::new();
    let rec = ModelRecord::new(
        "m",
        Modality::text(),
        Vec::new(),
        ModelSource::new(SourceKind::endpoint(), "http://x"),
    );
    let keys = adapter.honored_param_keys(&rec, &Capability::chat());
    assert!(keys.contains("temperature"));
    assert!(keys.contains("response_format"));
    assert!(
        adapter
            .honored_param_keys(&rec, &Capability::embed())
            .is_empty()
    );
}

/// A mock server that writes the response body in `pieces`, flushing between each
/// with a brief pause so the adapter observes them as separate `chunk()` reads.
async fn mock_streaming(pieces: Vec<&'static str>) -> MockServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let last_request = Arc::new(Mutex::new(None));
    let recorded = Arc::clone(&last_request);
    let total: usize = pieces.iter().map(|p| p.len()).sum();
    let accept = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let recorded = Arc::clone(&recorded);
            let pieces = pieces.clone();
            tokio::spawn(async move {
                if let Some((_, body)) = read_request(&mut stream).await {
                    *recorded.lock().expect("lock") = Some(body);
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {total}\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n"
                    );
                    let _ = stream.write_all(head.as_bytes()).await;
                    let _ = stream.flush().await;
                    for piece in pieces {
                        let _ = stream.write_all(piece.as_bytes()).await;
                        let _ = stream.flush().await;
                        tokio::time::sleep(std::time::Duration::from_millis(15)).await;
                    }
                }
            });
        }
    })
    .abort_handle();
    MockServer {
        base_url: format!("http://{addr}"),
        last_request,
        accept,
    }
}

#[tokio::test]
async fn reassembles_a_data_line_split_across_reads() {
    // The `data:` line is cut in the middle of the JSON, across two TCP writes.
    let server = mock_streaming(vec![
        "data: {\"choices\":[{\"delta\":{\"content\":\"Hel",
        "lo\"}}]}\n\ndata: [DONE]\n\n",
    ])
    .await;
    let adapter = OpenAiEndpointAdapter::new();
    let (chunks, error) = collect(adapter.invoke(
        &record(&server.base_url),
        Capability::chat(),
        chat_payload(),
    ))
    .await;
    assert!(error.is_none());
    assert_eq!(chunks[0], CapabilityChunk::Text("Hello".to_owned()));
}

#[tokio::test]
async fn tool_calls_flush_on_done_without_a_finish_reason() {
    let sse = "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"c1\",\"function\":{\"name\":\"f\",\"arguments\":\"{}\"}}]}}]}\n\n\
               data: [DONE]\n\n";
    let server = mock(move |_p, _b| (200, sse.to_owned())).await;
    let adapter = OpenAiEndpointAdapter::new();
    let (chunks, _) = collect(adapter.invoke(
        &record(&server.base_url),
        Capability::chat(),
        chat_payload(),
    ))
    .await;
    let calls = chunks
        .iter()
        .filter(|c| matches!(c, CapabilityChunk::ToolCall(_)))
        .count();
    assert_eq!(calls, 1);
}

#[tokio::test]
async fn a_finish_reason_then_done_does_not_double_emit_tool_calls() {
    let sse = "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"c1\",\"function\":{\"name\":\"f\",\"arguments\":\"{}\"}}]}}]}\n\n\
               data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n\
               data: [DONE]\n\n";
    let server = mock(move |_p, _b| (200, sse.to_owned())).await;
    let adapter = OpenAiEndpointAdapter::new();
    let (chunks, _) = collect(adapter.invoke(
        &record(&server.base_url),
        Capability::chat(),
        chat_payload(),
    ))
    .await;
    let calls = chunks
        .iter()
        .filter(|c| matches!(c, CapabilityChunk::ToolCall(_)))
        .count();
    assert_eq!(calls, 1, "the flushed fragments must not re-emit on [DONE]");
}

#[tokio::test]
async fn reassembles_multiple_concurrent_tool_call_indices() {
    let sse = "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"a\",\"function\":{\"name\":\"one\",\"arguments\":\"{}\"}},{\"index\":1,\"id\":\"b\",\"function\":{\"name\":\"two\",\"arguments\":\"{}\"}}]}}]}\n\n\
               data: [DONE]\n\n";
    let server = mock(move |_p, _b| (200, sse.to_owned())).await;
    let adapter = OpenAiEndpointAdapter::new();
    let (chunks, _) = collect(adapter.invoke(
        &record(&server.base_url),
        Capability::chat(),
        chat_payload(),
    ))
    .await;
    let names: Vec<&str> = chunks
        .iter()
        .filter_map(|c| match c {
            CapabilityChunk::ToolCall(call) => Some(call.name.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(names, vec!["one", "two"]);
}

#[tokio::test]
async fn a_prompt_payload_becomes_a_user_message() {
    let server = mock(|_p, _b| (200, "data: [DONE]\n\n".to_owned())).await;
    let adapter = OpenAiEndpointAdapter::new();
    let payload = object([("prompt", JsonValue::String("hello there".to_owned()))]);
    let (_c, error) =
        collect(adapter.invoke(&record(&server.base_url), Capability::chat(), payload)).await;
    assert!(error.is_none());
    let body = server.last_request.lock().unwrap().clone().unwrap();
    let sent: JsonValue = serde_json::from_slice(&body).unwrap();
    let messages = sent.as_object().unwrap().get("messages").unwrap();
    let JsonValue::Array(entries) = messages else {
        panic!("messages should be an array");
    };
    let first = entries[0].as_object().unwrap();
    assert_eq!(
        first.get("role"),
        Some(&JsonValue::String("user".to_owned()))
    );
    assert_eq!(
        first.get("content"),
        Some(&JsonValue::String("hello there".to_owned()))
    );
}

#[tokio::test]
async fn a_missing_tool_arguments_object_wires_as_an_empty_object() {
    let server = mock(|_p, _b| (200, "data: [DONE]\n\n".to_owned())).await;
    let adapter = OpenAiEndpointAdapter::new();
    // An assistant tool-call message from history with NO arguments key.
    let payload = object([(
        "messages",
        JsonValue::Array(vec![object([
            ("role", JsonValue::String("assistant".to_owned())),
            (
                "tool_calls",
                JsonValue::Array(vec![object([
                    ("id", JsonValue::String("x".to_owned())),
                    ("name", JsonValue::String("f".to_owned())),
                ])]),
            ),
        ])]),
    )]);
    collect(adapter.invoke(&record(&server.base_url), Capability::chat(), payload)).await;

    let body = server.last_request.lock().unwrap().clone().unwrap();
    let sent: JsonValue = serde_json::from_slice(&body).unwrap();
    let call = find_first_tool_call(&sent);
    let arguments = call
        .as_object()
        .unwrap()
        .get("function")
        .unwrap()
        .as_object()
        .unwrap()
        .get("arguments");
    // `{}`, never `null`.
    assert_eq!(arguments, Some(&JsonValue::String("{}".to_owned())));
}

fn find_first_tool_call(sent: &JsonValue) -> JsonValue {
    let JsonValue::Array(messages) = sent.as_object().unwrap().get("messages").unwrap() else {
        panic!("messages array");
    };
    let JsonValue::Array(calls) = messages[0].as_object().unwrap().get("tool_calls").unwrap()
    else {
        panic!("tool_calls array");
    };
    calls[0].clone()
}

#[tokio::test]
async fn a_full_gate_makes_invoke_unavailable() {
    let server = mock(|_p, _b| (200, "data: [DONE]\n\n".to_owned())).await;
    // A zero-slot gate: every request is refused before dialing.
    let adapter = OpenAiEndpointAdapter::with(
        Arc::new(StaticSecret("k")),
        Arc::new(EndpointConcurrencyGate::new(0)),
    );
    let (_c, error) = collect(adapter.invoke(
        &record(&server.base_url),
        Capability::chat(),
        chat_payload(),
    ))
    .await;
    assert!(matches!(error, Some(RuntimeError::Unavailable(_))));
}

#[tokio::test]
async fn a_base_url_with_a_v1_suffix_is_normalized() {
    let server = mock(|path, _b| {
        // The request must land on /v1/chat/completions, not /v1/v1/... .
        assert_eq!(path, "/v1/chat/completions");
        (200, "data: [DONE]\n\n".to_owned())
    })
    .await;
    let adapter = OpenAiEndpointAdapter::new();
    let mut rec = record(&format!("{}/v1", server.base_url));
    rec.source.repo = Some("m".to_owned());
    let (_c, error) = collect(adapter.invoke(&rec, Capability::chat(), chat_payload())).await;
    assert!(error.is_none());
}

#[test]
fn the_gate_limits_in_flight_requests_per_base() {
    let gate = Arc::new(EndpointConcurrencyGate::new(2));
    let a1 = gate.acquire("a");
    let a2 = gate.acquire("a");
    assert!(a1.is_some() && a2.is_some());
    // A third slot for the same base is refused...
    assert!(gate.acquire("a").is_none());
    // ...but a different base has its own budget.
    assert!(gate.acquire("b").is_some());
    // Dropping a guard frees the slot.
    drop(a1);
    assert!(gate.acquire("a").is_some());
}
