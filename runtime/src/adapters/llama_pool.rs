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
use kernel::resolution::projector_for;
use tokio::sync::Mutex as AsyncMutex;

use super::RuntimeError;
use super::llama_server::{
    BackendFuture, EmbedderKind, LlamaBackend, ServerMode, keeps_kv_cache, model_gguf_path,
};

/// Why a server that died before it answered `/health` gave nothing.
const STARTUP_EXIT: &str = "llama-server exited during startup";

/// Whether `error` says the server died before it answered `/health`.
pub(crate) fn exited_during_startup(error: &RuntimeError) -> bool {
    matches!(error, RuntimeError::Unavailable(reason) if reason == STARTUP_EXIT)
}

const DEFAULT_READY_TIMEOUT: Duration = Duration::from_secs(30);
const HEALTH_POLL_INTERVAL: Duration = Duration::from_millis(200);
const HEALTH_REQUEST_TIMEOUT: Duration = Duration::from_secs(1);

/// A running server process. Dropping the handle stops the process.
pub trait ServerProcess: Send + Sync {
    /// Whether the process is still running (not yet exited).
    fn is_alive(&self) -> bool;
}

/// One `llama-server` to start.
#[derive(Debug, Clone, Copy)]
pub struct ServerLaunch<'a> {
    /// The GGUF it loads.
    pub gguf_path: &'a str,
    /// The loopback port it binds.
    pub port: u16,
    /// Its context window, in tokens.
    pub context_tokens: i64,
    /// What it serves.
    pub mode: ServerMode,
    /// The multimodal projector it loads, when the model sees.
    pub projector: Option<&'a str>,
    /// The name it gives the model in what it answers, in place of the path
    /// of its weights.
    pub alias: &'a str,
}

/// Spawns a `llama-server`-compatible process bound to a port. Injected so the pool
/// is testable without the real binary.
pub trait ServerSpawner: Send + Sync {
    /// Spawn the server `launch` describes. The production implementation uses
    /// `tokio::process`, so it must be called from within a Tokio runtime (the
    /// pool always calls it from an async task).
    fn spawn(&self, launch: &ServerLaunch<'_>) -> Result<Box<dyn ServerProcess>, RuntimeError>;
}

/// What a `llama-server` launch needs beyond its model, port and window: the
/// mode, with what the model's header says about how to launch it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Launch {
    /// A chat server.
    Chat,
    /// An embedding server for an embedder of this kind.
    Embedding(EmbedderKind),
    /// A decision server.
    Decision {
        /// Whether llama.cpp keeps a KV cache for the model's architecture, as
        /// it does for every decoder (clef, qwen35) and not for `modern-bert`
        /// (laya).
        cached: bool,
    },
}

impl Launch {
    /// The launch for `mode`, reading the header of the GGUF at `gguf_path`
    /// when the mode depends on it.
    fn of(mode: ServerMode, gguf_path: &str) -> Self {
        match mode {
            ServerMode::Chat => Self::Chat,
            ServerMode::Embedding => Self::Embedding(EmbedderKind::of(gguf_path)),
            ServerMode::Decision => Self::Decision {
                cached: keeps_kv_cache(gguf_path),
            },
        }
    }
}

