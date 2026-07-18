//! The sidecar supervisor: process lifecycle, the frame wire, exclusive
//! sessions, watchdogs, and the two request pumps.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use kernel::capabilities::{AudioFrame, CapabilityChunk, GenerationStats};
use kernel::jobs::JobRuntimeEvent;
use kernel::records::JsonValue;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout};
use tokio::sync::{mpsc, oneshot};
use tokio::task::AbortHandle;

use crate::environment::scrubbed_environment;
use crate::frame_codec::{self, Decoder, Frame};
use crate::governor::lock;

use super::spec::{DEFAULT_SAMPLE_RATE, SidecarSpec};
use super::{SidecarError, SidecarStream};

const STDERR_TAIL_CHARS: usize = 2000;
const READ_CHUNK: usize = 64 * 1024;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

struct Watchdog {
    session: u64,
    handle: AbortHandle,
}

struct Sidecar {
    child: Child,
    generation: u64,
    decoder: Decoder,
    buffered: VecDeque<Frame>,
    waiter: Option<oneshot::Sender<Option<Frame>>>,
    eof: bool,
    sample_rate: i64,
    stderr_tail: String,
    busy: bool,
    job_session: Option<u64>,
    job_op_sent: bool,
    pending: Vec<oneshot::Sender<()>>,
    last_frame_at: Instant,
    writer: mpsc::UnboundedSender<Vec<u8>>,
}

#[derive(Default)]
struct State {
    sidecars: HashMap<String, Sidecar>,
    cancel_watchdogs: HashMap<String, Watchdog>,
    frame_watchdogs: HashMap<String, Watchdog>,
}

struct Inner {
    state: Mutex<State>,
    spawn_locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

/// Owns the live sidecar processes and their wire protocol. Cheap to clone (an
/// `Arc` handle) so tasks and pumps can hold their own reference.
#[derive(Clone)]
pub struct SidecarSupervisor {
    inner: Arc<Inner>,
}

impl Default for SidecarSupervisor {
    fn default() -> Self {
        Self::new()
    }
}

impl SidecarSupervisor {
    /// A supervisor with no running sidecars.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                state: Mutex::new(State::default()),
                spawn_locks: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// Whether `id`'s process is currently running.
    pub fn is_running(&self, id: &str) -> bool {
        let mut state = lock(&self.inner.state);
        state
            .sidecars
            .get_mut(id)
            .map(|sidecar| is_alive(&mut sidecar.child))
            .unwrap_or(false)
    }

    /// The OS process id of `id`'s sidecar, if it is tracked.
    pub fn process_identifier(&self, id: &str) -> Option<i32> {
        lock(&self.inner.state)
            .sidecars
            .get(id)
            .and_then(|sidecar| sidecar.child.id())
            .map(|pid| pid as i32)
    }

    /// Ensure `spec`'s sidecar is running, spawning (or respawning a dead one)
    /// and waiting for its `ready` handshake. A live sidecar under the same id
    /// returns immediately.
    pub async fn ensure_running(&self, spec: &SidecarSpec) -> Result<(), SidecarError> {
        let id = &spec.runtime_id;
        // Serialize spawn per id so a concurrent call cannot double-spawn: the
        // check-kill-insert-handshake sequence is not atomic (it drops the state
        // lock and awaits), which the Swift actor got for free.
        let spawn_lock = {
            let mut locks = lock(&self.inner.spawn_locks);
            Arc::clone(locks.entry(id.clone()).or_default())
        };
        let _spawn = spawn_lock.lock().await;
        {
            let mut state = lock(&self.inner.state);
            if let Some(sidecar) = state.sidecars.get_mut(id)
                && is_alive(&mut sidecar.child)
            {
                return Ok(());
            }
        }
        if self.contains(id) {
            self.kill(id);
        }

        let mut command = tokio::process::Command::new(&spec.executable);
        command.args(&spec.arguments);
        command.env_clear();
        for (key, value) in scrubbed_environment(std::env::vars(), &spec.environment) {
            command.env(key, value);
        }
        if let Some(dir) = &spec.working_directory {
            command.current_dir(dir);
        }
        command.stdin(std::process::Stdio::piped());
        command.stdout(std::process::Stdio::piped());
        command.stderr(std::process::Stdio::piped());

        let mut child = command.spawn().map_err(|err| SidecarError::SidecarDied {
            runtime_id: id.clone(),
            detail: format!("failed to launch: {err}"),
        })?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let generation = next_id();
        let (writer_tx, writer_rx) = mpsc::unbounded_channel();

        let sidecar = Sidecar {
            child,
            generation,
            decoder: Decoder::new(),
            buffered: VecDeque::new(),
            waiter: None,
            eof: false,
            sample_rate: DEFAULT_SAMPLE_RATE,
            stderr_tail: String::new(),
            busy: false,
            job_session: None,
            job_op_sent: false,
            pending: Vec::new(),
            last_frame_at: Instant::now(),
            writer: writer_tx,
        };
        lock(&self.inner.state).sidecars.insert(id.clone(), sidecar);

        if let Some(stdin) = stdin {
            tokio::spawn(writer_task(self.clone(), id.clone(), stdin, writer_rx));
        }
        if let Some(stdout) = stdout {
            tokio::spawn(reader_task(self.clone(), id.clone(), generation, stdout));
        }
        if let Some(stderr) = stderr {
            tokio::spawn(stderr_task(self.clone(), id.clone(), generation, stderr));
        }

        self.await_ready(spec).await
    }

