//! The full HTTP path: a real axum server over a mock port, driven with an HTTP
//! client. This is the end-to-end proof the gateway serves requests.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use common::MockPort;
use gateway::audit::{Auditing, GatewayAuditEntry, NoopAudit};
use gateway::auth::OpenAuth;
use gateway::port::GatewayPort;
use gateway::router::{GatewayRouter, standard_routes};
use gateway::server;
use kernel::capabilities::CapabilityChunk;
use tokio::net::TcpListener;

async fn start(port: MockPort) -> String {
    let router = Arc::new(GatewayRouter::new(
        Arc::new(port) as Arc<dyn GatewayPort>,
        Box::new(OpenAuth),
        Box::new(NoopAudit),
        standard_routes(),
        4,
    ));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(server::serve(listener, router));
    format!("http://{addr}")
}

#[tokio::test]
async fn the_version_endpoint_answers_over_http() {
    let base = start(MockPort::default()).await;
    let response = reqwest::get(format!("{base}/api/version")).await.unwrap();
    assert_eq!(response.status(), 200);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["version"], "0.5.0");
}

#[tokio::test]
async fn a_chat_completion_answers_over_http() {
    let (mut port, _id) = MockPort::with_ready_model("llama3");
    port.chunks = vec![
        CapabilityChunk::Text("hello".to_owned()),
        CapabilityChunk::Done(None),
    ];
    let base = start(port).await;

    let response = reqwest::Client::new()
        .post(format!("{base}/v1/chat/completions"))
        .header("Content-Type", "application/json")
        .body(r#"{"model":"llama3","messages":[{"role":"user","content":"hi"}],"stream":false}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["object"], "chat.completion");
    assert_eq!(body["choices"][0]["message"]["content"], "hello");
}

#[tokio::test]
async fn a_streaming_chat_answers_with_sse_over_http() {
    let (mut port, _id) = MockPort::with_ready_model("llama3");
    port.chunks = vec![
        CapabilityChunk::Text("a".to_owned()),
        CapabilityChunk::Text("b".to_owned()),
        CapabilityChunk::Done(None),
    ];
    let base = start(port).await;

    let response = reqwest::Client::new()
        .post(format!("{base}/v1/chat/completions"))
        .body(r#"{"model":"llama3","messages":[{"role":"user","content":"hi"}],"stream":true}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let text = response.text().await.unwrap();
    assert!(text.contains("chat.completion.chunk"));
    assert!(text.trim_end().ends_with("data: [DONE]"));
}

#[tokio::test]
async fn an_unknown_route_is_404_over_http() {
    let base = start(MockPort::default()).await;
    let response = reqwest::get(format!("{base}/nope")).await.unwrap();
    assert_eq!(response.status(), 404);
}

/// An audit sink that keeps every entry.
#[derive(Clone, Default)]
struct KeptAudit(Arc<std::sync::Mutex<Vec<GatewayAuditEntry>>>);

impl Auditing for KeptAudit {
    fn append(&self, entry: GatewayAuditEntry) {
        self.0.lock().unwrap().push(entry);
    }
}

/// A `flush`-recording audit sink.
struct FlagAudit(Arc<AtomicBool>);

impl Auditing for FlagAudit {
    fn append(&self, _entry: GatewayAuditEntry) {}
    fn flush(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn a_graceful_shutdown_flushes_the_audit_log() {
    let flushed = Arc::new(AtomicBool::new(false));
    let router = Arc::new(GatewayRouter::new(
        Arc::new(MockPort::default()) as Arc<dyn GatewayPort>,
        Box::new(OpenAuth),
        Box::new(FlagAudit(flushed.clone())),
        standard_routes(),
        4,
    ));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    // An already-resolved shutdown makes the server drain and return at once.
    server::serve_with_shutdown(listener, router, async {})
        .await
        .unwrap();
    assert!(flushed.load(Ordering::SeqCst));
}

#[tokio::test]
async fn a_cut_answers_what_is_unanswered_and_closes_what_streams() {
    let (mut port, _id) = MockPort::with_ready_model("llama3");
    port.chunks = vec![CapabilityChunk::Text("hel".to_owned())];
    port.open_streams = Some(std::sync::Mutex::new(Vec::new()));
    let port = Arc::new(port);
    let audit = KeptAudit::default();
    let router = Arc::new(GatewayRouter::new(
        Arc::clone(&port) as Arc<dyn GatewayPort>,
        Box::new(OpenAuth),
        Box::new(audit.clone()),
        standard_routes(),
        4,
    ));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let (cut, cut_off) = tokio::sync::oneshot::channel::<()>();
    let serving = tokio::spawn(server::serve_with_cut(
        listener,
        router,
        async {
            let _ = stopped.await;
        },
        async {
            let _ = cut_off.await;
        },
    ));

    let client = reqwest::Client::new();
    let chat = |stream: bool| {
        client
            .post(format!("{base}/v1/chat/completions"))
            .header("Content-Type", "application/json")
            .body(format!(
                r#"{{"model":"llama3","messages":[{{"role":"user","content":"hi"}}],"stream":{stream}}}"#
            ))
            .send()
    };
    let streamed = chat(true).await.unwrap();
    assert_eq!(streamed.status(), 200);
    let unanswered = tokio::spawn(chat(false));
    let open = || port.open_streams.as_ref().unwrap().lock().unwrap().len();
    let both_open = async {
        while open() < 2 {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    };
    tokio::time::timeout(std::time::Duration::from_secs(10), both_open)
        .await
        .expect("both requests reached the model");

    let _ = stop.send(());
    let _ = cut.send(());
    let body = streamed
        .text()
        .await
        .expect("a streamed body that ends cleanly");
    assert!(body.contains("hel"), "{body}");
    let unanswered = unanswered.await.unwrap().unwrap();
    assert_eq!(unanswered.status(), 503);
    let refusal: serde_json::Value = unanswered.json().await.unwrap();
    assert_eq!(refusal["error"]["message"], "the gateway is stopping");
    tokio::time::timeout(std::time::Duration::from_secs(5), serving)
        .await
        .expect("the server ends once the cut requests are answered")
        .unwrap()
        .unwrap();
    let streams = port.open_streams.as_ref().unwrap().lock().unwrap();
    assert!(
        streams.iter().all(|sender| sender.is_closed()),
        "the work behind a cut request was kept"
    );
    let entries = audit.0.lock().unwrap();
    assert_eq!(entries.len(), 2, "{entries:?}");
    for entry in entries.iter() {
        assert_eq!(entry.route, "/v1/chat/completions");
        assert_eq!((entry.status, entry.outcome.as_str()), (503, "saturated"));
        assert_eq!(entry.detail.as_deref(), Some("the gateway is stopping"));
        assert_eq!(entry.model.as_deref(), Some("llama3"));
        assert_eq!(entry.client.as_deref(), Some("local"));
    }
}