/// The `llama-server` arguments that start `server`, shaped by `launch`.
/// `--pooling` is never passed, so an embedder pools the way its own header
/// says.
fn launch_args(server: &ServerLaunch<'_>, launch: Launch) -> Vec<String> {
    let context = server.context_tokens.to_string();
    let mut args = vec![
        "--model".to_owned(),
        server.gguf_path.to_owned(),
        "--port".to_owned(),
        server.port.to_string(),
        "--host".to_owned(),
        "127.0.0.1".to_owned(),
        "--ctx-size".to_owned(),
        context.clone(),
        // Without it llama-server names the model by the path of its weights
        // in what it answers, which a System One envelope carries verbatim.
        "--alias".to_owned(),
        server.alias.to_owned(),
    ];
    // llama-server loads a projector by itself only one it downloads, never
    // one beside a file it is pointed at.
    if let Some(projector) = server.projector {
        args.extend(["--mmproj".to_owned(), projector.to_owned()]);
    }
    let one_slot = match launch {
        Launch::Chat => return args,
        Launch::Embedding(kind) => {
            args.push("--embedding".to_owned());
            matches!(kind, EmbedderKind::Decoder { .. })
        }
        // llama.cpp reads a laya, kev or clef prompt in one physical batch,
        // and openjev and lev read a question with its options in one logical
        // batch; one batch of the window serves them all. llama.cpp starts the
        // embedding mode the first three score through by itself.
        Launch::Decision { cached } => cached,
    };
    // A non-causal encoder reads a whole input in one physical batch, so a
    // batch smaller than the window fails any longer input.
    args.extend([
        "--ubatch-size".to_owned(),
        context.clone(),
        "--batch-size".to_owned(),
        context,
    ]);
    // A cached model's slots share one KV cache of the window's size: inputs
    // that fit it alone overflow it together, which llama-server answers by
    // crashing. llama.cpp keeps no memory for an encoder such as BERT, so its
    // server keeps llama-server's slots, which serve several short inputs at
    // once.
    if one_slot {
        args.extend(["--parallel".to_owned(), "1".to_owned()]);
    }
    args
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

    /// Turn a spawn failure into an actionable error. A missing binary (the
    /// common case, since `llama-server` is looked up on `PATH`) names what to
    /// install rather than leaking the OS "No such file or directory"; any other
    /// failure keeps its detail, which is genuinely about this spawn.
    fn spawn_error(&self, error: std::io::Error) -> RuntimeError {
        if error.kind() == std::io::ErrorKind::NotFound {
            RuntimeError::Unavailable(format!(
                "{} isn't installed or isn't on your PATH. It comes with llama.cpp (e.g. `brew install llama.cpp`).",
                self.binary.display()
            ))
        } else {
            RuntimeError::Unavailable(format!("could not start llama-server: {error}"))
        }
    }
}