    async fn await_ready(&self, spec: &SidecarSpec) -> Result<(), SidecarError> {
        let id = &spec.runtime_id;
        let ready = match tokio::time::timeout(spec.ready_timeout, self.next_frame(id)).await {
            Ok(frame) => frame,
            Err(_) => {
                self.expire_waiter(id);
                None
            }
        };
        let value = match ready {
            Some(Frame::Control(value)) if control_event(&value) == Some("ready") => value,
            _ => {
                let tail = self.stderr_tail(id);
                self.kill(id);
                return Err(SidecarError::SidecarDied {
                    runtime_id: id.clone(),
                    detail: format!("failed to start: {}", error_summary(&tail)),
                });
            }
        };
        if let Some(rate) = value
            .as_object()
            .and_then(|obj| obj.get("sample_rate"))
            .and_then(JsonValue::as_i64)
            && let Some(sidecar) = lock(&self.inner.state).sidecars.get_mut(id)
        {
            sidecar.sample_rate = rate;
        }
        Ok(())
    }

    /// Open a capability stream (chat/vision/speech/transcription/embeddings).
    pub fn request(
        &self,
        spec: &SidecarSpec,
        control: JsonValue,
    ) -> SidecarStream<CapabilityChunk> {
        let (tx, rx) = mpsc::unbounded_channel();
        let supervisor = self.clone();
        let spec = spec.clone();
        tokio::spawn(async move {
            let session = next_id();
            let id = spec.runtime_id.clone();
            let watcher = {
                let supervisor = supervisor.clone();
                let spec = spec.clone();
                let id = id.clone();
                let watch_tx = tx.clone();
                tokio::spawn(async move {
                    watch_tx.closed().await;
                    if spec.cooperative_cancel {
                        supervisor.send_cancel_and_arm(&id, session, spec.cancel_grace_timeout);
                    } else {
                        supervisor.kill_if_owns(&id, session);
                    }
                })
            };
            let result = supervisor.run_stream(&spec, control, session, &tx).await;
            watcher.abort();
            if let Err(err) = result {
                let _ = tx.send(Err(err));
            }
            supervisor.settle_stream(&id, session);
        });
        SidecarStream { rx }
    }

    /// Open a job stream (image generation). Cancellation is always cooperative
    /// with a kill-after-grace watchdog.
    pub fn job_request(
        &self,
        spec: &SidecarSpec,
        control: JsonValue,
    ) -> SidecarStream<JobRuntimeEvent> {
        let (tx, rx) = mpsc::unbounded_channel();
        let supervisor = self.clone();
        let spec = spec.clone();
        tokio::spawn(async move {
            let session = next_id();
            let id = spec.runtime_id.clone();
            let watcher = {
                let supervisor = supervisor.clone();
                let grace = spec.cancel_grace_timeout;
                let id = id.clone();
                let watch_tx = tx.clone();
                tokio::spawn(async move {
                    watch_tx.closed().await;
                    supervisor.send_cancel_and_arm(&id, session, grace);
                })
            };
            let result = supervisor.run_job(&spec, control, session, &tx).await;
            watcher.abort();
            if let Err(err) = result {
                let _ = tx.send(Err(err));
            }
            supervisor.settle_stream(&id, session);
        });
        SidecarStream { rx }
    }

