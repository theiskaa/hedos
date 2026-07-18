//! The `LlamaServerPool`: the concrete [`LlamaBackend`] that spawns, health-checks,
//! reuses, and tears down `llama-server` subprocesses. One server runs per model,
//! reused across requests; a cold server is polled on `/health` until ready.
//!
//! The process spawning is behind a [`ServerSpawner`] seam so the pool's caching,
//! readiness, and lifecycle logic can be tested without a real llama.cpp binary.
//!
//! Deferred refinements (documented, not yet needed for v1): teardown is
//! `kill_on_drop` SIGKILL rather than a graceful `process::terminate_tree`;
//! [`LlamaServerPool::evict`] is the escape hatch for a hung-but-alive server (the
//! pool can't detect one, only a crash); slot-map entries are not evicted (bounded
//! by the number of distinct model ids); and `free_port` is a bind-then-drop that
//! can, rarely, race a concurrent cold-start for a different model (which then
//! surfaces as a clean `Unavailable`, no retry).

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use kernel::records::ModelRecord;
use tokio::sync::Mutex as AsyncMutex;

use super::RuntimeError;
use super::llama_server::{BackendFuture, LlamaBackend, model_gguf_path};

const DEFAULT_READY_TIMEOUT: Duration = Duration::from_secs(30);
const HEALTH_POLL_INTERVAL: Duration = Duration::from_millis(200);
const HEALTH_REQUEST_TIMEOUT: Duration = Duration::from_secs(1);

/// A running server process. Dropping the handle stops the process.
pub trait ServerProcess: Send + Sync {
    /// Whether the process is still running (not yet exited).
    fn is_alive(&self) -> bool;
}

/// Spawns a `llama-server`-compatible process bound to a port. Injected so the pool
/// is testable without the real binary.
pub trait ServerSpawner: Send + Sync {
    /// Spawn a server for `gguf_path`, bound to `port`, sized to `context_tokens`.
    /// The production implementation uses `tokio::process`, so it must be called
    /// from within a Tokio runtime (the pool always calls it from an async task).
    fn spawn(
        &self,
        gguf_path: &str,
        port: u16,
        context_tokens: i64,
    ) -> Result<Box<dyn ServerProcess>, RuntimeError>;
}

/// The production spawner: runs the `llama-server` binary.
pub struct LlamaServerSpawner {
    binary: PathBuf,
    extra_args: Vec<String>,
}

impl LlamaServerSpawner {
    /// A spawner running `binary` (a path to `llama-server`).
    pub fn new(binary: impl Into<PathBuf>) -> Self {
        Self {
            binary: binary.into(),
            extra_args: Vec::new(),
        }
    }

    /// Extra flags appended to every launch (e.g. `--n-gpu-layers 999`).
    pub fn with_extra_args(mut self, args: Vec<String>) -> Self {
        self.extra_args = args;
        self
    }
}

impl ServerSpawner for LlamaServerSpawner {
    fn spawn(
        &self,
        gguf_path: &str,
        port: u16,
        context_tokens: i64,
    ) -> Result<Box<dyn ServerProcess>, RuntimeError> {
        let child = tokio::process::Command::new(&self.binary)
            .arg("--model")
            .arg(gguf_path)
            .arg("--port")
            .arg(port.to_string())
            .arg("--host")
            .arg("127.0.0.1")
            .arg("--ctx-size")
            .arg(context_tokens.to_string())
            .args(&self.extra_args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            // The OS reaps the server when the handle drops (pool teardown / a
            // failed readiness wait).
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| {
                RuntimeError::Unavailable(format!("could not start llama-server: {error}"))
            })?;
        Ok(Box::new(ChildProcess {
            child: StdMutex::new(child),
        }))
    }
}

struct ChildProcess {
    child: StdMutex<tokio::process::Child>,
}

impl ServerProcess for ChildProcess {
    fn is_alive(&self) -> bool {
        // `try_wait` reaps without blocking: `Ok(None)` means still running.
        self.child
            .lock()
            .map(|mut child| matches!(child.try_wait(), Ok(None)))
            .unwrap_or(false)
    }
}

/// A per-model slot holding that model's running server, if any. The async mutex
/// serializes spawns for one model (so concurrent requests don't double-launch)
/// without blocking other models.
#[derive(Default)]
struct Slot {
    server: AsyncMutex<Option<Running>>,
}

struct Running {
    base_url: String,
    /// The `--ctx-size` this server was launched with. A request needing a larger
    /// window respawns rather than reusing a too-small server.
    context_tokens: i64,
    process: Box<dyn ServerProcess>,
}

/// A pool of `llama-server` instances, one per model, reused across requests.
pub struct LlamaServerPool {
    spawner: Arc<dyn ServerSpawner>,
    client: reqwest::Client,
    slots: Arc<StdMutex<HashMap<String, Arc<Slot>>>>,
    ready_timeout: Duration,
}

