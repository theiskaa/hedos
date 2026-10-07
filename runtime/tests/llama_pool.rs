//! Tests for the `LlamaServerPool`: readiness polling, per-model reuse/dedup,
//! separate servers per model, unhealthy timeout, dead-process respawn, and the
//! full adapter→pool→server chat path — all against a mock spawner that binds the
//! allocated port with a real loopback HTTP server.

mod support;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use kernel::capabilities::CapabilityChunk;
use kernel::records::{
    Capability, JsonValue, Modality, ModelRecord, ModelSource, RuntimeId, SourceKind,
};
use runtime::adapters::{
    ChunkStream, LlamaBackend, LlamaServerAdapter, LlamaServerPool, LlamaServerSpawner,
    RuntimeAdapter, RuntimeError, ServerLaunch, ServerMode, ServerProcess, ServerSpawner,
};
use support::{TempDir, gguf, kv_string, projector_gguf};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::task::AbortHandle;

/// A spawner that binds the requested port with a loopback HTTP server answering
/// `/health` and `/v1/chat/completions`. Records spawn count + the alive flags so
/// tests can simulate a crash.
struct MockSpawner {
    count: Arc<AtomicUsize>,
    modes: Arc<Mutex<Vec<ServerMode>>>,
    projectors: Arc<Mutex<Vec<Option<String>>>>,
    healthy: bool,
    alive_flags: Arc<Mutex<Vec<Arc<AtomicBool>>>>,
    health_flags: Arc<Mutex<Vec<Arc<AtomicBool>>>>,
}

impl MockSpawner {
    fn new(healthy: bool) -> Self {
        Self {
            count: Arc::new(AtomicUsize::new(0)),
            modes: Arc::new(Mutex::new(Vec::new())),
            projectors: Arc::new(Mutex::new(Vec::new())),
            healthy,
            alive_flags: Arc::new(Mutex::new(Vec::new())),
            health_flags: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl ServerSpawner for MockSpawner {
    fn spawn(&self, launch: &ServerLaunch<'_>) -> Result<Box<dyn ServerProcess>, RuntimeError> {
        self.count.fetch_add(1, Ordering::SeqCst);
        self.modes.lock().unwrap().push(launch.mode);
        self.projectors
            .lock()
            .unwrap()
            .push(launch.projector.map(str::to_owned));
        // The pool frees the port just before this binds it, and a server it
        // replaced may have held it a moment ago; like llama-server, take it
        // over rather than wait out the closed connections' TIME_WAIT.
        let unavailable = |error: std::io::Error| RuntimeError::Unavailable(error.to_string());
        let socket = tokio::net::TcpSocket::new_v4().map_err(unavailable)?;
        socket.set_reuseaddr(true).map_err(unavailable)?;
        socket
            .bind(std::net::SocketAddr::from(([127, 0, 0, 1], launch.port)))
            .map_err(unavailable)?;
        let listener = socket.listen(64).map_err(unavailable)?;
        let health = Arc::new(AtomicBool::new(self.healthy));
        self.health_flags.lock().unwrap().push(Arc::clone(&health));
        let accept = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let health = Arc::clone(&health);
                tokio::spawn(async move { serve(&mut stream, &health).await });
            }
        })
        .abort_handle();

        let alive = Arc::new(AtomicBool::new(true));
        self.alive_flags.lock().unwrap().push(Arc::clone(&alive));
        Ok(Box::new(MockProcess { accept, alive }))
    }
}