    async fn run_stream(
        &self,
        spec: &SidecarSpec,
        control: JsonValue,
        session: u64,
        tx: &mpsc::UnboundedSender<Result<CapabilityChunk, SidecarError>>,
    ) -> Result<(), SidecarError> {
        let id = &spec.runtime_id;
        let _guard = self.acquire_exclusive(id, session).await;
        if tx.is_closed() {
            return Ok(());
        }
        self.send(id, control)?;
        self.mark_job_op_sent(id, session);
        self.arm_frame_watchdog(id, session, spec.frame_timeout);
        self.pump(spec, tx).await
    }

    async fn run_job(
        &self,
        spec: &SidecarSpec,
        control: JsonValue,
        session: u64,
        tx: &mpsc::UnboundedSender<Result<JobRuntimeEvent, SidecarError>>,
    ) -> Result<(), SidecarError> {
        let id = &spec.runtime_id;
        let _guard = self.acquire_exclusive(id, session).await;
        if tx.is_closed() {
            return Ok(());
        }
        self.send(id, control)?;
        self.mark_job_op_sent(id, session);
        self.arm_frame_watchdog(id, session, spec.frame_timeout);
        self.pump_job(spec, tx).await
    }

    async fn pump(
        &self,
        spec: &SidecarSpec,
        tx: &mpsc::UnboundedSender<Result<CapabilityChunk, SidecarError>>,
    ) -> Result<(), SidecarError> {
        let id = &spec.runtime_id;
        let sample_rate = lock(&self.inner.state)
            .sidecars
            .get(id)
            .map(|sidecar| sidecar.sample_rate)
            .unwrap_or(DEFAULT_SAMPLE_RATE);

        while let Some(frame) = self.next_frame(id).await {
            let value = match frame {
                Frame::Binary(data) => {
                    let _ = tx.send(Ok(CapabilityChunk::Audio(AudioFrame::new(
                        data,
                        sample_rate,
                    ))));
                    continue;
                }
                Frame::Control(value) => value,
            };
            let object = value.as_object();
            match control_event(&value) {
                Some("begin") => {
                    let _ = tx.send(Ok(CapabilityChunk::Status("generating".to_owned())));
                }
                Some("text") => {
                    let text = string_field(object, "text");
                    let start = int_field(object, "t0_ms");
                    let end = int_field(object, "t1_ms");
                    let chunk = match (start, end) {
                        (Some(start_ms), Some(end_ms)) => CapabilityChunk::Segment {
                            text,
                            start_ms,
                            end_ms,
                        },
                        _ => CapabilityChunk::Text(text),
                    };
                    let _ = tx.send(Ok(chunk));
                }
                Some("thinking") => {
                    let _ = tx.send(Ok(CapabilityChunk::Thinking(string_field(object, "text"))));
                }
                Some("vector") => {
                    let values = object
                        .and_then(|obj| obj.get("values"))
                        .and_then(JsonValue::as_array)
                        .map(|entries| entries.iter().filter_map(JsonValue::as_f64).collect())
                        .unwrap_or_default();
                    let _ = tx.send(Ok(CapabilityChunk::Vector(values)));
                }
                Some("status") => {
                    let _ = tx.send(Ok(CapabilityChunk::Status(string_field(object, "message"))));
                }
                Some("done") => {
                    let _ = tx.send(Ok(CapabilityChunk::Done(Some(done_stats(object)))));
                    return Ok(());
                }
                Some("cancelled") => return Err(SidecarError::Cancelled),
                Some("error") => return Err(SidecarError::RuntimeFailed(error_message(object))),
                _ => continue,
            }
        }
        Err(self.frame_loop_exit_error(spec))
    }