impl LlamaServerPool {
    /// A pool that launches servers through `spawner`.
    pub fn new(spawner: Arc<dyn ServerSpawner>) -> Self {
        Self {
            spawner,
            client: reqwest::Client::new(),
            slots: Arc::new(StdMutex::new(HashMap::new())),
            ready_timeout: DEFAULT_READY_TIMEOUT,
        }
    }

    /// Override how long to wait for a cold server to answer `/health`.
    pub fn with_ready_timeout(mut self, timeout: Duration) -> Self {
        self.ready_timeout = timeout;
        self
    }

    /// Drop any running server for `model_id`, so the next request respawns. Call
    /// this when a server is observed wedged: the pool gates reuse on process
    /// liveness (`try_wait`), which can't detect a hung-but-alive server, so a
    /// caller that sees requests failing at the HTTP layer evicts it here.
    pub async fn evict(&self, model_id: &str) {
        let slot = self
            .slots
            .lock()
            .ok()
            .and_then(|slots| slots.get(model_id).cloned());
        if let Some(slot) = slot {
            *slot.server.lock().await = None;
        }
    }
}

impl LlamaBackend for LlamaServerPool {
    fn base_url(&self, record: &ModelRecord, context_tokens: i64) -> BackendFuture {
        let spawner = Arc::clone(&self.spawner);
        let client = self.client.clone();
        let slots = Arc::clone(&self.slots);
        let ready_timeout = self.ready_timeout;
        // Clone only what `ensure` needs, not the whole record.
        let model_id = record.id.clone();
        let gguf = model_gguf_path(record).to_owned();
        Box::pin(async move {
            ensure(
                &spawner,
                &client,
                &slots,
                ready_timeout,
                &model_id,
                &gguf,
                context_tokens,
            )
            .await
        })
    }
}

/// Ensure a ready server exists for `model_id` (weights at `gguf`), returning its
/// base URL. Reuses a live server whose context window covers `context_tokens`;
/// otherwise allocates a port, spawns, and waits for readiness (a too-small server
/// is replaced — dropping the old `Running` reaps it).
async fn ensure(
    spawner: &Arc<dyn ServerSpawner>,
    client: &reqwest::Client,
    slots: &StdMutex<HashMap<String, Arc<Slot>>>,
    ready_timeout: Duration,
    model_id: &str,
    gguf: &str,
    context_tokens: i64,
) -> Result<String, RuntimeError> {
    // Brief lock to get-or-create this model's slot; not held across the spawn.
    let slot = {
        let mut slots = slots
            .lock()
            .map_err(|_| RuntimeError::Failed("llama pool lock poisoned".to_owned()))?;
        Arc::clone(slots.entry(model_id.to_owned()).or_default())
    };
    let mut guard = slot.server.lock().await;
    if let Some(running) = guard.as_ref()
        && running.process.is_alive()
        && running.context_tokens >= context_tokens
    {
        return Ok(running.base_url.clone());
    }

    let port = free_port()?;
    let process = spawner.spawn(gguf, port, context_tokens)?;
    let base_url = format!("http://127.0.0.1:{port}");
    // If readiness fails, `process` drops here → the OS reaps it (kill_on_drop).
    wait_ready(client, &base_url, process.as_ref(), ready_timeout).await?;
    *guard = Some(Running {
        base_url: base_url.clone(),
        context_tokens,
        process,
    });
    Ok(base_url)
}

/// An ephemeral free TCP port on the loopback. The listener is dropped so the
/// server can bind it (a small TOCTOU window inherent to picking a port up front).
fn free_port() -> Result<u16, RuntimeError> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .map_err(|error| RuntimeError::Unavailable(format!("no free port: {error}")))?;
    listener
        .local_addr()
        .map(|addr| addr.port())
        .map_err(|error| RuntimeError::Unavailable(format!("no free port: {error}")))
}

/// Poll `{base}/health` until it answers success, the process exits, or the timeout
/// elapses.
async fn wait_ready(
    client: &reqwest::Client,
    base_url: &str,
    process: &dyn ServerProcess,
    timeout: Duration,
) -> Result<(), RuntimeError> {
    let url = format!("{base_url}/health");
    let start = Instant::now();
    loop {
        if !process.is_alive() {
            return Err(RuntimeError::Unavailable(
                "llama-server exited during startup".to_owned(),
            ));
        }
        if let Ok(response) = client
            .get(&url)
            .timeout(HEALTH_REQUEST_TIMEOUT)
            .send()
            .await
            && response.status().is_success()
        {
            return Ok(());
        }
        if start.elapsed() >= timeout {
            return Err(RuntimeError::Unavailable(
                "llama-server did not become ready in time".to_owned(),
            ));
        }
        tokio::time::sleep(HEALTH_POLL_INTERVAL).await;
    }
}
