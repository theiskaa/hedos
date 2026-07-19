//! Liveness tracking for the two external image daemons Hedos can serve through
//! (ComfyUI and AUTOMATIC1111). It probes each daemon's HTTP API for reachability
//! and served checkpoints, and matches a [`ModelRecord`] against those checkpoints
//! so an adapter only bids when its daemon can actually run the model.
//!
//! Cheap to clone (`Arc` handle). The snapshot lock is a `std::sync::Mutex` held
//! only for synchronous reads/writes and never across an `.await`.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use kernel::jobs::JobRuntimeEvent;
use kernel::records::{JsonValue, ModelRecord};
use serde_json::Value;

use super::{JobStream, RuntimeError, RuntimeStream};
use crate::governor::BoxFuture;

/// The per-request timeout for a liveness probe. Short: a probe that hangs means
/// the daemon is as good as dead for this pass.
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// The two daemons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Daemon {
    /// ComfyUI (default `http://127.0.0.1:8188`).
    ComfyUi,
    /// AUTOMATIC1111 (default `http://127.0.0.1:7860`).
    A1111,
}

/// One daemon's last-probed state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DaemonState {
    /// Whether the daemon answered its liveness endpoint.
    pub alive: bool,
    /// The checkpoint names the daemon reported serving.
    pub models: Vec<String>,
}

/// Both daemons' states from one probe pass.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    /// ComfyUI's state.
    pub comfy_ui: DaemonState,
    /// AUTOMATIC1111's state.
    pub a1111: DaemonState,
}

struct Live {
    snapshot: Snapshot,
    /// Bumped on every [`DaemonLiveness::mark_dead`]; a `probe` only stores its
    /// result if the epoch is unchanged, so a mark-dead that races a probe wins.
    epoch: u64,
}

struct Inner {
    live: Mutex<Live>,
    client: reqwest::Client,
    comfy_url: String,
    a1111_url: String,
}

/// Tracks whether the ComfyUI and AUTOMATIC1111 daemons are up and what they serve.
#[derive(Clone)]
pub struct DaemonLiveness {
    inner: Arc<Inner>,
}

impl DaemonLiveness {
    /// A tracker probing the daemons at the given base URLs.
    pub fn new(comfy_url: impl Into<String>, a1111_url: impl Into<String>) -> Self {
        Self {
            inner: Arc::new(Inner {
                live: Mutex::new(Live {
                    snapshot: Snapshot::default(),
                    epoch: 0,
                }),
                client: reqwest::Client::new(),
                comfy_url: comfy_url.into(),
                a1111_url: a1111_url.into(),
            }),
        }
    }

    /// A tracker probing the daemons at their conventional local ports.
    pub fn with_defaults() -> Self {
        Self::new("http://127.0.0.1:8188", "http://127.0.0.1:7860")
    }

    /// ComfyUI's base URL.
    pub fn comfy_url(&self) -> &str {
        &self.inner.comfy_url
    }

    /// AUTOMATIC1111's base URL.
    pub fn a1111_url(&self) -> &str {
        &self.inner.a1111_url
    }

    /// The last probed snapshot.
    pub fn current(&self) -> Snapshot {
        self.with_live(|live| live.snapshot.clone())
    }

    /// Mark `daemon` dead now, clearing its state and invalidating any probe pass
    /// still in flight.
    pub fn mark_dead(&self, daemon: Daemon) {
        self.with_live(|live| {
            live.epoch = live.epoch.wrapping_add(1);
            match daemon {
                Daemon::ComfyUi => live.comfy_ui_reset(),
                Daemon::A1111 => live.a1111_reset(),
            }
        });
    }

    /// Probe both daemons concurrently and store the fresh snapshot, unless a
    /// [`mark_dead`](Self::mark_dead) bumped the epoch while the probe ran.
    pub async fn probe(&self) {
        let start_epoch = self.with_live(|live| live.epoch);
        let (comfy_ui, a1111) = tokio::join!(self.probe_comfy_ui(), self.probe_a1111());
        let fresh = Snapshot { comfy_ui, a1111 };
        self.with_live(|live| {
            if live.epoch == start_epoch {
                live.snapshot = fresh;
            }
        });
    }

    async fn probe_comfy_ui(&self) -> DaemonState {
        let base = &self.inner.comfy_url;
        if !self.reachable(&format!("{base}/system_stats")).await {
            return DaemonState::default();
        }
        let models = match self.fetch_json(&format!("{base}/object_info")).await {
            Some(json) => comfy_checkpoints(&json),
            None => Vec::new(),
        };
        DaemonState {
            alive: true,
            models,
        }
    }