    async fn pump_job(
        &self,
        spec: &SidecarSpec,
        tx: &mpsc::UnboundedSender<Result<JobRuntimeEvent, SidecarError>>,
    ) -> Result<(), SidecarError> {
        let id = &spec.runtime_id;
        while let Some(frame) = self.next_frame(id).await {
            let Frame::Control(value) = frame else {
                continue;
            };
            let object = value.as_object();
            match control_event(&value) {
                Some("begin") => {
                    let _ = tx.send(Ok(JobRuntimeEvent::Started));
                }
                Some("step") => {
                    let step = int_field(object, "n").unwrap_or(0);
                    let total_steps = int_field(object, "total").unwrap_or(0);
                    let _ = tx.send(Ok(JobRuntimeEvent::Progress { step, total_steps }));
                }
                Some("preview") => {
                    if let Some(data) = self.next_binary_frame(id).await {
                        let _ = tx.send(Ok(JobRuntimeEvent::Preview(data)));
                    }
                }
                Some("image") => {
                    let format = object
                        .and_then(|obj| obj.get("format"))
                        .and_then(JsonValue::as_str)
                        .unwrap_or("png")
                        .to_owned();
                    if let Some(data) = self.next_binary_frame(id).await {
                        let _ = tx.send(Ok(JobRuntimeEvent::Result {
                            data,
                            file_extension: format,
                        }));
                    }
                }
                Some("done") => return Ok(()),
                Some("cancelled") => return Err(SidecarError::Cancelled),
                Some("error") => return Err(SidecarError::RuntimeFailed(error_message(object))),
                _ => continue,
            }
        }
        Err(self.frame_loop_exit_error(spec))
    }

    async fn next_binary_frame(&self, id: &str) -> Option<Vec<u8>> {
        match self.next_frame(id).await {
            Some(Frame::Binary(data)) => Some(data),
            _ => None,
        }
    }

    fn frame_loop_exit_error(&self, spec: &SidecarSpec) -> SidecarError {
        let id = &spec.runtime_id;
        let (at_eof, tail) = {
            let state = lock(&self.inner.state);
            match state.sidecars.get(id) {
                Some(sidecar) => (sidecar.eof, sidecar.stderr_tail.trim().to_owned()),
                None => (true, String::new()),
            }
        };
        if !at_eof {
            self.kill(id);
            SidecarError::RuntimeFailed(format!(
                "the runtime made no progress for {:?} and was stopped",
                spec.frame_timeout
            ))
        } else {
            SidecarError::SidecarDied {
                runtime_id: id.clone(),
                detail: format!("stopped unexpectedly: {}", error_summary(&tail)),
            }
        }
    }

    async fn acquire_exclusive(&self, id: &str, session: u64) -> ExclusiveGuard {
        loop {
            let receiver = {
                let mut state = lock(&self.inner.state);
                match state.sidecars.get_mut(id) {
                    Some(sidecar) if sidecar.busy => {
                        let (tx, rx) = oneshot::channel();
                        sidecar.pending.push(tx);
                        Some(rx)
                    }
                    Some(sidecar) => {
                        sidecar.busy = true;
                        sidecar.job_session = Some(session);
                        sidecar.job_op_sent = false;
                        None
                    }
                    None => None,
                }
            };
            match receiver {
                Some(receiver) => {
                    let _ = receiver.await;
                }
                None => break,
            }
        }
        ExclusiveGuard {
            inner: Arc::clone(&self.inner),
            id: id.to_owned(),
            session,
        }
    }

    fn mark_job_op_sent(&self, id: &str, session: u64) {
        if let Some(sidecar) = lock(&self.inner.state).sidecars.get_mut(id)
            && sidecar.job_session == Some(session)
        {
            sidecar.job_op_sent = true;
        }
    }

    async fn next_frame(&self, id: &str) -> Option<Frame> {
        let receiver = {
            let mut state = lock(&self.inner.state);
            let sidecar = state.sidecars.get_mut(id)?;
            if let Some(frame) = sidecar.buffered.pop_front() {
                return Some(frame);
            }
            if sidecar.eof {
                return None;
            }
            let (tx, rx) = oneshot::channel();
            sidecar.waiter = Some(tx);
            rx
        };
        receiver.await.ok().flatten()
    }

    fn expire_waiter(&self, id: &str) {
        if let Some(sidecar) = lock(&self.inner.state).sidecars.get_mut(id)
            && let Some(waiter) = sidecar.waiter.take()
        {
            let _ = waiter.send(None);
        }
    }

    fn stderr_tail(&self, id: &str) -> String {
        lock(&self.inner.state)
            .sidecars
            .get(id)
            .map(|sidecar| sidecar.stderr_tail.trim().to_owned())
            .unwrap_or_default()
    }

    fn ingest(&self, id: &str, generation: u64, data: &[u8]) {
        let mut state = lock(&self.inner.state);
        let Some(sidecar) = state.sidecars.get_mut(id) else {
            return;
        };
        if sidecar.generation != generation {
            return;
        }
        match sidecar.decoder.append(data) {
            Ok(frames) => {
                for frame in frames {
                    deliver(sidecar, frame);
                }
            }
            Err(_) => {
                drop(state);
                self.kill(id);
            }
        }
    }