async fn serve(stream: &mut tokio::net::TcpStream, healthy: &AtomicBool) {
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
        if healthy.load(Ordering::SeqCst) {
            (200, "OK".to_owned())
        } else {
            (503, "loading".to_owned())
        }
    } else if path == "/v1/embeddings" {
        (
            200,
            r#"{"data":[{"index":0,"embedding":[0.5,0.25]}],"usage":{"prompt_tokens":2}}"#
                .to_owned(),
        )
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

    let base = pool
        .base_url(&record("a"), 4096, ServerMode::Chat)
        .await
        .expect("ready");
    assert!(base.starts_with("http://127.0.0.1:"));
    assert_eq!(spawner.count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn reuses_a_running_server_for_the_same_model() {
    let spawner = Arc::new(MockSpawner::new(true));
    let pool = fast_pool(Arc::clone(&spawner));
    let rec = record("a");

    let first = pool
        .base_url(&rec, 4096, ServerMode::Chat)
        .await
        .expect("ready");
    let second = pool
        .base_url(&rec, 4096, ServerMode::Chat)
        .await
        .expect("ready");
    assert_eq!(first, second);
    // Only one process was ever spawned.
    assert_eq!(spawner.count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn respawns_a_wedged_but_alive_server() {
    let spawner = Arc::new(MockSpawner::new(true));
    let pool = fast_pool(Arc::clone(&spawner));
    let rec = record("a");

    let first = pool
        .base_url(&rec, 4096, ServerMode::Chat)
        .await
        .expect("ready");
    assert_eq!(spawner.count.load(Ordering::SeqCst), 1);

    // The server wedges: its process stays alive but `/health` stops answering.
    spawner.health_flags.lock().unwrap()[0].store(false, Ordering::SeqCst);

    // The next request must not recycle it — a fresh (healthy) server is spawned.
    let second = pool
        .base_url(&rec, 4096, ServerMode::Chat)
        .await
        .expect("respawn");
    assert_ne!(first, second);
    assert_eq!(spawner.count.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn spawns_a_separate_server_per_model() {
    let spawner = Arc::new(MockSpawner::new(true));
    let pool = fast_pool(Arc::clone(&spawner));

    let a = pool
        .base_url(&record("a"), 4096, ServerMode::Chat)
        .await
        .expect("ready");
    let b = pool
        .base_url(&record("b"), 4096, ServerMode::Chat)
        .await
        .expect("ready");
    assert_ne!(a, b);
    assert_eq!(spawner.count.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn an_unhealthy_server_times_out_as_unavailable() {
    let spawner = Arc::new(MockSpawner::new(false));
    // Short timeout so the never-ready server fails fast.
    let pool = LlamaServerPool::new(spawner).with_ready_timeout(Duration::from_millis(400));
    let error = pool
        .base_url(&record("a"), 4096, ServerMode::Chat)
        .await
        .unwrap_err();
    assert!(matches!(error, RuntimeError::Unavailable(_)));
}

#[tokio::test]
async fn a_dead_process_is_respawned() {
    let spawner = Arc::new(MockSpawner::new(true));
    let pool = fast_pool(Arc::clone(&spawner));
    let rec = record("a");

    pool.base_url(&rec, 4096, ServerMode::Chat)
        .await
        .expect("ready");
    assert_eq!(spawner.count.load(Ordering::SeqCst), 1);
    // Simulate the server crashing: flip its alive flag.
    spawner.alive_flags.lock().unwrap()[0].store(false, Ordering::SeqCst);
    pool.base_url(&rec, 4096, ServerMode::Chat)
        .await
        .expect("respawn");
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
            async move { pool.base_url(&record("a"), 4096, ServerMode::Chat).await }
        },
        {
            let pool = Arc::clone(&pool);
            async move { pool.base_url(&record("a"), 4096, ServerMode::Chat).await }
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

    pool.base_url(&rec, 2048, ServerMode::Chat)
        .await
        .expect("ready");
    assert_eq!(spawner.count.load(Ordering::SeqCst), 1);
    // A larger window than the running 2048 → respawn.
    pool.base_url(&rec, 4096, ServerMode::Chat)
        .await
        .expect("respawn");
    assert_eq!(spawner.count.load(Ordering::SeqCst), 2);
    // A smaller window is covered by the running 4096 → reuse.
    pool.base_url(&rec, 1024, ServerMode::Chat)
        .await
        .expect("reuse");
    assert_eq!(spawner.count.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn an_embedding_request_launches_an_embedding_server() {
    let spawner = Arc::new(MockSpawner::new(true));
    let pool = fast_pool(Arc::clone(&spawner));

    pool.base_url(&record("e"), 2048, ServerMode::Embedding)
        .await
        .expect("ready");
    assert_eq!(*spawner.modes.lock().unwrap(), [ServerMode::Embedding]);
}

#[tokio::test]
async fn a_server_in_the_other_mode_is_replaced_not_reused() {
    let spawner = Arc::new(MockSpawner::new(true));
    let pool = fast_pool(Arc::clone(&spawner));
    let rec = record("a");

    pool.base_url(&rec, 4096, ServerMode::Chat)
        .await
        .expect("ready");
    pool.base_url(&rec, 4096, ServerMode::Embedding)
        .await
        .expect("respawn");
    assert_eq!(spawner.count.load(Ordering::SeqCst), 2);

    pool.base_url(&rec, 4096, ServerMode::Embedding)
        .await
        .expect("reuse");
    assert_eq!(spawner.count.load(Ordering::SeqCst), 2);
    assert_eq!(
        *spawner.modes.lock().unwrap(),
        [ServerMode::Chat, ServerMode::Embedding]
    );
}

#[tokio::test]
async fn a_decision_request_launches_a_decision_server_and_replaces_a_chat_one() {
    let spawner = Arc::new(MockSpawner::new(true));
    let pool = fast_pool(Arc::clone(&spawner));
    let rec = record("judge");

    pool.base_url(&rec, 4096, ServerMode::Chat)
        .await
        .expect("ready");
    pool.base_url(&rec, 4096, ServerMode::Decision)
        .await
        .expect("respawn");
    pool.base_url(&rec, 4096, ServerMode::Decision)
        .await
        .expect("reuse");
    assert_eq!(
        *spawner.modes.lock().unwrap(),
        [ServerMode::Chat, ServerMode::Decision]
    );
}

/// A record for a loose GGUF in `dir`, quantized Q8_0, beside a BF16 and a
/// Q8_0 projector, claiming `capabilities`.
fn sighted(dir: &std::path::Path, capabilities: Vec<Capability>) -> ModelRecord {
    let weights = dir.join("model-Q8_0.gguf");
    std::fs::write(&weights, b"GGUF").unwrap();
    std::fs::write(dir.join("mmproj-model-BF16.gguf"), projector_gguf(64, 0)).unwrap();
    std::fs::write(dir.join("mmproj-model-Q8_0.gguf"), projector_gguf(64, 0)).unwrap();
    let mut rec = record("sighted");
    rec.source.path = weights.to_str().unwrap().to_owned();
    rec.quantization = Some("Q8_0".to_owned());
    rec.capabilities = capabilities;
    rec
}

#[tokio::test]
async fn a_model_that_sees_is_launched_with_its_projector_in_chat_and_decision_mode() {
    let scratch = TempDir::new();
    let projector = scratch.join("mmproj-model-Q8_0.gguf");
    for (capabilities, mode) in [
        (
            vec![Capability::chat(), Capability::see()],
            ServerMode::Chat,
        ),
        (
            vec![Capability::judge(), Capability::see()],
            ServerMode::Decision,
        ),
    ] {
        let spawner = Arc::new(MockSpawner::new(true));
        let pool = fast_pool(Arc::clone(&spawner));
        pool.base_url(&sighted(scratch.path(), capabilities), 4096, mode)
            .await
            .expect("ready");
        assert_eq!(
            *spawner.projectors.lock().unwrap(),
            [Some(projector.to_str().unwrap().to_owned())],
            "{mode:?}"
        );
    }

    // A record that does not claim sight is launched without one, whatever
    // sits beside it, and so is any embedding server.
    for (capabilities, mode) in [
        (vec![Capability::chat()], ServerMode::Chat),
        (vec![Capability::judge()], ServerMode::Decision),
        (
            vec![Capability::embed(), Capability::see()],
            ServerMode::Embedding,
        ),
    ] {
        let spawner = Arc::new(MockSpawner::new(true));
        let pool = fast_pool(Arc::clone(&spawner));
        pool.base_url(&sighted(scratch.path(), capabilities), 4096, mode)
            .await
            .expect("ready");
        assert_eq!(*spawner.projectors.lock().unwrap(), [None], "{mode:?}");
    }
}

#[tokio::test]
async fn a_server_launched_without_the_projector_wanted_is_replaced_not_reused() {
    let scratch = TempDir::new();
    let spawner = Arc::new(MockSpawner::new(true));
    let pool = fast_pool(Arc::clone(&spawner));
    let blind = sighted(scratch.path(), vec![Capability::chat()]);
    let seeing = sighted(scratch.path(), vec![Capability::chat(), Capability::see()]);

    pool.base_url(&blind, 4096, ServerMode::Chat)
        .await
        .expect("ready");
    pool.base_url(&seeing, 4096, ServerMode::Chat)
        .await
        .expect("respawn");
    pool.base_url(&seeing, 4096, ServerMode::Chat)
        .await
        .expect("reuse");
    assert_eq!(spawner.count.load(Ordering::SeqCst), 2);
}

/// The stand-in `llama-server` for the real spawner: it writes its arguments,
/// one a line, to the file named in place of `ARGS_FILE`, then answers every
/// request on its `--port` with an empty JSON object.
const RECORDING_STAND_IN: &str = r#"#!/bin/sh
printf '%s\n' "$@" > 'ARGS_FILE'
while [ $# -gt 0 ]; do [ "$1" = --port ] && port=$2; shift; done
exec /usr/bin/python3 -I -c '
import http.server, sys
class Answer(http.server.BaseHTTPRequestHandler):
    def answer(self):
        self.send_response(200)
        self.send_header("Content-Length", "2")
        self.end_headers()
        self.wfile.write(b"{}")
    do_GET = answer
    def log_message(self, *args):
        pass
http.server.HTTPServer(("127.0.0.1", int(sys.argv[1])), Answer).serve_forever()
' "$port"
"#;

/// Whether the interpreter the stand-in server runs on works here. On a Mac
/// without the Command Line Tools `/usr/bin/python3` is only a stub that
/// offers to install them, so the stand-in cases are skipped there.
fn python_runs() -> bool {
    let runs = std::process::Command::new("/usr/bin/python3")
        .args(["-I", "-c", ""])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if !runs {
        eprintln!("skipped: /usr/bin/python3 does not run, and the stand-in server needs it");
    }
    runs
}

#[tokio::test]
async fn the_real_spawner_launches_a_decision_server_sized_to_its_window() {
    if !python_runs() {
        return;
    }
    let root = TempDir::new();
    let args_file = root.join("args");
    let server = root.join("llama-server");
    let script = RECORDING_STAND_IN.replace("ARGS_FILE", args_file.to_str().unwrap());
    std::fs::write(&server, script).unwrap();
    std::fs::set_permissions(&server, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();

    for (architecture, decision, one_slot) in [
        ("clef", "clef", true),
        ("qwen35", "openjev", true),
        ("modern-bert", "laya", false),
    ] {
        let weights = root.join(&format!("{decision}.gguf"));
        let header = gguf(&[
            kv_string("general.architecture", architecture),
            kv_string(&format!("{architecture}.decision.type"), decision),
        ]);
        std::fs::write(&weights, header).unwrap();
        let mut rec = record(decision);
        rec.source.path = weights.to_str().unwrap().to_owned();
        rec.capabilities = vec![Capability::judge()];
        let pool = LlamaServerPool::new(Arc::new(LlamaServerSpawner::new(&server)))
            .with_ready_timeout(Duration::from_secs(10));

        pool.base_url(&rec, 16384, ServerMode::Decision)
            .await
            .expect("ready");
        let args: Vec<String> = std::fs::read_to_string(&args_file)
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect();
        let after = |flag: &str| {
            args.iter()
                .position(|arg| arg == flag)
                .and_then(|index| args.get(index + 1))
                .map(String::as_str)
        };
        assert_eq!(after("--model"), weights.to_str(), "{decision}");
        assert_eq!(after("--alias"), Some(decision), "{decision}");
        assert_eq!(after("--ctx-size"), Some("16384"), "{decision}");
        assert_eq!(after("--ubatch-size"), Some("16384"), "{decision}");
        assert_eq!(after("--batch-size"), Some("16384"), "{decision}");
        assert_eq!(after("--parallel"), one_slot.then_some("1"), "{decision}");
        assert!(!args.iter().any(|arg| arg == "--embedding"), "{decision}");
        assert_eq!(after("--mmproj"), None, "{decision}");
    }

    // clef with its projector beside it, as identification left it.
    let projector = root.join("mmproj-clef-Q8_0.gguf");
    std::fs::write(&projector, projector_gguf(64, 0)).unwrap();
    let mut rec = record("clef-sighted");
    rec.source.path = root.join("clef.gguf").to_str().unwrap().to_owned();
    rec.capabilities = vec![Capability::judge(), Capability::see()];
    let pool = LlamaServerPool::new(Arc::new(LlamaServerSpawner::new(&server)))
        .with_ready_timeout(Duration::from_secs(10));
    pool.base_url(&rec, 16384, ServerMode::Decision)
        .await
        .expect("ready");
    let args = std::fs::read_to_string(&args_file).unwrap();
    let args: Vec<&str> = args.lines().collect();
    let mmproj = args.iter().position(|arg| *arg == "--mmproj");
    assert_eq!(
        mmproj.and_then(|index| args.get(index + 1)).copied(),
        projector.to_str()
    );
}

#[tokio::test]
async fn evict_forces_a_respawn() {
    let spawner = Arc::new(MockSpawner::new(true));
    let pool = fast_pool(Arc::clone(&spawner));
    let rec = record("a");

    pool.base_url(&rec, 4096, ServerMode::Chat)
        .await
        .expect("ready");
    assert_eq!(spawner.count.load(Ordering::SeqCst), 1);
    pool.evict(&rec.id).await;
    pool.base_url(&rec, 4096, ServerMode::Chat)
        .await
        .expect("respawn");
    assert_eq!(spawner.count.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn a_process_dead_on_arrival_is_unavailable() {
    struct DeadSpawner;
    impl ServerSpawner for DeadSpawner {
        fn spawn(
            &self,
            _launch: &ServerLaunch<'_>,
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
    let error = pool
        .base_url(&record("a"), 4096, ServerMode::Chat)
        .await
        .unwrap_err();
    assert!(matches!(error, RuntimeError::Unavailable(_)));
}

#[tokio::test]
async fn a_spawn_failure_is_forwarded() {
    struct FailingSpawner;
    impl ServerSpawner for FailingSpawner {
        fn spawn(
            &self,
            _launch: &ServerLaunch<'_>,
        ) -> Result<Box<dyn ServerProcess>, RuntimeError> {
            Err(RuntimeError::Unavailable("no binary".to_owned()))
        }
    }
    let pool = LlamaServerPool::new(Arc::new(FailingSpawner));
    let error = pool
        .base_url(&record("a"), 4096, ServerMode::Chat)
        .await
        .unwrap_err();
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

#[tokio::test]
async fn the_adapter_serves_an_embedding_end_to_end_through_the_pool() {
    let spawner = Arc::new(MockSpawner::new(true));
    let pool = fast_pool(Arc::clone(&spawner));
    let adapter = LlamaServerAdapter::new(Arc::new(pool));

    let mut rec = ModelRecord::new(
        "e",
        Modality::embedding(),
        vec![Capability::embed()],
        ModelSource::new(SourceKind::file(), "/models/e.gguf"),
    );
    rec.runtime.id = Some(RuntimeId::llama_cpp());
    let payload = JsonValue::Object(
        [("input".to_owned(), JsonValue::String("hello".to_owned()))]
            .into_iter()
            .collect(),
    );
    let (chunks, error) = collect(adapter.invoke(&rec, Capability::embed(), payload)).await;
    assert!(error.is_none(), "error: {error:?}");
    assert_eq!(chunks[0], CapabilityChunk::Vector(vec![0.5, 0.25]));
    assert!(matches!(chunks[1], CapabilityChunk::Done(Some(_))));
    assert_eq!(*spawner.modes.lock().unwrap(), [ServerMode::Embedding]);
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