    async fn probe_a1111(&self) -> DaemonState {
        let base = &self.inner.a1111_url;
        match self.fetch_json(&format!("{base}/sdapi/v1/sd-models")).await {
            Some(json) => DaemonState {
                alive: true,
                models: a1111_checkpoints(&json),
            },
            None => DaemonState::default(),
        }
    }

    async fn reachable(&self, url: &str) -> bool {
        matches!(
            self.inner
                .client
                .get(url)
                .timeout(PROBE_TIMEOUT)
                .send()
                .await,
            Ok(response) if response.status() == reqwest::StatusCode::OK
        )
    }

    async fn fetch_json(&self, url: &str) -> Option<Value> {
        let response = self
            .inner
            .client
            .get(url)
            .timeout(PROBE_TIMEOUT)
            .send()
            .await
            .ok()?;
        if response.status() != reqwest::StatusCode::OK {
            return None;
        }
        response.json::<Value>().await.ok()
    }

    fn with_live<T>(&self, f: impl FnOnce(&mut Live) -> T) -> T {
        // The mutex is only poisoned if a holder panicked mid-update; recover the
        // guard rather than propagating, since the state is a plain cache.
        let mut live = self
            .inner
            .live
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        f(&mut live)
    }

    /// The checkpoint names `record` could be served under by a daemon serving
    /// `served_models`.
    pub(crate) fn matching_models(record: &ModelRecord, served_models: &[String]) -> Vec<String> {
        let mut candidates: Vec<String> = vec![normalized(&record.name)];
        if let Some(repo) = &record.source.repo {
            candidates.push(normalized(repo));
        }
        candidates.push(normalized(&record.source.path));
        served_models
            .iter()
            .filter(|model| candidates.contains(&normalized(model)))
            .cloned()
            .collect()
    }

    /// Whether `record` matches any of the daemon's `served_models`.
    pub(crate) fn matches(record: &ModelRecord, served_models: &[String]) -> bool {
        !Self::matching_models(record, served_models).is_empty()
    }

    /// Overwrite the snapshot directly, for tests that need a daemon to look alive
    /// without a running server.
    #[cfg(test)]
    pub(crate) fn seed(&self, snapshot: Snapshot) {
        self.with_live(|live| live.snapshot = snapshot);
    }
}

/// Why a daemon generation failed. A `Transport` failure (the daemon did not
/// answer) additionally marks the daemon dead; a `Failed` is a logical error (bad
/// response, no image) that leaves the liveness state alone.
pub(crate) enum DaemonError {
    /// The daemon was unreachable — mark it dead.
    Transport(String),
    /// The daemon answered but the exchange failed.
    Failed(String),
}

/// Classify a `reqwest` error along the transport/logical boundary: a body that
/// failed to decode is a logical [`DaemonError::Failed`] (the daemon answered with
/// something unparseable), while any other failure — connect, timeout, or a
/// network drop mid-body — is a [`DaemonError::Transport`] that marks the daemon
/// dead.
/// reqwest surfaces a mid-body drop at `.json()`/`.bytes()`, not at `send()`, so
/// this must key on `is_decode()` rather than on which call failed.
pub(crate) fn daemon_error(error: reqwest::Error) -> DaemonError {
    if error.is_decode() {
        DaemonError::Failed(error.to_string())
    } else {
        DaemonError::Transport(error.to_string())
    }
}

/// Drive one daemon image job: emit `Started`, run `generate` (racing the
/// consumer dropping the stream, which cancels it), then emit the `Result` PNG or
/// a failure — marking `daemon` dead on a transport error. Shared by the ComfyUI
/// and AUTOMATIC1111 adapters.
pub(crate) fn run_daemon_job(
    daemon: Daemon,
    liveness: DaemonLiveness,
    generate: BoxFuture<Result<Vec<u8>, DaemonError>>,
) -> JobStream {
    let (tx, stream) = RuntimeStream::channel();
    tokio::spawn(async move {
        if tx.send(Ok(JobRuntimeEvent::Started)).is_err() {
            return;
        }
        let outcome = tokio::select! {
            _ = tx.closed() => return,
            outcome = generate => outcome,
        };
        let event = match outcome {
            Ok(data) => Ok(JobRuntimeEvent::Result {
                data,
                file_extension: "png".to_owned(),
            }),
            Err(DaemonError::Transport(message)) => {
                liveness.mark_dead(daemon);
                Err(RuntimeError::Failed(message))
            }
            Err(DaemonError::Failed(message)) => Err(RuntimeError::Failed(message)),
        };
        let _ = tx.send(event);
    });
    stream
}