    fn mark_eof(&self, id: &str, generation: u64) {
        let mut state = lock(&self.inner.state);
        let Some(sidecar) = state.sidecars.get_mut(id) else {
            return;
        };
        if sidecar.generation != generation {
            return;
        }
        sidecar.eof = true;
        if let Some(waiter) = sidecar.waiter.take() {
            let _ = waiter.send(None);
        }
        for pending in std::mem::take(&mut sidecar.pending) {
            let _ = pending.send(());
        }
    }

    fn append_stderr(&self, id: &str, generation: u64, text: &str) {
        let mut state = lock(&self.inner.state);
        let Some(sidecar) = state.sidecars.get_mut(id) else {
            return;
        };
        if sidecar.generation != generation {
            return;
        }
        sidecar.stderr_tail.push_str(text);
        let overflow = sidecar
            .stderr_tail
            .chars()
            .count()
            .saturating_sub(STDERR_TAIL_CHARS);
        if overflow > 0 {
            let start = sidecar
                .stderr_tail
                .char_indices()
                .nth(overflow)
                .map(|(index, _)| index)
                .unwrap_or(0);
            sidecar.stderr_tail = sidecar.stderr_tail[start..].to_owned();
        }
    }

    fn send(&self, id: &str, control: JsonValue) -> Result<(), SidecarError> {
        let data = frame_codec::encode(&Frame::Control(control)).map_err(|err| {
            SidecarError::RuntimeFailed(format!("failed to encode a frame: {err}"))
        })?;
        let sent = {
            let mut state = lock(&self.inner.state);
            match state.sidecars.get_mut(id) {
                Some(sidecar) => {
                    if is_alive(&mut sidecar.child) {
                        Some(sidecar.writer.send(data).is_ok())
                    } else {
                        None
                    }
                }
                None => None,
            }
        };
        match sent {
            Some(true) => Ok(()),
            Some(false) => {
                self.kill(id);
                Err(SidecarError::SidecarDied {
                    runtime_id: id.to_owned(),
                    detail: "write failed".to_owned(),
                })
            }
            None => Err(SidecarError::SidecarDied {
                runtime_id: id.to_owned(),
                detail: "is not running".to_owned(),
            }),
        }
    }

    fn send_cancel_and_arm(&self, id: &str, session: u64, grace: Duration) {
        let armed = {
            let state = lock(&self.inner.state);
            state
                .sidecars
                .get(id)
                .map(|sidecar| {
                    sidecar.busy && sidecar.job_session == Some(session) && sidecar.job_op_sent
                })
                .unwrap_or(false)
        };
        if !armed {
            return;
        }
        let _ = self.send(id, cancel_op());

        let mut state = lock(&self.inner.state);
        if let Some(watchdog) = state.cancel_watchdogs.remove(id) {
            watchdog.handle.abort();
        }
        let supervisor = self.clone();
        let id_owned = id.to_owned();
        let handle = tokio::spawn(async move {
            tokio::time::sleep(grace).await;
            supervisor.expire_cancel_watchdog(&id_owned, session);
        })
        .abort_handle();
        state
            .cancel_watchdogs
            .insert(id.to_owned(), Watchdog { session, handle });
    }

    fn expire_cancel_watchdog(&self, id: &str, session: u64) {
        let should_kill = {
            let mut state = lock(&self.inner.state);
            let matches = state
                .cancel_watchdogs
                .get(id)
                .map(|watchdog| watchdog.session == session)
                .unwrap_or(false);
            if !matches {
                return;
            }
            state.cancel_watchdogs.remove(id);
            state
                .sidecars
                .get(id)
                .map(|sidecar| sidecar.busy && sidecar.job_session == Some(session))
                .unwrap_or(false)
        };
        if should_kill {
            self.kill(id);
        }
    }

    fn settle_stream(&self, id: &str, session: u64) {
        let mut state = lock(&self.inner.state);
        let matches = state
            .cancel_watchdogs
            .get(id)
            .map(|watchdog| watchdog.session == session)
            .unwrap_or(false);
        if matches && let Some(watchdog) = state.cancel_watchdogs.remove(id) {
            watchdog.handle.abort();
        }
    }

