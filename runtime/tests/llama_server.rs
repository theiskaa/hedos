//! Tests for the local `llama-server` adapter: the effective-context math, the
//! bid/can_serve/honored surface, and the proxy path through a mock backend +
//! mock OpenAI server.

use std::sync::{Arc, Mutex};

use kernel::capabilities::CapabilityChunk;
use kernel::records::{
    Capability, ExecutionMode, JsonValue, Modality, ModelRecord, ModelSource, RuntimeId, SourceKind,
};
use kernel::resolution::{IdentifiedModel, ModelFormat};
use runtime::adapters::{
    BackendFuture, ChunkStream, LlamaBackend, LlamaServerAdapter, RuntimeAdapter, RuntimeError,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::task::AbortHandle;

/// A backend that returns a fixed base URL (or a fixed error), recording the
/// context-token size it was asked for.
struct MockBackend {
    result: Result<String, RuntimeError>,
    last_context: Arc<Mutex<Option<i64>>>,
}

impl LlamaBackend for MockBackend {
    fn base_url(&self, _record: &ModelRecord, context_tokens: i64) -> BackendFuture {
        *self.last_context.lock().unwrap() = Some(context_tokens);
        let result = self.result.clone();
        Box::pin(async move { result })
    }
}

struct MockServer {
    base_url: String,
    accept: AbortHandle,
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.accept.abort();
    }
}

async fn mock(body: &'static str) -> MockServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let accept = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut tmp = [0u8; 4096];
                let mut buffer = Vec::new();
                while stream.read(&mut tmp).await.map(|n| n > 0).unwrap_or(false) {
                    buffer.extend_from_slice(&tmp);
                    if buffer.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes()).await;
                let _ = stream.write_all(body.as_bytes()).await;
                let _ = stream.flush().await;
            });
        }
    })
    .abort_handle();
    MockServer {
        base_url: format!("http://{addr}"),
        accept,
    }
}

fn adapter_over(
    result: Result<String, RuntimeError>,
) -> (LlamaServerAdapter, Arc<Mutex<Option<i64>>>) {
    let last_context = Arc::new(Mutex::new(None));
    let backend = MockBackend {
        result,
        last_context: Arc::clone(&last_context),
    };
    (LlamaServerAdapter::new(Arc::new(backend)), last_context)
}

fn gguf_record() -> ModelRecord {
    let mut rec = ModelRecord::new(
        "Llama 3",
        Modality::text(),
        vec![Capability::chat()],
        ModelSource::new(SourceKind::file(), "/models/llama3.gguf"),
    );
    rec.runtime.id = Some(RuntimeId::llama_cpp());
    rec
}