impl Live {
    fn comfy_ui_reset(&mut self) {
        self.snapshot.comfy_ui = DaemonState::default();
    }

    fn a1111_reset(&mut self) {
        self.snapshot.a1111 = DaemonState::default();
    }
}

/// The checkpoint names ComfyUI's `/object_info` advertises for
/// `CheckpointLoaderSimple`'s `ckpt_name` input.
pub(crate) fn comfy_checkpoints(object_info: &Value) -> Vec<String> {
    let names = object_info
        .get("CheckpointLoaderSimple")
        .and_then(|loader| loader.get("input"))
        .and_then(|input| input.get("required"))
        .and_then(|required| required.get("ckpt_name"))
        .and_then(Value::as_array)
        .and_then(|ckpt| ckpt.first())
        .and_then(Value::as_array);
    match names {
        Some(names) => names
            .iter()
            .filter_map(|value| value.as_str().map(str::to_owned))
            .collect(),
        None => Vec::new(),
    }
}

/// The checkpoint names AUTOMATIC1111's `/sdapi/v1/sd-models` lists, preferring
/// `model_name` and falling back to `title`.
pub(crate) fn a1111_checkpoints(models: &Value) -> Vec<String> {
    let Some(entries) = models.as_array() else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|entry| {
            entry
                .get("model_name")
                .or_else(|| entry.get("title"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect()
}

/// A checkpoint name reduced to its comparison key: the last path component,
/// lowercased, with the `.safetensors`/`.ckpt` extension removed.
pub(crate) fn normalized(name: &str) -> String {
    last_component(name)
        .to_lowercase()
        .replace(".safetensors", "")
        .replace(".ckpt", "")
}

/// The last `/`-separated component of `path` (trailing slashes ignored), matching
/// `NSString.lastPathComponent` for the shapes daemon model names take.
pub(crate) fn last_component(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rsplit_once('/') {
        Some((_, last)) => last,
        None => trimmed,
    }
}

/// The `(width, height)` a payload requests: explicit `width`/`height` integers,
/// else a `"WxH"` `size` string, else `(fallback, fallback)`.
pub(crate) fn dimensions(object: &BTreeMap<String, JsonValue>, fallback: i64) -> (i64, i64) {
    if let (Some(width), Some(height)) = (
        object.get("width").and_then(JsonValue::as_i64),
        object.get("height").and_then(JsonValue::as_i64),
    ) {
        return (width, height);
    }
    if let Some(size) = object.get("size").and_then(JsonValue::as_str) {
        let lowered = size.to_lowercase();
        // Empty subsequences are dropped, so `"640xx480"` still parses as two
        // components.
        let parts: Vec<&str> = lowered.split('x').filter(|part| !part.is_empty()).collect();
        if let [width, height] = parts.as_slice()
            && let (Ok(width), Ok(height)) = (width.parse::<i64>(), height.parse::<i64>())
        {
            return (width, height);
        }
    }
    (fallback, fallback)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel::records::{Modality, ModelSource, SourceKind};
    use serde_json::json;

    fn record_named(name: &str, repo: Option<&str>, path: &str) -> ModelRecord {
        let mut source = ModelSource::new(SourceKind::huggingface_cache(), path);
        source.repo = repo.map(str::to_owned);
        ModelRecord::new(name, Modality::image(), Vec::new(), source)
    }

    #[test]
    fn comfy_checkpoints_digs_out_the_ckpt_name_list() {
        let object_info = json!({
            "CheckpointLoaderSimple": {
                "input": { "required": { "ckpt_name": [["a.safetensors", "b.ckpt"]] } }
            }
        });
        assert_eq!(
            comfy_checkpoints(&object_info),
            vec!["a.safetensors", "b.ckpt"]
        );
        assert!(comfy_checkpoints(&json!({})).is_empty());
    }

    #[test]
    fn a1111_checkpoints_prefers_model_name_then_title() {
        let models = json!([
            { "model_name": "sd_xl", "title": "SD XL [abc]" },
            { "title": "only-title" },
            { "hash": "nope" },
        ]);
        assert_eq!(a1111_checkpoints(&models), vec!["sd_xl", "only-title"]);
    }

    #[test]
    fn normalized_strips_the_path_extension_and_case() {
        assert_eq!(normalized("Models/Foo.safetensors"), "foo");
        assert_eq!(normalized("bar.ckpt"), "bar");
        assert_eq!(normalized("Baz"), "baz");
    }

    #[test]
    fn matching_models_matches_name_repo_or_path_tail() {
        let record = record_named(
            "MyModel",
            Some("org/my-model"),
            "/weights/MyModel.safetensors",
        );
        // Matches by normalized name.
        assert!(DaemonLiveness::matches(
            &record,
            &["MyModel.safetensors".to_owned()]
        ));
        // Matches by repo tail.
        assert!(DaemonLiveness::matches(
            &record,
            &["my-model.ckpt".to_owned()]
        ));
        // No match.
        assert!(!DaemonLiveness::matches(&record, &["other".to_owned()]));
    }

    #[test]
    fn dimensions_reads_explicit_then_size_string_then_fallback() {
        let mut explicit = BTreeMap::new();
        explicit.insert("width".to_owned(), JsonValue::Int(768));
        explicit.insert("height".to_owned(), JsonValue::Int(1024));
        assert_eq!(dimensions(&explicit, 512), (768, 1024));

        let mut sized = BTreeMap::new();
        sized.insert("size".to_owned(), JsonValue::String("640X480".to_owned()));
        assert_eq!(dimensions(&sized, 512), (640, 480));

        assert_eq!(dimensions(&BTreeMap::new(), 512), (512, 512));
    }

    #[test]
    fn dimensions_ignores_empty_size_segments_like_swift() {
        let mut sized = BTreeMap::new();
        sized.insert("size".to_owned(), JsonValue::String("640xx480".to_owned()));
        assert_eq!(dimensions(&sized, 512), (640, 480));
    }

    async fn serve_json(delay: Duration, body: &'static str) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut buffer = [0u8; 1024];
                    let _ = socket.read(&mut buffer).await;
                    if !delay.is_zero() {
                        tokio::time::sleep(delay).await;
                    }
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn probe_marks_the_daemon_alive_and_reads_its_checkpoints() {
        let a1111 = serve_json(Duration::ZERO, r#"[{"model_name":"foo"}]"#).await;
        let liveness = DaemonLiveness::new("http://127.0.0.1:1", a1111);
        liveness.probe().await;
        let snapshot = liveness.current();
        assert!(snapshot.a1111.alive);
        assert_eq!(snapshot.a1111.models, vec!["foo".to_owned()]);
        // ComfyUI's dead port stays not-alive.
        assert!(!snapshot.comfy_ui.alive);
    }

    #[tokio::test]
    async fn a_mark_dead_during_a_probe_is_not_clobbered_by_the_stale_result() {
        // The A1111 stub answers slowly; a mark_dead lands mid-probe, bumping the
        // epoch. The probe's (alive) result must be discarded by the epoch guard.
        let a1111 = serve_json(Duration::from_millis(300), r#"[{"model_name":"foo"}]"#).await;
        let liveness = DaemonLiveness::new("http://127.0.0.1:1", a1111);
        let probing = {
            let liveness = liveness.clone();
            tokio::spawn(async move { liveness.probe().await })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        liveness.mark_dead(Daemon::A1111);
        probing.await.unwrap();
        assert!(!liveness.current().a1111.alive);
    }

    #[test]
    fn mark_dead_clears_state_and_bumps_the_epoch() {
        let liveness = DaemonLiveness::with_defaults();
        liveness.with_live(|live| {
            live.snapshot.comfy_ui = DaemonState {
                alive: true,
                models: vec!["x".to_owned()],
            };
        });
        assert!(liveness.current().comfy_ui.alive);
        liveness.mark_dead(Daemon::ComfyUi);
        assert!(!liveness.current().comfy_ui.alive);
        assert!(liveness.current().comfy_ui.models.is_empty());
    }

    #[tokio::test]
    async fn probe_against_a_dead_port_reports_not_alive() {
        // Port 1 is not listening; both probes should fail fast and report dead.
        let liveness = DaemonLiveness::new("http://127.0.0.1:1", "http://127.0.0.1:1");
        liveness.probe().await;
        let snapshot = liveness.current();
        assert!(!snapshot.comfy_ui.alive);
        assert!(!snapshot.a1111.alive);
    }
}