impl ServerSpawner for LlamaServerSpawner {
    fn spawn(&self, launch: &ServerLaunch<'_>) -> Result<Box<dyn ServerProcess>, RuntimeError> {
        let shape = Launch::of(launch.mode, launch.gguf_path);
        let child = tokio::process::Command::new(&self.binary)
            .args(launch_args(launch, shape))
            .args(&self.extra_args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            // Its own group, so a Ctrl-C typed at hedos (or a foreground
            // command hedos runs) never reaches the server.
            .process_group(0)
            // The OS reaps the server when the handle drops (pool teardown / a
            // failed readiness wait).
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| self.spawn_error(error))?;
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
    /// What the server was launched to do; a request for the other mode
    /// replaces it.
    mode: ServerMode,
    /// Whether it was launched to read images; a request that wants the
    /// other replaces it.
    sees: bool,
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
    fn base_url(
        &self,
        record: &ModelRecord,
        context_tokens: i64,
        mode: ServerMode,
    ) -> BackendFuture {
        let spawner = Arc::clone(&self.spawner);
        let client = self.client.clone();
        let slots = Arc::clone(&self.slots);
        let ready_timeout = self.ready_timeout;
        // Clone only what `ensure` needs, not the whole record, but for a
        // model that sees, whose projector is looked for when a server starts.
        let wanted = Wanted {
            model_id: record.id.clone(),
            gguf: model_gguf_path(record).to_owned(),
            // llama-server splits an alias at commas into several names.
            alias: record.display_name().replace(',', " ").trim().to_owned(),
            context_tokens,
            mode,
            sight: mode.reads_images(record).then(|| record.clone()),
        };
        Box::pin(async move { ensure(&spawner, &client, &slots, ready_timeout, &wanted).await })
    }
}

/// The server a request needs: whose weights, how large a window, which mode,
/// and, for a server that reads images, the record its projector is found for.
struct Wanted {
    model_id: String,
    gguf: String,
    alias: String,
    context_tokens: i64,
    mode: ServerMode,
    sight: Option<ModelRecord>,
}

/// Ensure a ready server exists for the `wanted` model in its mode, returning
/// its base URL. Reuses a live server in that mode whose context window covers
/// the one wanted and still answers `/health`; otherwise allocates a port,
/// spawns, and waits for readiness (a too-small, other-mode, other-sight, or
/// wedged server is replaced, and dropping the old `Running` reaps it). The
/// projector is looked for only then, so a request to a running server reads
/// no folder.
async fn ensure(
    spawner: &Arc<dyn ServerSpawner>,
    client: &reqwest::Client,
    slots: &StdMutex<HashMap<String, Arc<Slot>>>,
    ready_timeout: Duration,
    wanted: &Wanted,
) -> Result<String, RuntimeError> {
    let Wanted {
        model_id,
        gguf,
        alias,
        context_tokens,
        mode,
        sight,
    } = wanted;
    let (context_tokens, mode) = (*context_tokens, *mode);
    // Brief lock to get-or-create this model's slot; not held across the spawn.
    let slot = {
        let mut slots = slots
            .lock()
            .map_err(|_| RuntimeError::Failed("llama pool lock poisoned".to_owned()))?;
        Arc::clone(slots.entry(model_id.clone()).or_default())
    };
    let mut guard = slot.server.lock().await;
    if let Some(running) = guard.as_ref()
        && running.process.is_alive()
        && running.mode == mode
        && running.sees == sight.is_some()
        && running.context_tokens >= context_tokens
        && healthy_for_reuse(client, &running.base_url).await
    {
        return Ok(running.base_url.clone());
    }

    let port = free_port()?;
    let projector = sight
        .as_ref()
        .and_then(projector_for)
        .and_then(|path| path.to_str().map(str::to_owned));
    let process = spawner.spawn(&ServerLaunch {
        gguf_path: gguf,
        port,
        context_tokens,
        mode,
        projector: projector.as_deref(),
        alias,
    })?;
    let base_url = format!("http://127.0.0.1:{port}");
    // If readiness fails, `process` drops here → the OS reaps it (kill_on_drop).
    wait_ready(client, &base_url, process.as_ref(), ready_timeout).await?;
    *guard = Some(Running {
        base_url: base_url.clone(),
        context_tokens,
        mode,
        sees: sight.is_some(),
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

/// A reuse-time health gate tolerant of a single transient stall: a warm server
/// is condemned only after failing `/health` twice in a row. A server that is
/// merely busy (its HTTP layer still answers) passes the first probe in
/// milliseconds; requiring two consecutive misses keeps a one-off blip under load
/// from evicting — and SIGKILL-ing — a server that is actively serving a request.
async fn healthy_for_reuse(client: &reqwest::Client, base_url: &str) -> bool {
    health_ok(client, base_url).await || health_ok(client, base_url).await
}

/// A single `/health` probe: `true` only if the server answers success promptly.
async fn health_ok(client: &reqwest::Client, base_url: &str) -> bool {
    client
        .get(format!("{base_url}/health"))
        .timeout(HEALTH_REQUEST_TIMEOUT)
        .send()
        .await
        .map(|response| response.status().is_success())
        .unwrap_or(false)
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
            return Err(RuntimeError::Unavailable(STARTUP_EXIT.to_owned()));
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A chat server for `gguf_path` on port 8080, named `Llama 3`.
    fn launch(
        gguf_path: &'static str,
        context_tokens: i64,
        projector: Option<&'static str>,
    ) -> ServerLaunch<'static> {
        ServerLaunch {
            gguf_path,
            port: 8080,
            context_tokens,
            mode: ServerMode::Chat,
            projector,
            alias: "Llama 3",
        }
    }

    #[test]
    fn a_missing_binary_names_what_to_install_instead_of_the_errno() {
        // A bare command name that isn't on PATH: the OS returns ENOENT, which
        // must not reach the user verbatim.
        let spawner = LlamaServerSpawner::new("llama-server-definitely-not-installed");
        match spawner.spawn(&launch("/tmp/model.gguf", 2048, None)) {
            Err(RuntimeError::Unavailable(message)) => {
                assert!(message.contains("isn't installed"), "message: {message}");
                assert!(message.contains("PATH"), "message: {message}");
                assert!(!message.contains("os error"), "leaked the errno: {message}");
            }
            Err(other) => panic!("expected Unavailable, got {other:?}"),
            Ok(_) => panic!("spawning a missing binary must fail"),
        }
    }

    #[test]
    fn a_chat_server_loads_its_model_under_the_models_name() {
        assert_eq!(
            launch_args(&launch("/m.gguf", 4096, None), Launch::Chat),
            [
                "--model",
                "/m.gguf",
                "--port",
                "8080",
                "--host",
                "127.0.0.1",
                "--ctx-size",
                "4096",
                "--alias",
                "Llama 3",
            ]
        );
    }

    #[test]
    fn embedding_launch_args_size_the_batch_to_the_window_with_one_slot_for_a_decoder() {
        let decoders = [
            EmbedderKind::Decoder { pools_last: true },
            EmbedderKind::Decoder { pools_last: false },
        ];
        for kind in decoders.into_iter().chain([EmbedderKind::Encoder]) {
            let args = launch_args(&launch("/e.gguf", 2048, None), Launch::Embedding(kind));
            assert!(args.contains(&"--embedding".to_owned()));
            let after = |flag: &str| {
                args.iter()
                    .position(|arg| arg == flag)
                    .and_then(|index| args.get(index + 1))
                    .map(String::as_str)
            };
            assert_eq!(after("--ubatch-size"), Some("2048"), "{kind:?}");
            assert_eq!(after("--batch-size"), Some("2048"), "{kind:?}");
            assert_eq!(after("--ctx-size"), Some("2048"), "{kind:?}");
            let one_slot = (kind != EmbedderKind::Encoder).then_some("1");
            assert_eq!(after("--parallel"), one_slot, "{kind:?}");
            assert!(!args.iter().any(|arg| arg == "--pooling"));
        }
    }

    #[test]
    fn only_an_embedder_llama_cpp_keeps_no_memory_for_keeps_its_slots() {
        let parallel = |architecture: &str| {
            let kind = EmbedderKind::from_header(Some(architecture), None);
            let args = launch_args(&launch("/e.gguf", 2048, None), Launch::Embedding(kind));
            args.iter()
                .position(|arg| arg == "--parallel")
                .and_then(|index| args.get(index + 1))
                .cloned()
        };
        for cached in ["llama-embed", "t5encoder", "qwen3"] {
            assert_eq!(parallel(cached).as_deref(), Some("1"), "{cached}");
        }
        for memoryless in ["bert", "nomic-bert", "gemma-embedding"] {
            assert_eq!(parallel(memoryless), None, "{memoryless}");
        }
    }

    fn after<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
        args.iter()
            .position(|arg| arg == flag)
            .and_then(|index| args.get(index + 1))
            .map(String::as_str)
    }

    #[test]
    fn a_projector_is_loaded_in_chat_and_decision_servers() {
        for shape in [Launch::Chat, Launch::Decision { cached: true }] {
            let args = launch_args(&launch("/m.gguf", 4096, Some("/mmproj-Q8_0.gguf")), shape);
            assert_eq!(
                after(&args, "--mmproj"),
                Some("/mmproj-Q8_0.gguf"),
                "{shape:?}"
            );
            let args = launch_args(&launch("/m.gguf", 4096, None), shape);
            assert!(!args.iter().any(|arg| arg == "--mmproj"), "{shape:?}");
        }
    }

    #[test]
    fn decision_launch_args_hold_the_whole_prompt_in_one_batch_without_embedding_mode() {
        for cached in [true, false] {
            let args = launch_args(&launch("/d.gguf", 16384, None), Launch::Decision { cached });
            assert_eq!(after(&args, "--model"), Some("/d.gguf"));
            assert_eq!(after(&args, "--ctx-size"), Some("16384"), "{cached}");
            assert_eq!(after(&args, "--ubatch-size"), Some("16384"), "{cached}");
            assert_eq!(after(&args, "--batch-size"), Some("16384"), "{cached}");
            assert_eq!(
                after(&args, "--parallel"),
                cached.then_some("1"),
                "{cached}"
            );
            // llama.cpp turns embedding mode on itself for the types that score
            // through it; passing it would also refuse openjev and lev.
            assert!(!args.iter().any(|arg| arg == "--embedding"), "{cached}");
            assert!(!args.iter().any(|arg| arg == "--pooling"), "{cached}");
        }
    }

    #[test]
    fn a_decision_server_whose_header_cannot_be_read_launches_with_one_slot() {
        assert_eq!(
            Launch::of(ServerMode::Decision, "/nonexistent.gguf"),
            Launch::Decision { cached: true }
        );
        assert_eq!(
            Launch::of(ServerMode::Chat, "/nonexistent.gguf"),
            Launch::Chat
        );
    }

    #[test]
    fn a_non_missing_spawn_error_keeps_its_detail() {
        // A permissions failure is about this spawn, not a missing install, so
        // its detail is kept rather than replaced with the install hint.
        let spawner = LlamaServerSpawner::new("llama-server");
        let error = spawner.spawn_error(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "permission denied",
        ));
        match error {
            RuntimeError::Unavailable(message) => {
                assert!(
                    message.contains("could not start llama-server"),
                    "message: {message}"
                );
                assert!(!message.contains("isn't installed"), "message: {message}");
            }
            other => panic!("expected Unavailable, got {other:?}"),
        }
    }
}