    fn arm_frame_watchdog(&self, id: &str, session: u64, timeout: Duration) {
        let mut state = lock(&self.inner.state);
        if let Some(watchdog) = state.frame_watchdogs.remove(id) {
            watchdog.handle.abort();
        }
        let supervisor = self.clone();
        let id_owned = id.to_owned();
        let handle = tokio::spawn(async move {
            supervisor
                .run_frame_watchdog(&id_owned, session, timeout)
                .await;
        })
        .abort_handle();
        state
            .frame_watchdogs
            .insert(id.to_owned(), Watchdog { session, handle });
    }

    async fn run_frame_watchdog(&self, id: &str, session: u64, timeout: Duration) {
        loop {
            tokio::time::sleep(timeout).await;
            let mut state = lock(&self.inner.state);
            let Some(sidecar) = state.sidecars.get_mut(id) else {
                return;
            };
            if !(sidecar.busy && sidecar.job_session == Some(session)) {
                return;
            }
            if sidecar.last_frame_at.elapsed() >= timeout
                && let Some(waiter) = sidecar.waiter.take()
            {
                let _ = waiter.send(None);
            }
        }
    }

    /// Politely stop `id`: clear its watchdogs, send a `shutdown` op, wait up to
    /// three seconds for it to exit, then kill it if it ignored the request.
    pub async fn shutdown(&self, id: &str) {
        {
            let mut state = lock(&self.inner.state);
            abort_watchdogs(&mut state, id);
        }
        if !self.contains(id) {
            return;
        }
        if self.is_running(id) {
            let _ = self.send(id, shutdown_op());
            for _ in 0..30 {
                if !self.is_running(id) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            if self.is_running(id)
                && let Some(sidecar) = lock(&self.inner.state).sidecars.get_mut(id)
            {
                let _ = sidecar.child.start_kill();
            }
        }
        let mut state = lock(&self.inner.state);
        if let Some(mut sidecar) = state.sidecars.remove(id) {
            sidecar.eof = true;
            if let Some(waiter) = sidecar.waiter.take() {
                let _ = waiter.send(None);
            }
            for pending in std::mem::take(&mut sidecar.pending) {
                let _ = pending.send(());
            }
        }
    }

    /// Politely stop every live sidecar.
    pub async fn shutdown_all(&self) {
        for id in self.ids() {
            self.shutdown(&id).await;
        }
    }

    /// Kill every live sidecar immediately, no graceful shutdown.
    pub fn terminate_all(&self) {
        for id in self.ids() {
            self.kill(&id);
        }
    }

    fn kill(&self, id: &str) {
        let mut state = lock(&self.inner.state);
        abort_watchdogs(&mut state, id);
        if let Some(mut sidecar) = state.sidecars.remove(id) {
            let _ = sidecar.child.start_kill();
            if let Some(waiter) = sidecar.waiter.take() {
                let _ = waiter.send(None);
            }
            for pending in std::mem::take(&mut sidecar.pending) {
                let _ = pending.send(());
            }
        }
    }

    /// Kill `id` only if it is still busy on `session`. The consumer-drop watcher
    /// fires on both a genuine cancel and normal completion (the receiver closes
    /// either way), and `watcher.abort()` racing the synchronous `kill` is not
    /// airtight — this guard makes the stale post-completion firing a no-op while
    /// a real mid-stream cancel (same session, still busy) still kills.
    fn kill_if_owns(&self, id: &str, session: u64) {
        let owns = {
            let state = lock(&self.inner.state);
            state
                .sidecars
                .get(id)
                .map(|sidecar| sidecar.busy && sidecar.job_session == Some(session))
                .unwrap_or(false)
        };
        if owns {
            self.kill(id);
        }
    }

    fn contains(&self, id: &str) -> bool {
        lock(&self.inner.state).sidecars.contains_key(id)
    }

    fn ids(&self) -> Vec<String> {
        lock(&self.inner.state).sidecars.keys().cloned().collect()
    }
}

struct ExclusiveGuard {
    inner: Arc<Inner>,
    id: String,
    session: u64,
}

impl Drop for ExclusiveGuard {
    fn drop(&mut self) {
        let mut state = lock(&self.inner.state);
        let matches = state
            .sidecars
            .get(&self.id)
            .map(|sidecar| sidecar.job_session == Some(self.session))
            .unwrap_or(false);
        if !matches {
            return;
        }
        if let Some(watchdog) = state.frame_watchdogs.remove(&self.id) {
            watchdog.handle.abort();
        }
        if let Some(sidecar) = state.sidecars.get_mut(&self.id) {
            sidecar.job_session = None;
            sidecar.job_op_sent = false;
            sidecar.busy = false;
            for pending in std::mem::take(&mut sidecar.pending) {
                let _ = pending.send(());
            }
        }
    }
}

fn deliver(sidecar: &mut Sidecar, frame: Frame) {
    sidecar.last_frame_at = Instant::now();
    if let Some(waiter) = sidecar.waiter.take() {
        if let Err(Some(frame)) = waiter.send(Some(frame)) {
            sidecar.buffered.push_back(frame);
        }
    } else {
        sidecar.buffered.push_back(frame);
    }
}

fn abort_watchdogs(state: &mut State, id: &str) {
    if let Some(watchdog) = state.cancel_watchdogs.remove(id) {
        watchdog.handle.abort();
    }
    if let Some(watchdog) = state.frame_watchdogs.remove(id) {
        watchdog.handle.abort();
    }
}

fn is_alive(child: &mut Child) -> bool {
    matches!(child.try_wait(), Ok(None))
}

async fn writer_task(
    supervisor: SidecarSupervisor,
    id: String,
    mut stdin: ChildStdin,
    mut writer_rx: mpsc::UnboundedReceiver<Vec<u8>>,
) {
    while let Some(bytes) = writer_rx.recv().await {
        if stdin.write_all(&bytes).await.is_err() || stdin.flush().await.is_err() {
            supervisor.kill(&id);
            break;
        }
    }
}

async fn reader_task(
    supervisor: SidecarSupervisor,
    id: String,
    generation: u64,
    mut stdout: ChildStdout,
) {
    let mut buffer = vec![0u8; READ_CHUNK];
    loop {
        match stdout.read(&mut buffer).await {
            Ok(0) | Err(_) => {
                supervisor.mark_eof(&id, generation);
                break;
            }
            Ok(count) => supervisor.ingest(&id, generation, &buffer[..count]),
        }
    }
}

async fn stderr_task(
    supervisor: SidecarSupervisor,
    id: String,
    generation: u64,
    mut stderr: ChildStderr,
) {
    let mut buffer = vec![0u8; READ_CHUNK];
    loop {
        match stderr.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(count) => {
                let text = String::from_utf8_lossy(&buffer[..count]);
                supervisor.append_stderr(&id, generation, &text);
            }
        }
    }
}

fn control_event(value: &JsonValue) -> Option<&str> {
    value
        .as_object()
        .and_then(|obj| obj.get("event"))
        .and_then(JsonValue::as_str)
}

type ControlObject<'a> = Option<&'a BTreeMap<String, JsonValue>>;

fn string_field(object: ControlObject<'_>, key: &str) -> String {
    object
        .and_then(|obj| obj.get(key))
        .and_then(JsonValue::as_str)
        .unwrap_or("")
        .to_owned()
}

fn int_field(object: ControlObject<'_>, key: &str) -> Option<i64> {
    object
        .and_then(|obj| obj.get(key))
        .and_then(JsonValue::as_i64)
}

fn done_stats(object: ControlObject<'_>) -> GenerationStats {
    let seconds = object
        .and_then(|obj| obj.get("seconds"))
        .and_then(JsonValue::as_f64);
    GenerationStats {
        prompt_tokens: int_field(object, "prompt_tokens"),
        completion_tokens: int_field(object, "completion_tokens"),
        duration_ms: seconds.map(|value| (value * 1000.0) as i64),
        ..Default::default()
    }
}

fn error_message(object: ControlObject<'_>) -> String {
    object
        .and_then(|obj| obj.get("message"))
        .and_then(JsonValue::as_str)
        .unwrap_or("sidecar error")
        .to_owned()
}

fn error_summary(tail: &str) -> String {
    tail.lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
        .trim()
        .to_owned()
}

fn cancel_op() -> JsonValue {
    op("cancel")
}

fn shutdown_op() -> JsonValue {
    op("shutdown")
}

fn op(name: &str) -> JsonValue {
    let mut map = BTreeMap::new();
    map.insert("op".to_owned(), JsonValue::String(name.to_owned()));
    JsonValue::Object(map)
}