fn chat_payload() -> JsonValue {
    JsonValue::Object(
        [(
            "messages".to_owned(),
            JsonValue::Array(vec![JsonValue::Object(
                [
                    ("role".to_owned(), JsonValue::String("user".to_owned())),
                    ("content".to_owned(), JsonValue::String("hi".to_owned())),
                ]
                .into_iter()
                .collect(),
            )]),
        )]
        .into_iter()
        .collect(),
    )
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

#[test]
fn effective_context_clamps_into_the_model_window() {
    let mut rec = gguf_record();
    // No declared context → the 4096 default.
    rec.context_length = None;
    assert_eq!(
        LlamaServerAdapter::effective_context_tokens(&rec, None),
        4096
    );

    rec.context_length = Some(8192);
    // A request is honored within the window...
    assert_eq!(
        LlamaServerAdapter::effective_context_tokens(&rec, Some(2000)),
        2000
    );
    // ...clamped up to the window ceiling...
    assert_eq!(
        LlamaServerAdapter::effective_context_tokens(&rec, Some(100_000)),
        8192
    );
    // ...and up to the 512 floor.
    assert_eq!(
        LlamaServerAdapter::effective_context_tokens(&rec, Some(100)),
        512
    );

    // A huge window caps the default at 32768 but honors a larger explicit request.
    rec.context_length = Some(131_072);
    assert_eq!(
        LlamaServerAdapter::effective_context_tokens(&rec, None),
        32768
    );
    assert_eq!(
        LlamaServerAdapter::effective_context_tokens(&rec, Some(65_000)),
        65_000
    );

    // A tiny window pulls the floor down with it.
    rec.context_length = Some(300);
    assert_eq!(
        LlamaServerAdapter::effective_context_tokens(&rec, None),
        300
    );
}

#[test]
fn bid_requires_a_gguf_chat_model() {
    let (adapter, _) = adapter_over(Ok("http://x".to_owned()));
    let rec = gguf_record();
    let gguf_chat = IdentifiedModel::new(
        ModelFormat::Gguf,
        Some(Modality::text()),
        vec![Capability::chat()],
        ExecutionMode::Stream,
    );
    assert!(adapter.bid(&rec, &gguf_chat).is_some());

    // GGUF but no chat capability (e.g. an embedding gguf) → no bid.
    let gguf_embed = IdentifiedModel::new(
        ModelFormat::Gguf,
        Some(Modality::embedding()),
        vec![Capability::embed()],
        ExecutionMode::Stream,
    );
    assert!(adapter.bid(&rec, &gguf_embed).is_none());

    // Not a GGUF → no bid.
    let safetensors = IdentifiedModel::new(
        ModelFormat::Safetensors,
        Some(Modality::text()),
        vec![Capability::chat()],
        ExecutionMode::Stream,
    );
    assert!(adapter.bid(&rec, &safetensors).is_none());
}

#[test]
fn can_serve_requires_the_llama_runtime_and_chat() {
    let (adapter, _) = adapter_over(Ok("http://x".to_owned()));
    let mut rec = gguf_record();
    assert!(adapter.can_serve(&rec, &Capability::chat()));
    assert!(adapter.can_serve(&rec, &Capability::complete()));
    assert!(!adapter.can_serve(&rec, &Capability::embed()));
    // Pinned to another runtime → not served.
    rec.runtime.id = Some(RuntimeId::ollama());
    assert!(!adapter.can_serve(&rec, &Capability::chat()));
}

#[test]
fn honored_params_and_effective_window() {
    let (adapter, _) = adapter_over(Ok("http://x".to_owned()));
    let rec = gguf_record();
    let keys = adapter.honored_param_keys(&rec, &Capability::chat());
    assert!(keys.contains("temperature"));
    assert!(keys.contains("context_length"));
    assert!(
        adapter
            .honored_param_keys(&rec, &Capability::embed())
            .is_empty()
    );
    assert_eq!(
        adapter.effective_context_window(&rec, Some(1000)),
        Some(1000)
    );
}

#[tokio::test]
async fn invoke_proxies_through_the_backend_and_streams() {
    let server =
        mock("data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\ndata: [DONE]\n\n").await;
    let (adapter, last_context) = adapter_over(Ok(server.base_url.clone()));
    let mut rec = gguf_record();
    rec.context_length = Some(8192);

    let payload = JsonValue::Object(
        [
            (
                "messages".to_owned(),
                chat_payload()
                    .as_object()
                    .unwrap()
                    .get("messages")
                    .unwrap()
                    .clone(),
            ),
            ("context_length".to_owned(), JsonValue::Int(2048)),
        ]
        .into_iter()
        .collect(),
    );
    let (chunks, error) = collect(adapter.invoke(&rec, Capability::chat(), payload)).await;
    assert!(error.is_none());
    assert_eq!(chunks[0], CapabilityChunk::Text("hi".to_owned()));
    // The backend was asked to size the server to the requested context.
    assert_eq!(*last_context.lock().unwrap(), Some(2048));
}

#[tokio::test]
async fn invoke_without_a_requested_context_uses_the_capped_default() {
    let server = mock("data: [DONE]\n\n").await;
    let (adapter, last_context) = adapter_over(Ok(server.base_url.clone()));
    let mut rec = gguf_record();
    // A huge window → the backend is sized to the 32768 default, not the window.
    rec.context_length = Some(131_072);
    // Payload carries no `context_length`.
    collect(adapter.invoke(&rec, Capability::chat(), chat_payload())).await;
    assert_eq!(*last_context.lock().unwrap(), Some(32768));
}

#[tokio::test]
async fn a_stream_without_a_done_marker_still_delivers_content() {
    // The server emits content then closes without `data: [DONE]`.
    let server = mock("data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n").await;
    let (adapter, _) = adapter_over(Ok(server.base_url.clone()));
    let (chunks, error) =
        collect(adapter.invoke(&gguf_record(), Capability::chat(), chat_payload())).await;
    assert!(error.is_none());
    assert_eq!(chunks[0], CapabilityChunk::Text("partial".to_owned()));
}

#[tokio::test]
async fn a_prompt_payload_is_served() {
    let server =
        mock("data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\ndata: [DONE]\n\n").await;
    let (adapter, _) = adapter_over(Ok(server.base_url.clone()));
    let payload = JsonValue::Object(
        [("prompt".to_owned(), JsonValue::String("hello".to_owned()))]
            .into_iter()
            .collect(),
    );
    let (chunks, error) =
        collect(adapter.invoke(&gguf_record(), Capability::chat(), payload)).await;
    assert!(error.is_none());
    assert_eq!(chunks[0], CapabilityChunk::Text("ok".to_owned()));
}

#[test]
fn honored_params_cover_the_complete_capability() {
    let (adapter, _) = adapter_over(Ok("http://x".to_owned()));
    let keys = adapter.honored_param_keys(&gguf_record(), &Capability::complete());
    assert!(keys.contains("temperature"));
    assert!(keys.contains("context_length"));
}
