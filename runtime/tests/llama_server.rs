//! Tests for the local `llama-server` adapter: the effective-context math, the
//! bid/can_serve/honored surface, and the proxy path through a mock backend +
//! mock OpenAI server.

mod support;

use std::sync::{Arc, Mutex};

use kernel::capabilities::{CapabilityChunk, GenerationStats};
use kernel::records::{
    BidPreference, Capability, ExecutionMode, JsonValue, Modality, ModelRecord, ModelSource,
    RunTier, RuntimeId, SourceKind,
};
use kernel::resolution::{IdentifiedModel, ModelFormat, RuntimeBid, identify};
use runtime::adapters::{
    BackendFuture, ChunkStream, LlamaBackend, LlamaServerAdapter, RuntimeAdapter, RuntimeError,
    ServerMode,
};
use support::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::task::AbortHandle;

/// A backend that returns a fixed base URL (or a fixed error), recording the
/// context-token size and the server mode it was asked for.
struct MockBackend {
    result: Result<String, RuntimeError>,
    last_context: Arc<Mutex<Option<i64>>>,
    last_mode: Arc<Mutex<Option<ServerMode>>>,
}

impl LlamaBackend for MockBackend {
    fn base_url(
        &self,
        _record: &ModelRecord,
        context_tokens: i64,
        mode: ServerMode,
    ) -> BackendFuture {
        *self.last_context.lock().unwrap() = Some(context_tokens);
        *self.last_mode.lock().unwrap() = Some(mode);
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
    mock_answering(200, body).await
}

async fn mock_answering(status: u16, body: &'static str) -> MockServer {
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
                    "HTTP/1.1 {status} OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
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

/// An embedding server that answers each request with one vector per input,
/// `[n]` for an input named `input-n`, its rows in reverse order, and one
/// prompt token per input, a count it leaves out when an input is named
/// `no-usage`. An input whose name starts `too-long` gets llama-server's answer for an
/// input larger than its batch. Records each request's size.
async fn echo_embedder() -> (MockServer, Arc<Mutex<Vec<usize>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let sizes = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&sizes);
    let accept = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let seen = Arc::clone(&seen);
            tokio::spawn(async move {
                let Some(request) = read_request(&mut stream).await else {
                    return;
                };
                let inputs: Vec<String> = request["input"]
                    .as_array()
                    .map(|inputs| {
                        inputs
                            .iter()
                            .filter_map(|input| input.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default();
                seen.lock().unwrap().push(inputs.len());
                let (status, body) = if inputs.iter().any(|input| input.starts_with("too-long")) {
                    (
                        500,
                        serde_json::json!({"error": {"code": 500, "message": "input (3000 tokens) is too large to process. increase the physical batch size (current batch size: 2048)", "type": "server_error"}}),
                    )
                } else {
                    let rows: Vec<serde_json::Value> = inputs
                        .iter()
                        .enumerate()
                        .rev()
                        .map(|(index, input)| {
                            let n: f64 = input.trim_start_matches("input-").parse().unwrap_or(-1.0);
                            serde_json::json!({"object": "embedding", "index": index, "embedding": [n]})
                        })
                        .collect();
                    let body = if inputs.iter().any(|input| input == "no-usage") {
                        serde_json::json!({"data": rows})
                    } else {
                        serde_json::json!({"data": rows, "usage": {"prompt_tokens": inputs.len(), "total_tokens": inputs.len()}})
                    };
                    (200, body)
                };
                let body = body.to_string();
                let head = format!(
                    "HTTP/1.1 {status} OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes()).await;
                let _ = stream.write_all(body.as_bytes()).await;
                let _ = stream.flush().await;
            });
        }
    })
    .abort_handle();
    (
        MockServer {
            base_url: format!("http://{addr}"),
            accept,
        },
        sizes,
    )
}

/// What a holding embedding server saw on one connection.
#[derive(Debug, PartialEq)]
enum Held {
    /// A request carrying this many inputs, which it never answers.
    Request(usize),
    /// The client closed the connection.
    Closed,
}

/// An embedding server that reads each request and never answers it,
/// reporting the request and, later, the client closing its connection.
async fn holding_embedder() -> (MockServer, tokio::sync::mpsc::UnboundedReceiver<Held>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let (seen, held) = tokio::sync::mpsc::unbounded_channel();
    let accept = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let seen = seen.clone();
            tokio::spawn(async move {
                let Some(request) = read_request(&mut stream).await else {
                    return;
                };
                let inputs = request["input"].as_array().map_or(0, Vec::len);
                let _ = seen.send(Held::Request(inputs));
                let mut rest = [0u8; 64];
                while stream.read(&mut rest).await.is_ok_and(|n| n > 0) {}
                let _ = seen.send(Held::Closed);
            });
        }
    })
    .abort_handle();
    (
        MockServer {
            base_url: format!("http://{addr}"),
            accept,
        },
        held,
    )
}

