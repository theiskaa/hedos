//! Tests for the `LlamaServerPool`: readiness polling, per-model reuse/dedup,
//! separate servers per model, unhealthy timeout, dead-process respawn, and the
//! full adapter→pool→server chat path — all against a mock spawner that binds the
//! allocated port with a real loopback HTTP server.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use kernel::capabilities::CapabilityChunk;
use kernel::records::{
    Capability, JsonValue, Modality, ModelRecord, ModelSource, RuntimeId, SourceKind,
};
use runtime::adapters::{
    ChunkStream, LlamaBackend, LlamaServerAdapter, LlamaServerPool, RuntimeAdapter, RuntimeError,
    ServerProcess, ServerSpawner,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::task::AbortHandle;

/// A spawner that binds the requested port with a loopback HTTP server answering
/// `/health` and `/v1/chat/completions`. Records spawn count + the alive flags so
/// tests can simulate a crash.
struct MockSpawner {
    count: Arc<AtomicUsize>,
    healthy: bool,
    alive_flags: Arc<Mutex<Vec<Arc<AtomicBool>>>>,
}

impl MockSpawner {
    fn new(healthy: bool) -> Self {
        Self {
            count: Arc::new(AtomicUsize::new(0)),
            healthy,
            alive_flags: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl ServerSpawner for MockSpawner {
    fn spawn(
        &self,
        _gguf_path: &str,
        port: u16,
        _context_tokens: i64,
    ) -> Result<Box<dyn ServerProcess>, RuntimeError> {
        self.count.fetch_add(1, Ordering::SeqCst);
        let std_listener = std::net::TcpListener::bind(("127.0.0.1", port))
            .map_err(|error| RuntimeError::Unavailable(error.to_string()))?;
        std_listener
            .set_nonblocking(true)
            .map_err(|error| RuntimeError::Unavailable(error.to_string()))?;
        let listener = tokio::net::TcpListener::from_std(std_listener)
            .map_err(|error| RuntimeError::Unavailable(error.to_string()))?;
        let healthy = self.healthy;
        let accept = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                tokio::spawn(async move { serve(&mut stream, healthy).await });
            }
        })
        .abort_handle();

        let alive = Arc::new(AtomicBool::new(true));
        self.alive_flags.lock().unwrap().push(Arc::clone(&alive));
        Ok(Box::new(MockProcess { accept, alive }))
    }
}

async fn serve(stream: &mut tokio::net::TcpStream, healthy: bool) {
    let mut buffer = Vec::new();
    let mut tmp = [0u8; 2048];
    loop {
        let Ok(n) = stream.read(&mut tmp).await else {
            return;
        };
        if n == 0 {
            return;
        }
        buffer.extend_from_slice(&tmp[..n]);
        if buffer.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }
    let head = String::from_utf8_lossy(&buffer);
    let path = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/");

    let (status, body): (u16, String) = if path == "/health" {
        if healthy {
            (200, "OK".to_owned())
        } else {
            (503, "loading".to_owned())
        }
    } else if path == "/v1/chat/completions" {
        (
            200,
            "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\ndata: [DONE]\n\n".to_owned(),
        )
    } else {
        (404, "no".to_owned())
    };
    let response = format!(
        "HTTP/1.1 {status} OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.flush().await;
}

struct MockProcess {
    accept: AbortHandle,
    alive: Arc<AtomicBool>,
}

impl ServerProcess for MockProcess {
    fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }
}

impl Drop for MockProcess {
    fn drop(&mut self) {
        self.accept.abort();
    }
}

fn record(name: &str) -> ModelRecord {
    let mut rec = ModelRecord::new(
        name,
        Modality::text(),
        vec![Capability::chat()],
        ModelSource::new(SourceKind::file(), &format!("/models/{name}.gguf")),
    );
    rec.runtime.id = Some(RuntimeId::llama_cpp());
    rec
}

fn fast_pool(spawner: Arc<MockSpawner>) -> LlamaServerPool {
    LlamaServerPool::new(spawner).with_ready_timeout(Duration::from_secs(2))
}

#[tokio::test]
async fn spawns_a_ready_server_and_returns_its_url() {
    let spawner = Arc::new(MockSpawner::new(true));
    let pool = fast_pool(Arc::clone(&spawner));

    let base = pool.base_url(&record("a"), 4096).await.expect("ready");
    assert!(base.starts_with("http://127.0.0.1:"));
    assert_eq!(spawner.count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn reuses_a_running_server_for_the_same_model() {
    let spawner = Arc::new(MockSpawner::new(true));
    let pool = fast_pool(Arc::clone(&spawner));
    let rec = record("a");

    let first = pool.base_url(&rec, 4096).await.expect("ready");
    let second = pool.base_url(&rec, 4096).await.expect("ready");
    assert_eq!(first, second);
    // Only one process was ever spawned.
    assert_eq!(spawner.count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn spawns_a_separate_server_per_model() {
    let spawner = Arc::new(MockSpawner::new(true));
    let pool = fast_pool(Arc::clone(&spawner));

    let a = pool.base_url(&record("a"), 4096).await.expect("ready");
    let b = pool.base_url(&record("b"), 4096).await.expect("ready");
    assert_ne!(a, b);
    assert_eq!(spawner.count.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn an_unhealthy_server_times_out_as_unavailable() {
    let spawner = Arc::new(MockSpawner::new(false));
    // Short timeout so the never-ready server fails fast.
    let pool = LlamaServerPool::new(spawner).with_ready_timeout(Duration::from_millis(400));
    let error = pool.base_url(&record("a"), 4096).await.unwrap_err();
    assert!(matches!(error, RuntimeError::Unavailable(_)));
}

#[tokio::test]
async fn a_dead_process_is_respawned() {
    let spawner = Arc::new(MockSpawner::new(true));
    let pool = fast_pool(Arc::clone(&spawner));
    let rec = record("a");

    pool.base_url(&rec, 4096).await.expect("ready");
    assert_eq!(spawner.count.load(Ordering::SeqCst), 1);
    // Simulate the server crashing: flip its alive flag.
    spawner.alive_flags.lock().unwrap()[0].store(false, Ordering::SeqCst);
    pool.base_url(&rec, 4096).await.expect("respawn");
    assert_eq!(spawner.count.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn concurrent_requests_for_one_model_spawn_exactly_once() {
    let spawner = Arc::new(MockSpawner::new(true));
    let pool = Arc::new(fast_pool(Arc::clone(&spawner)));

    // Two requests for the same model race; the second must wait on the slot and
    // reuse the server the first spawns — not double-launch.
    let (a, b) = tokio::join!(
        {
            let pool = Arc::clone(&pool);
            async move { pool.base_url(&record("a"), 4096).await }
        },
        {
            let pool = Arc::clone(&pool);
            async move { pool.base_url(&record("a"), 4096).await }
        },
    );
    assert_eq!(a.expect("ready"), b.expect("ready"));
    assert_eq!(spawner.count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_larger_context_request_respawns_but_a_covered_one_reuses() {
    let spawner = Arc::new(MockSpawner::new(true));
    let pool = fast_pool(Arc::clone(&spawner));
    let rec = record("a");

    pool.base_url(&rec, 2048).await.expect("ready");
    assert_eq!(spawner.count.load(Ordering::SeqCst), 1);
    // A larger window than the running 2048 → respawn.
    pool.base_url(&rec, 4096).await.expect("respawn");
    assert_eq!(spawner.count.load(Ordering::SeqCst), 2);
    // A smaller window is covered by the running 4096 → reuse.
    pool.base_url(&rec, 1024).await.expect("reuse");
    assert_eq!(spawner.count.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn evict_forces_a_respawn() {
    let spawner = Arc::new(MockSpawner::new(true));
    let pool = fast_pool(Arc::clone(&spawner));
    let rec = record("a");

    pool.base_url(&rec, 4096).await.expect("ready");
    assert_eq!(spawner.count.load(Ordering::SeqCst), 1);
    pool.evict(&rec.id).await;
    pool.base_url(&rec, 4096).await.expect("respawn");
    assert_eq!(spawner.count.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn a_process_dead_on_arrival_is_unavailable() {
    struct DeadSpawner;
    impl ServerSpawner for DeadSpawner {
        fn spawn(
            &self,
            _gguf: &str,
            _port: u16,
            _ctx: i64,
        ) -> Result<Box<dyn ServerProcess>, RuntimeError> {
            Ok(Box::new(DeadProcess))
        }
    }
    struct DeadProcess;
    impl ServerProcess for DeadProcess {
        fn is_alive(&self) -> bool {
            false
        }
    }
    // The readiness poll sees the process already exited → Unavailable, fast.
    let pool =
        LlamaServerPool::new(Arc::new(DeadSpawner)).with_ready_timeout(Duration::from_secs(5));
    let error = pool.base_url(&record("a"), 4096).await.unwrap_err();
    assert!(matches!(error, RuntimeError::Unavailable(_)));
}

#[tokio::test]
async fn a_spawn_failure_is_forwarded() {
    struct FailingSpawner;
    impl ServerSpawner for FailingSpawner {
        fn spawn(
            &self,
            _gguf: &str,
            _port: u16,
            _ctx: i64,
        ) -> Result<Box<dyn ServerProcess>, RuntimeError> {
            Err(RuntimeError::Unavailable("no binary".to_owned()))
        }
    }
    let pool = LlamaServerPool::new(Arc::new(FailingSpawner));
    let error = pool.base_url(&record("a"), 4096).await.unwrap_err();
    assert!(matches!(error, RuntimeError::Unavailable(_)));
}

#[tokio::test]
async fn the_adapter_serves_a_chat_end_to_end_through_the_pool() {
    let spawner = Arc::new(MockSpawner::new(true));
    let pool = fast_pool(spawner);
    let adapter = LlamaServerAdapter::new(Arc::new(pool));

    let payload = JsonValue::Object(
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
    );
    let (chunks, error) = collect(adapter.invoke(&record("a"), Capability::chat(), payload)).await;
    assert!(error.is_none());
    assert_eq!(chunks[0], CapabilityChunk::Text("hi".to_owned()));
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
