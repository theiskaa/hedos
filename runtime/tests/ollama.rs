//! Tests for the Ollama adapter, driven by a mock HTTP server (a `tokio` TCP
//! listener) that captures the request and returns canned responses.

use std::sync::{Arc, Mutex};

use kernel::capabilities::CapabilityChunk;
use kernel::records::{Capability, JsonValue, Modality, ModelRecord, ModelSource, SourceKind};
use runtime::adapters::{ChunkStream, OllamaAdapter, RuntimeAdapter, RuntimeError};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::task::AbortHandle;

/// A mock Ollama server. `handler(path, request_body) -> (status, response_body)`.
struct MockOllama {
    base_url: String,
    last_request: Arc<Mutex<Option<Vec<u8>>>>,
    accept: AbortHandle,
}

impl Drop for MockOllama {
    fn drop(&mut self) {
        self.accept.abort();
    }
}

async fn mock(
    handler: impl Fn(&str, &[u8]) -> (u16, String) + Send + Sync + 'static,
) -> MockOllama {
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
                        "HTTP/1.1 {status} OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n",
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
    MockOllama {
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

fn record() -> ModelRecord {
    ModelRecord::new(
        "llama3.2",
        Modality::text(),
        vec![Capability::chat()],
        ModelSource::new(SourceKind::ollama(), "llama3.2"),
    )
}

fn chat_payload(content: &str) -> JsonValue {
    let message = object([
        ("role", JsonValue::String("user".to_owned())),
        ("content", JsonValue::String(content.to_owned())),
    ]);
    object([("messages", JsonValue::Array(vec![message]))])
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

#[tokio::test]
async fn streams_a_chat_completion() {
    let server = mock(|path, _body| {
        assert_eq!(path, "/api/chat");
        (
            200,
            concat!(
                "{\"message\":{\"content\":\"Hello\"}}\n",
                "{\"message\":{\"content\":\" world\"}}\n",
                "{\"done\":true,\"prompt_eval_count\":5,\"eval_count\":2,\"total_duration\":2000000,\"done_reason\":\"stop\"}\n"
            )
            .to_owned(),
        )
    })
    .await;
    let adapter = OllamaAdapter::with_base_url(&server.base_url);

    let stream = adapter.invoke(&record(), Capability::chat(), chat_payload("hi"));
    let (chunks, error) = collect(stream).await;
    assert!(error.is_none(), "no error: {error:?}");

    let text: String = chunks
        .iter()
        .filter_map(|c| match c {
            CapabilityChunk::Text(t) => Some(t.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text, "Hello world");
    let stats = chunks.iter().rev().find_map(|c| match c {
        CapabilityChunk::Done(stats) => stats.clone(),
        _ => None,
    });
    let stats = stats.expect("done stats");
    assert_eq!(stats.prompt_tokens, Some(5));
    assert_eq!(stats.completion_tokens, Some(2));
    assert_eq!(stats.duration_ms, Some(2));
    assert_eq!(stats.finish_reason.as_deref(), Some("stop"));

    // The request body carried the streaming flag, the model, and the messages.
    let request = server
        .last_request
        .lock()
        .unwrap()
        .clone()
        .expect("request");
    let body: JsonValue = serde_json::from_slice(&request).expect("json body");
    let fields = body.as_object().expect("object");
    assert_eq!(fields.get("stream"), Some(&JsonValue::Bool(true)));
    assert_eq!(
        fields.get("model"),
        Some(&JsonValue::String("llama3.2".to_owned()))
    );
    assert!(
        fields
            .get("messages")
            .and_then(JsonValue::as_array)
            .is_some()
    );
}

#[tokio::test]
async fn yields_tool_calls() {
    let server = mock(|_path, _body| {
        (
            200,
            concat!(
                "{\"message\":{\"tool_calls\":[{\"function\":{\"name\":\"get_weather\",\"arguments\":{\"city\":\"Paris\"}}}]}}\n",
                "{\"done\":true}\n"
            )
            .to_owned(),
        )
    })
    .await;
    let adapter = OllamaAdapter::with_base_url(&server.base_url);

    let (chunks, error) =
        collect(adapter.invoke(&record(), Capability::chat(), chat_payload("weather?"))).await;
    assert!(error.is_none());
    let call = chunks.iter().find_map(|c| match c {
        CapabilityChunk::ToolCall(call) => Some(call),
        _ => None,
    });
    let call = call.expect("tool call");
    assert_eq!(call.name, "get_weather");
    assert_eq!(
        call.arguments.as_object().and_then(|a| a.get("city")),
        Some(&JsonValue::String("Paris".to_owned()))
    );
}

#[tokio::test]
async fn surfaces_a_stream_error_line() {
    let server = mock(|_path, _body| (200, "{\"error\":\"model not found\"}\n".to_owned())).await;
    let adapter = OllamaAdapter::with_base_url(&server.base_url);
    let (_chunks, error) =
        collect(adapter.invoke(&record(), Capability::chat(), chat_payload("hi"))).await;
    assert!(
        matches!(error, Some(RuntimeError::Failed(ref m)) if m.contains("model not found")),
        "{error:?}"
    );
}

#[tokio::test]
async fn surfaces_an_http_error() {
    let server = mock(|_path, _body| (404, "{\"error\":\"boom\"}".to_owned())).await;
    let adapter = OllamaAdapter::with_base_url(&server.base_url);
    let (_chunks, error) =
        collect(adapter.invoke(&record(), Capability::chat(), chat_payload("hi"))).await;
    assert!(
        matches!(error, Some(RuntimeError::Failed(ref m)) if m.contains("boom")),
        "{error:?}"
    );
}

#[tokio::test]
async fn streams_embeddings() {
    let server = mock(|path, _body| {
        assert_eq!(path, "/api/embed");
        (
            200,
            "{\"embeddings\":[[0.1,0.2],[0.3,0.4]],\"prompt_eval_count\":4}".to_owned(),
        )
    })
    .await;
    let adapter = OllamaAdapter::with_base_url(&server.base_url);

    let payload = object([("input", JsonValue::String("hello".to_owned()))]);
    let (chunks, error) = collect(adapter.invoke(&record(), Capability::embed(), payload)).await;
    assert!(error.is_none());
    let vectors: Vec<Vec<f64>> = chunks
        .iter()
        .filter_map(|c| match c {
            CapabilityChunk::Vector(v) => Some(v.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(vectors, vec![vec![0.1, 0.2], vec![0.3, 0.4]]);
    let stats = chunks.iter().rev().find_map(|c| match c {
        CapabilityChunk::Done(stats) => stats.clone(),
        _ => None,
    });
    assert_eq!(stats.expect("done").prompt_tokens, Some(4));
}

#[tokio::test]
async fn a_down_daemon_reports_unavailable() {
    // Bind then drop to get a very-likely-free port.
    let dead_port = {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        listener.local_addr().unwrap().port()
    };
    let adapter = OllamaAdapter::with_base_url(format!("http://127.0.0.1:{dead_port}"));
    let (_chunks, error) =
        collect(adapter.invoke(&record(), Capability::chat(), chat_payload("hi"))).await;
    assert!(
        matches!(error, Some(RuntimeError::Unavailable(_))),
        "{error:?}"
    );
}

#[tokio::test]
async fn a_malformed_embedding_element_is_rejected() {
    let server =
        mock(|_path, _body| (200, "{\"embeddings\":[[0.1,0.2],[\"bad\"]]}".to_owned())).await;
    let adapter = OllamaAdapter::with_base_url(&server.base_url);
    let payload = object([("input", JsonValue::String("x".to_owned()))]);
    let (_chunks, error) = collect(adapter.invoke(&record(), Capability::embed(), payload)).await;
    assert!(
        matches!(error, Some(RuntimeError::Failed(ref m)) if m.contains("not understood")),
        "a non-numeric embedding fails the whole response: {error:?}"
    );
}

#[tokio::test]
async fn a_multiline_error_body_is_parsed() {
    let server = mock(|_path, _body| (500, "not json\n{\"error\":\"deep boom\"}".to_owned())).await;
    let adapter = OllamaAdapter::with_base_url(&server.base_url);
    let (_chunks, error) =
        collect(adapter.invoke(&record(), Capability::chat(), chat_payload("hi"))).await;
    assert!(
        matches!(error, Some(RuntimeError::Failed(ref m)) if m.contains("deep boom")),
        "the error on a later line is still extracted: {error:?}"
    );
}

#[test]
fn can_serve_rules() {
    let adapter = OllamaAdapter::new();
    let mut chat_only = record();
    chat_only.capabilities = vec![Capability::chat()];

    assert!(adapter.can_serve(&chat_only, &Capability::chat()));
    assert!(adapter.can_serve(&chat_only, &Capability::complete()));
    assert!(adapter.can_serve(&chat_only, &Capability::embed()));
    assert!(
        !adapter.can_serve(&chat_only, &Capability::speak()),
        "unsupported capability"
    );
    assert!(
        !adapter.can_serve(&chat_only, &Capability::see()),
        "see needs the see capability"
    );

    let mut vision = record();
    vision.capabilities = vec![Capability::chat(), Capability::see()];
    assert!(adapter.can_serve(&vision, &Capability::see()));

    let mut non_ollama = record();
    non_ollama.source = ModelSource::new(SourceKind::file(), "/tmp/x");
    assert!(
        !adapter.can_serve(&non_ollama, &Capability::chat()),
        "not an ollama model"
    );
}