/// Read one HTTP request and decode its JSON body.
async fn read_request(stream: &mut tokio::net::TcpStream) -> Option<serde_json::Value> {
    let mut buffer = Vec::new();
    let mut tmp = [0u8; 8192];
    let head_end = loop {
        let n = stream.read(&mut tmp).await.ok().filter(|n| *n > 0)?;
        buffer.extend_from_slice(&tmp[..n]);
        if let Some(at) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
            break at + 4;
        }
    };
    let head = String::from_utf8_lossy(&buffer[..head_end]).to_ascii_lowercase();
    let length: usize = head
        .lines()
        .find_map(|line| line.strip_prefix("content-length:"))
        .and_then(|value| value.trim().parse().ok())?;
    while buffer.len() < head_end + length {
        let n = stream.read(&mut tmp).await.ok().filter(|n| *n > 0)?;
        buffer.extend_from_slice(&tmp[..n]);
    }
    serde_json::from_slice(&buffer[head_end..head_end + length]).ok()
}

type Seen<T> = Arc<Mutex<Option<T>>>;

fn adapter_over(result: Result<String, RuntimeError>) -> (LlamaServerAdapter, Seen<i64>) {
    let (adapter, last_context, _) = adapter_watching(result);
    (adapter, last_context)
}

fn adapter_watching(
    result: Result<String, RuntimeError>,
) -> (LlamaServerAdapter, Seen<i64>, Seen<ServerMode>) {
    let last_context = Arc::new(Mutex::new(None));
    let last_mode = Arc::new(Mutex::new(None));
    let backend = MockBackend {
        result,
        last_context: Arc::clone(&last_context),
        last_mode: Arc::clone(&last_mode),
    };
    (
        LlamaServerAdapter::new(Arc::new(backend)),
        last_context,
        last_mode,
    )
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

fn embedder_record() -> ModelRecord {
    let mut rec = ModelRecord::new(
        "nomic-embed",
        Modality::embedding(),
        vec![Capability::embed()],
        ModelSource::new(SourceKind::file(), "/models/nomic-embed.gguf"),
    );
    rec.runtime.id = Some(RuntimeId::llama_cpp());
    rec.context_length = Some(2048);
    rec
}

fn embed_payload(inputs: &[&str]) -> JsonValue {
    JsonValue::Object(
        [(
            "input".to_owned(),
            JsonValue::Array(
                inputs
                    .iter()
                    .map(|input| JsonValue::String((*input).to_owned()))
                    .collect(),
            ),
        )]
        .into_iter()
        .collect(),
    )
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
fn bid_takes_a_gguf_that_chats_or_embeds() {
    let (adapter, _) = adapter_over(Ok("http://x".to_owned()));
    let rec = gguf_record();
    let identified = |format, modality, capabilities| {
        IdentifiedModel::new(format, Some(modality), capabilities, ExecutionMode::Stream)
    };
    let native = |bid: RuntimeBid| {
        assert_eq!(bid.tier, RunTier::Native);
        assert_eq!(bid.preference, BidPreference::LLAMA_CPP);
        assert!(bid.alternatives.is_empty());
    };

    let gguf_chat = identified(
        ModelFormat::Gguf,
        Modality::text(),
        vec![Capability::chat()],
    );
    native(
        adapter
            .bid(&rec, &gguf_chat)
            .expect("a chat gguf is bid on"),
    );

    let gguf_embed = identified(
        ModelFormat::Gguf,
        Modality::embedding(),
        vec![Capability::embed()],
    );
    native(
        adapter
            .bid(&rec, &gguf_embed)
            .expect("an embedding gguf is bid on"),
    );

    let component = identified(ModelFormat::Gguf, Modality::vision(), Vec::new());
    assert!(adapter.bid(&rec, &component).is_none());

    let safetensors_embedder = identified(
        ModelFormat::Safetensors,
        Modality::embedding(),
        vec![Capability::embed()],
    );
    assert!(adapter.bid(&rec, &safetensors_embedder).is_none());
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
fn can_serve_embeds_only_for_an_encoder_record() {
    let (adapter, _) = adapter_over(Ok("http://x".to_owned()));
    let mut encoder = embedder_record();
    assert!(adapter.can_serve(&encoder, &Capability::embed()));
    assert!(!adapter.can_serve(&encoder, &Capability::chat()));
    assert!(!adapter.can_serve(&encoder, &Capability::complete()));

    let chat = gguf_record();
    assert!(!adapter.can_serve(&chat, &Capability::embed()));
    assert!(adapter.can_serve(&chat, &Capability::chat()));
    assert!(adapter.can_serve(&chat, &Capability::complete()));

    encoder.runtime.id = Some(RuntimeId::ollama());
    assert!(!adapter.can_serve(&encoder, &Capability::embed()));
}

#[tokio::test]
async fn an_embed_invoke_launches_an_embedding_server_and_yields_a_vector_per_input() {
    let server = mock(
        r#"{"object":"list","data":[{"object":"embedding","index":1,"embedding":[0.3,0.4]},{"object":"embedding","index":0,"embedding":[0.1,0.2]}],"usage":{"prompt_tokens":5,"total_tokens":5}}"#,
    )
    .await;
    let (adapter, last_context, last_mode) = adapter_watching(Ok(server.base_url.clone()));
    let (chunks, error) = collect(adapter.invoke(
        &embedder_record(),
        Capability::embed(),
        embed_payload(&["hello", "world"]),
    ))
    .await;
    assert!(error.is_none(), "error: {error:?}");
    assert_eq!(
        chunks,
        vec![
            CapabilityChunk::Vector(vec![0.1, 0.2]),
            CapabilityChunk::Vector(vec![0.3, 0.4]),
            CapabilityChunk::Done(Some(GenerationStats {
                prompt_tokens: Some(5),
                ..Default::default()
            })),
        ]
    );
    assert_eq!(*last_mode.lock().unwrap(), Some(ServerMode::Embedding));
    assert_eq!(*last_context.lock().unwrap(), Some(2048));
}

#[tokio::test]
async fn an_embed_invoke_ignores_a_requested_context_length() {
    let server = mock(r#"{"data":[{"index":0,"embedding":[1.0]}]}"#).await;
    let (adapter, last_context, _) = adapter_watching(Ok(server.base_url.clone()));
    let mut payload = embed_payload(&["hello"]);
    if let JsonValue::Object(fields) = &mut payload {
        fields.insert("context_length".to_owned(), JsonValue::Int(512));
    }
    let (_, error) =
        collect(adapter.invoke(&embedder_record(), Capability::embed(), payload)).await;
    assert!(error.is_none(), "error: {error:?}");
    assert_eq!(*last_context.lock().unwrap(), Some(2048));
}

#[tokio::test]
async fn an_input_longer_than_the_window_is_the_callers_and_names_the_window() {
    let server = mock_answering(
        500,
        r#"{"error":{"code":500,"message":"input (2102 tokens) is too large to process. increase the physical batch size (current batch size: 2048)","type":"server_error"}}"#,
    )
    .await;
    let (adapter, _) = adapter_over(Ok(server.base_url.clone()));
    let (chunks, error) = collect(adapter.invoke(
        &embedder_record(),
        Capability::embed(),
        embed_payload(&["long"]),
    ))
    .await;
    assert!(chunks.is_empty());
    match error {
        Some(RuntimeError::Rejected(message)) => {
            assert!(message.contains("2102 tokens"), "{message}");
            assert!(message.contains("2048"), "{message}");
            assert!(!message.contains("batch size"), "{message}");
        }
        other => panic!("expected Rejected, got {other:?}"),
    }
}

#[tokio::test]
async fn a_model_that_cannot_embed_is_the_servers_failure_not_the_callers() {
    let server = mock_answering(
        400,
        r#"{"error":{"code":400,"message":"Pooling type 'none' is not OAI compatible. Please use a different pooling type","type":"invalid_request_error"}}"#,
    )
    .await;
    let (adapter, _) = adapter_over(Ok(server.base_url.clone()));
    let (_, error) = collect(adapter.invoke(
        &embedder_record(),
        Capability::embed(),
        embed_payload(&["hello"]),
    ))
    .await;
    match error {
        Some(RuntimeError::Failed(message)) => {
            assert!(message.contains("pools nothing"), "{message}");
            assert!(!message.contains("Please"), "{message}");
        }
        other => panic!("expected Failed, got {other:?}"),
    }
}

#[tokio::test]
async fn a_large_batch_is_sent_in_chunks_and_merged_in_order() {
    let (server, sizes) = echo_embedder().await;
    let (adapter, _) = adapter_over(Ok(server.base_url.clone()));
    let inputs: Vec<String> = (0..2100).map(|n| format!("input-{n}")).collect();
    let names: Vec<&str> = inputs.iter().map(String::as_str).collect();
    let (chunks, error) = collect(adapter.invoke(
        &embedder_record(),
        Capability::embed(),
        embed_payload(&names),
    ))
    .await;
    assert!(error.is_none(), "error: {error:?}");

    let mut expected: Vec<CapabilityChunk> = (0..2100)
        .map(|n| CapabilityChunk::Vector(vec![f64::from(n)]))
        .collect();
    expected.push(CapabilityChunk::Done(Some(GenerationStats {
        prompt_tokens: Some(2100),
        ..Default::default()
    })));
    assert_eq!(chunks, expected);

    let sizes = sizes.lock().unwrap().clone();
    assert_eq!(sizes.iter().sum::<usize>(), 2100);
    assert!(sizes.len() > 1, "one request carried the whole batch");
    assert!(sizes.iter().all(|size| *size <= 64), "{sizes:?}");
}

#[tokio::test]
async fn a_batch_of_long_inputs_carries_about_one_window_of_text_a_request() {
    let (server, sizes) = echo_embedder().await;
    let (adapter, _) = adapter_over(Ok(server.base_url.clone()));
    // A 2048-token window carries 8 KiB of text a request.
    let long = "x".repeat(3000);
    let names: Vec<&str> = std::iter::repeat_n(long.as_str(), 9)
        .chain(["short"])
        .collect();
    let (chunks, error) = collect(adapter.invoke(
        &embedder_record(),
        Capability::embed(),
        embed_payload(&names),
    ))
    .await;
    assert!(error.is_none(), "error: {error:?}");
    let vectors = chunks
        .iter()
        .filter(|chunk| matches!(chunk, CapabilityChunk::Vector(_)))
        .count();
    assert_eq!(vectors, 10);
    assert_eq!(*sizes.lock().unwrap(), [2, 2, 2, 2, 2]);
}

#[tokio::test]
async fn a_consumer_that_goes_away_closes_the_request_in_flight_and_sends_no_more() {
    let (server, mut held) = holding_embedder().await;
    let (adapter, _) = adapter_over(Ok(server.base_url.clone()));
    let inputs: Vec<String> = (0..200).map(|n| format!("input-{n}")).collect();
    let names: Vec<&str> = inputs.iter().map(String::as_str).collect();
    let stream = adapter.invoke(
        &embedder_record(),
        Capability::embed(),
        embed_payload(&names),
    );
    let patience = std::time::Duration::from_secs(5);
    let first = tokio::time::timeout(patience, held.recv()).await.unwrap();
    assert_eq!(first, Some(Held::Request(64)));

    drop(stream);
    let next = tokio::time::timeout(patience, held.recv()).await;
    assert_eq!(
        next.expect("the request in flight was kept open"),
        Some(Held::Closed)
    );
    let more = tokio::time::timeout(std::time::Duration::from_millis(300), held.recv()).await;
    assert!(more.is_err(), "sent more after the consumer went: {more:?}");
}

#[tokio::test]
async fn a_chunk_that_fails_fails_the_whole_batch_and_names_the_input() {
    let (server, sizes) = echo_embedder().await;
    let (adapter, _) = adapter_over(Ok(server.base_url.clone()));
    let mut inputs: Vec<String> = (0..100).map(|n| format!("input-{n}")).collect();
    inputs[80] = "too-long".to_owned();
    let names: Vec<&str> = inputs.iter().map(String::as_str).collect();
    let (chunks, error) = collect(adapter.invoke(
        &embedder_record(),
        Capability::embed(),
        embed_payload(&names),
    ))
    .await;
    match error {
        Some(RuntimeError::Rejected(message)) => assert_eq!(
            message,
            "input 80 is 3000 tokens, more than the 2048 this model embeds at once"
        ),
        other => panic!("expected Rejected, got {other:?}"),
    }
    assert!(
        !chunks
            .iter()
            .any(|chunk| matches!(chunk, CapabilityChunk::Done(_))),
        "a failed batch must not end as done"
    );
    // The two chunks, then the failing chunk's inputs one at a time up to the
    // one refused; nothing after it is sent.
    let mut expected = vec![64, 36];
    expected.extend(std::iter::repeat_n(1, 17));
    assert_eq!(*sizes.lock().unwrap(), expected);
}

#[tokio::test]
async fn an_over_long_input_sent_alone_is_named_without_being_sent_again() {
    let (server, sizes) = echo_embedder().await;
    let (adapter, _) = adapter_over(Ok(server.base_url.clone()));
    let long = format!("too-long {}", "x".repeat(9000));
    let (_, error) = collect(adapter.invoke(
        &embedder_record(),
        Capability::embed(),
        embed_payload(&["input-0", &long, "input-2"]),
    ))
    .await;
    match error {
        Some(RuntimeError::Rejected(message)) => assert_eq!(
            message,
            "input 1 is 3000 tokens, more than the 2048 this model embeds at once"
        ),
        other => panic!("expected Rejected, got {other:?}"),
    }
    assert_eq!(*sizes.lock().unwrap(), [1, 1]);
}

#[tokio::test]
async fn a_batch_reports_no_token_count_when_a_chunk_leaves_it_out() {
    let (server, _) = echo_embedder().await;
    let (adapter, _) = adapter_over(Ok(server.base_url.clone()));
    let mut inputs: Vec<String> = (0..100).map(|n| format!("input-{n}")).collect();
    inputs[70] = "no-usage".to_owned();
    let names: Vec<&str> = inputs.iter().map(String::as_str).collect();
    let (chunks, error) = collect(adapter.invoke(
        &embedder_record(),
        Capability::embed(),
        embed_payload(&names),
    ))
    .await;
    assert!(error.is_none(), "error: {error:?}");
    assert_eq!(
        chunks.last(),
        Some(&CapabilityChunk::Done(Some(GenerationStats::default())))
    );
}

#[tokio::test]
async fn an_embedder_with_a_wide_window_is_launched_capped() {
    let server = mock(r#"{"data":[{"index":0,"embedding":[1.0]}]}"#).await;
    let (adapter, last_context, _) = adapter_watching(Ok(server.base_url.clone()));
    let mut record = embedder_record();
    record.context_length = Some(32768);
    let (_, error) =
        collect(adapter.invoke(&record, Capability::embed(), embed_payload(&["a"]))).await;
    assert!(error.is_none(), "error: {error:?}");
    assert_eq!(*last_context.lock().unwrap(), Some(8192));
    assert_eq!(adapter.effective_context_window(&record, None), Some(8192));
}

#[test]
fn a_record_claiming_only_what_no_server_mode_answers_is_never_served_as_chat() {
    let (adapter, _) = adapter_over(Ok("http://x".to_owned()));
    let mut judge = gguf_record();
    judge.capabilities = vec![Capability::judge()];
    for capability in [
        Capability::chat(),
        Capability::complete(),
        Capability::embed(),
        Capability::judge(),
    ] {
        assert!(!adapter.can_serve(&judge, &capability), "{capability:?}");
    }

    let mut pinned = gguf_record();
    pinned.capabilities.clear();
    assert!(adapter.can_serve(&pinned, &Capability::chat()));
    assert!(!adapter.can_serve(&pinned, &Capability::embed()));
}

#[tokio::test]
async fn a_chat_invoke_still_launches_a_chat_server() {
    let server =
        mock("data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\ndata: [DONE]\n\n").await;
    let (adapter, _, last_mode) = adapter_watching(Ok(server.base_url.clone()));
    let (chunks, error) =
        collect(adapter.invoke(&gguf_record(), Capability::chat(), chat_payload())).await;
    assert!(error.is_none());
    assert_eq!(chunks[0], CapabilityChunk::Text("hi".to_owned()));
    assert_eq!(*last_mode.lock().unwrap(), Some(ServerMode::Chat));
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

#[test]
fn a_pulled_hugging_face_repo_of_gguf_weights_is_bid_on() {
    // The shape `hedos pull` leaves behind: the record names the repo
    // directory and the weights sit in the snapshot under it.
    let dir = TempDir::new();
    let snapshot = dir.join("snapshots").join("rev1");
    std::fs::create_dir_all(&snapshot).expect("snapshot");
    let weights = snapshot.join("qwen2.5-0.5b-instruct-q4_k_m.gguf");
    std::fs::write(&weights, b"GGUF").expect("weights");

    let mut rec = ModelRecord::new(
        "Qwen2.5-0.5B-Instruct-GGUF",
        Modality::text(),
        Vec::new(),
        ModelSource::new(
            SourceKind::huggingface_cache(),
            dir.path().to_str().expect("path"),
        ),
    );
    rec.source.reference = Some("rev1".to_owned());
    rec.primary_weight_path = Some(weights.to_string_lossy().into_owned());

    let identified = identify(&rec);
    assert_eq!(identified.format, ModelFormat::Gguf);
    let (adapter, _) = adapter_over(Ok("http://x".to_owned()));
    let bid = adapter
        .bid(&rec, &identified)
        .expect("a repo of gguf weights is one llama.cpp can serve");
    assert_eq!(bid.tier, RunTier::Native);
}

#[tokio::test]
async fn a_server_that_stops_mid_request_is_reported_without_its_address() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let accept = tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let _ = read_request(&mut stream).await;
            drop(stream);
        }
    })
    .abort_handle();
    let server = MockServer {
        base_url: format!("http://{addr}"),
        accept,
    };
    let (adapter, _) = adapter_over(Ok(server.base_url.clone()));
    let (_, error) = collect(adapter.invoke(
        &embedder_record(),
        Capability::embed(),
        embed_payload(&["a", "b"]),
    ))
    .await;
    match error {
        Some(RuntimeError::Failed(message)) => {
            assert_eq!(message, "llama-server stopped before answering");
        }
        other => panic!("expected Failed, got {other:?}"),
    }
}
