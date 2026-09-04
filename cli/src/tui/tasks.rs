//! Background work: each task runs on the tokio runtime and reports over the
//! event channel, so the loop never blocks on the kernel. This is the only
//! module in the UI that awaits kernel calls.

use std::fs;
use std::future::Future;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime};

use gateway::audit::{GatewayAuditEntry, GatewayAuditLog};
use kernel::capabilities::CapabilityChunk;
use kernel::discovery::service::DiscoverySummary;
use kernel::install::event::InstallProgress;
use kernel::install::plan::InstallPlan;
use kernel::install::provider::InstallProviderId;
use kernel::install::pulls::{PullControl, PullStore};
use kernel::records::{Capability, JsonValue, ModelRecord};
use kernel::time::now_millis;
use runtime::install::service::InstallService;
use runtime::install::{Started, restart, start_or_join, stop};
use serde_json::json;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::event::{Event, Planned, Refreshed, Reply, ReplyStep, Searched};
use super::facts::Facts;
use super::jobs;
use super::pull::SEARCH_LIMIT;
use super::text;
use crate::support::removal::remove_and_forget;
use crate::support::residency::{is_resident, unload_anywhere, warm_request};
use crate::support::session::Session;

/// How long a warm through the gateway may take; a large model legitimately
/// loads for minutes, so this only catches a gateway that never answers.
const GATEWAY_WARM_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// The shelf and the facts about it, read together.
pub struct Snapshot {
    pub records: Vec<ModelRecord>,
    pub facts: Facts,
}

impl Snapshot {
    /// This snapshot as the refresh event it becomes, stamped in the refresh
    /// order.
    pub fn stamped(self, sequence: u64) -> Refreshed {
        Refreshed {
            sequence,
            records: self.records,
            facts: self.facts,
        }
    }
}

/// What tasks run against: the kernel session, the install service the pull
/// modal plans through, and the audit log the machine facts are read from.
pub struct TaskContext {
    session: Arc<Session>,
    install: InstallService,
    audit: AuditReader,
    /// Tasks that change the shelf or the disk; the loop waits for them on
    /// quit, so a removal is never cut between deleting and forgetting.
    mutating: Mutex<Vec<JoinHandle<()>>>,
    /// The reply streaming into the chat pane; aborting it is how a reply
    /// stops, and one is enough since the pane sends one ask at a time.
    ask: Mutex<Option<JoinHandle<()>>>,
}

impl TaskContext {
    /// A context over `session`, installing through `install`.
    pub fn new(session: Arc<Session>, install: InstallService) -> Self {
        let audit = AuditReader::new(session.dirs.sub("gateway"));
        Self {
            session,
            install,
            audit,
            mutating: Mutex::new(Vec::new()),
            ask: Mutex::new(None),
        }
    }

    /// Stop the reply streaming into the chat pane, if one is.
    pub fn stop_ask(&self) {
        if let Some(handle) = self.ask().take() {
            handle.abort();
        }
    }

    fn ask(&self) -> MutexGuard<'_, Option<JoinHandle<()>>> {
        self.ask.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Wait for every mutating task still running; the tasks that were only
    /// reading are dropped with the runtime.
    pub async fn settle(&self) {
        self.stop_ask();
        let handles: Vec<JoinHandle<()>> =
            std::mem::take(&mut *self.mutating.lock().unwrap_or_else(PoisonError::into_inner));
        for handle in handles {
            let _ = handle.await;
        }
    }

    /// The session the tasks run against.
    pub fn session(&self) -> &Arc<Session> {
        &self.session
    }

    /// Whether a mutating task is still running.
    pub fn busy(&self) -> bool {
        self.mutating
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .any(|handle| !handle.is_finished())
    }

    /// The pull jobs on this machine, whoever started them.
    pub fn pull_store(&self) -> PullStore {
        runtime::boot::pull_store(&self.session.dirs)
    }

    /// The shelf and facts as they are now. The audit log is parsed on a
    /// blocking thread so a busy gateway's log never stalls the loop.
    pub async fn snapshot(self: &Arc<Self>) -> Snapshot {
        let records = self.session.shelf().await.to_vec();
        let reader = Arc::clone(self);
        let entries = tokio::task::spawn_blocking(move || reader.audit.entries())
            .await
            .unwrap_or_else(|_| Vec::new().into());
        let facts = Facts::collect(&self.session, &records, &entries).await;
        Snapshot { records, facts }
    }
}

/// A task's identity for the strip; the counter never repeats within a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TaskId(u64);

static NEXT_TASK: AtomicU64 = AtomicU64::new(1);
static NEXT_REFRESH: AtomicU64 = AtomicU64::new(1);

impl TaskId {
    /// A fresh id.
    pub(super) fn next() -> Self {
        Self(NEXT_TASK.fetch_add(1, Ordering::Relaxed))
    }
}

/// What a task does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskKind {
    /// Scan the machine's stores and reconcile the shelf.
    Scan,
    /// Load a model in this process.
    Warm { id: String, name: String },
    /// Load a model through the gateway on `port`, where it will be served.
    WarmViaGateway { id: String, name: String, port: u16 },
    /// Evict a model from this process, or from the Ollama daemon if it holds it.
    Unload { id: String, name: String },
    /// Delete a model's weights and forget its record.
    Remove { id: String, name: String },
}

/// What the strip calls a task: a verb and its subject.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskLabel {
    pub verb: &'static str,
    pub subject: String,
}

impl TaskKind {
    /// Every verb a strip row can start with, the hand-offs' included; the
    /// strip's verb column is as wide as the widest.
    pub const VERBS: [&'static str; 8] = [
        "scan", "warm", "unload", "pull", "remove", "launch", "chat", "serve",
    ];

    /// The label the strip shows for this kind.
    pub fn label(&self) -> TaskLabel {
        TaskLabel {
            verb: self.verb(),
            subject: self.subject().to_owned(),
        }
    }

    /// The verb the strip shows.
    pub fn verb(&self) -> &'static str {
        match self {
            TaskKind::Scan => "scan",
            TaskKind::Warm { .. } | TaskKind::WarmViaGateway { .. } => "warm",
            TaskKind::Unload { .. } => "unload",
            TaskKind::Remove { .. } => "remove",
        }
    }

    /// The subject the strip shows after the verb.
    pub fn subject(&self) -> &str {
        match self {
            TaskKind::Scan => "this machine",
            TaskKind::Warm { name, .. }
            | TaskKind::WarmViaGateway { name, .. }
            | TaskKind::Unload { name, .. }
            | TaskKind::Remove { name, .. } => name,
        }
    }

    /// The model this task concerns, if it concerns one.
    pub fn model_id(&self) -> Option<&str> {
        match self {
            TaskKind::Scan => None,
            TaskKind::Warm { id, .. }
            | TaskKind::WarmViaGateway { id, .. }
            | TaskKind::Unload { id, .. }
            | TaskKind::Remove { id, .. } => Some(id),
        }
    }

    /// What the strip says while the task runs.
    pub fn activity(&self) -> &'static str {
        match self {
            TaskKind::Scan => "looking through the stores",
            TaskKind::Warm { .. } => "loading in this process",
            TaskKind::WarmViaGateway { .. } => "loading on the gateway",
            TaskKind::Unload { .. } => "evicting",
            TaskKind::Remove { .. } => "deleting",
        }
    }
}

/// Where a task is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskState {
    /// Still going.
    Running,
    /// Still going, with a status line from the provider.
    Status(String),
    /// Downloading.
    Downloading(InstallProgress),
    /// Finished, with a one-line result.
    Done(String),
    /// Stopped with work worth going on from, and how it stopped.
    Stopped(String),
    /// Gave up, with the reason.
    Failed(String),
}

impl TaskState {
    /// Whether the task is still going.
    pub fn running(&self) -> bool {
        !matches!(
            self,
            TaskState::Done(_) | TaskState::Stopped(_) | TaskState::Failed(_)
        )
    }
}

/// A task's progress, as sent over the channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskEvent {
    pub id: TaskId,
    pub state: TaskState,
}

/// Start `kind` on the runtime, reporting to `tx`; the returned id names it in
/// the strip. Every task ends with a `Done` or `Failed` event.
pub fn spawn(
    kind: &TaskKind,
    context: &Arc<TaskContext>,
    tx: &mpsc::UnboundedSender<Event>,
) -> TaskId {
    let id = TaskId::next();
    let tx = tx.clone();
    let mutating = matches!(kind, TaskKind::Remove { .. });
    let runner = Arc::clone(context);
    let kind = kind.clone();
    let handle = tokio::spawn(async move {
        let context = runner;
        let session = &context.session;
        let report = |state: TaskState| {
            let _ = tx.send(Event::Task(TaskEvent { id, state }));
        };
        let outcome = match kind {
            TaskKind::Scan => scan(session).await,
            TaskKind::Warm { id, .. } => warm(session, &id).await,
            TaskKind::WarmViaGateway { id, port, .. } => warm_via_gateway(&id, port).await,
            TaskKind::Unload { id, .. } => unload(session, &id).await,
            TaskKind::Remove { id, .. } => remove(session, &id).await,
        };
        report(match outcome {
            Ok(summary) => TaskState::Done(summary),
            Err(reason) => TaskState::Failed(reason),
        });
    });
    if mutating {
        let mut handles = context
            .mutating
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        handles.retain(|handle| !handle.is_finished());
        handles.push(handle);
    }
    id
}

/// Read the pull jobs and hand the rows to the loop.
///
/// The read is filesystem work, so it happens off the loop's thread; the rows
/// come back as an event like any other answer.
pub fn spawn_pulls(context: &Arc<TaskContext>, tx: &mpsc::UnboundedSender<Event>) {
    let store = context.pull_store();
    let tx = tx.clone();
    tokio::task::spawn_blocking(move || {
        let _ = tx.send(Event::Pulls(jobs::rows(&store, now_millis())));
    });
}

/// Create a job for `plan` and start a worker on it, or join the pull of that
/// model already under way. The strip picks the job up on its next poll.
pub fn spawn_start_pull(
    plan: InstallPlan,
    context: &Arc<TaskContext>,
    tx: &mpsc::UnboundedSender<Event>,
) {
    let store = context.pull_store();
    let tx = tx.clone();
    tokio::spawn(async move {
        match start_pull(&store, plan) {
            Ok(Some(note)) | Err(note) => {
                let _ = tx.send(Event::PullRefused(note));
            }
            Ok(None) => {}
        }
        spawn_pulls_into(&store, &tx);
    });
}

/// Ask the job named by `id` to stop, or put a worker back on it.
pub fn spawn_pull_control(
    action: PullAction,
    id: String,
    context: &Arc<TaskContext>,
    tx: &mpsc::UnboundedSender<Event>,
) {
    let store = context.pull_store();
    let tx = tx.clone();
    tokio::spawn(async move {
        if let Err(reason) = act_on_pull(&store, action, &id) {
            let _ = tx.send(Event::PullRefused(reason));
        }
        spawn_pulls_into(&store, &tx);
    });
}

/// What a key on a pull row asks of its job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PullAction {
    /// Stop it, keeping what landed for a resume.
    Pause,
    /// Stop it for good.
    Cancel,
    /// Put a worker back on it.
    Resume,
}

/// Send the rows as they read right now, so a key that changed one shows its
/// effect without waiting for the next poll.
fn spawn_pulls_into(store: &PullStore, tx: &mpsc::UnboundedSender<Event>) {
    let _ = tx.send(Event::Pulls(jobs::rows(store, now_millis())));
}

/// Start a pull, or join the one already fetching that model, and say which
/// happened when it was not simply started.
fn start_pull(store: &PullStore, plan: InstallPlan) -> Result<Option<String>, String> {
    match start_or_join(store, &plan) {
        Ok(Started::Created) => Ok(None),
        Ok(Started::Joined) => Ok(Some(format!("{} is already downloading", plan.reference))),
        Ok(Started::Resumed) => Ok(Some(format!("carrying on the pull of {}", plan.reference))),
        Err(error) => Err(format!("{}: {error}", plan.reference)),
    }
}

fn act_on_pull(store: &PullStore, action: PullAction, id: &str) -> Result<(), String> {
    let job = store.open(id).map_err(|error| error.to_string())?;
    match action {
        PullAction::Pause => stop(&job, PullControl::Pause).map(|_| ()),
        PullAction::Cancel => stop(&job, PullControl::Cancel).map(|_| ()),
        PullAction::Resume => restart(&job).map(|_| ()),
    }
    .map_err(|error| error.to_string())
}

/// Run `work` against the context and hand its answer to the loop; for the
/// asks whose answer is an event of its own rather than a strip row.
fn fire<F>(
    context: &Arc<TaskContext>,
    tx: &mpsc::UnboundedSender<Event>,
    work: impl FnOnce(Arc<TaskContext>) -> F + Send + 'static,
) where
    F: Future<Output = Event> + Send + 'static,
{
    let context = Arc::clone(context);
    let tx = tx.clone();
    tokio::spawn(async move {
        let _ = tx.send(work(context).await);
    });
}

/// Search the providers for `query`, reporting the hits.
pub fn spawn_search(query: String, context: &Arc<TaskContext>, tx: &mpsc::UnboundedSender<Event>) {
    fire(context, tx, |context| async move {
        let result = context.install.browse(&query, SEARCH_LIMIT).await;
        Event::Searched(Searched {
            query,
            hits: result.hits,
            note: result.failure_hint,
        })
    });
}

/// Resolve the plan for `reference`, reporting it or the reason there is
/// none, as the answer to ask number `ask`.
pub fn spawn_plan(
    provider: InstallProviderId,
    reference: String,
    ask: u64,
    context: &Arc<TaskContext>,
    tx: &mpsc::UnboundedSender<Event>,
) {
    fire(context, tx, move |context| async move {
        let result = context
            .install
            .plan(&provider, &reference)
            .await
            .map_err(|error| error.to_string());
        Event::Planned(Planned { ask, result })
    });
}

/// Stream the chat reply to `payload` from `record_id`, reporting each piece
/// of text and then the end. The task is held by the context, which stops a
/// reply by dropping its stream ([`TaskContext::stop_ask`]); a new ask
/// replaces one still running. Every ask ends with a `Done` or `Failed` step.
pub fn spawn_ask(
    record_id: String,
    payload: JsonValue,
    generation: u64,
    context: &Arc<TaskContext>,
    tx: &mpsc::UnboundedSender<Event>,
) {
    context.stop_ask();
    let runner = Arc::clone(context);
    let tx = tx.clone();
    let handle = tokio::spawn(async move {
        let context = runner;
        let report = |step: ReplyStep| {
            let _ = tx.send(Event::Reply(Reply { generation, step }));
        };
        let stream = context
            .session
            .kernel
            .invoke_with(&record_id, Capability::chat(), payload, None, None)
            .await;
        let mut stream = match stream {
            Ok(stream) => stream,
            Err(error) => return report(ReplyStep::Failed(error.to_string())),
        };
        let mut stats = None;
        while let Some(result) = stream.recv().await {
            match result {
                Ok(CapabilityChunk::Text(text)) => report(ReplyStep::Text(text)),
                Ok(CapabilityChunk::Done(reported)) => stats = reported,
                Ok(_) => {}
                Err(error) => return report(ReplyStep::Failed(error.to_string())),
            }
        }
        report(ReplyStep::Done(stats));
    });
    *context.ask() = Some(handle);
}

/// The next place in the refresh order, for a snapshot taken outside
/// [`spawn_refresh`] so it still outranks every refresh spawned before it.
pub fn next_refresh_sequence() -> u64 {
    NEXT_REFRESH.fetch_add(1, Ordering::Relaxed)
}

/// Re-read the shelf and the machine facts, reporting them as one event
/// stamped with a sequence so a slow older refresh never overwrites a newer.
pub fn spawn_refresh(context: &Arc<TaskContext>, tx: &mpsc::UnboundedSender<Event>) {
    let sequence = next_refresh_sequence();
    fire(context, tx, move |context| async move {
        // The registry is re-read from disk first. A pull worker registers what
        // it fetched in a process of its own, and so does anything else on the
        // machine, so a shelf read only from memory goes stale without ever
        // saying so.
        let _ = context.session.kernel.reload_registry().await;
        Event::Refreshed(context.snapshot().await.stamped(sequence))
    });
}

async fn scan(session: &Session) -> Result<String, String> {
    let summary = session
        .discover()
        .await
        .map_err(|error| error.to_string())?;
    Ok(scan_summary(&summary))
}

/// `found 12 models · 9 hf · 3 ollama · 2 issues` in the strip's own
/// register, the stores in the order `per_kind` keeps them, sorted by
/// kind, or `found nothing`.
fn scan_summary(summary: &DiscoverySummary) -> String {
    if summary.total_count == 0 {
        return "found nothing".to_owned();
    }
    let mut parts = vec![format!(
        "found {}",
        text::count(summary.total_count, "model")
    )];
    parts.extend(
        summary
            .per_kind
            .iter()
            .filter(|(_, stat)| stat.count > 0)
            .map(|(kind, stat)| format!("{} {}", stat.count, text::short_store(kind.as_str()))),
    );
    if !summary.issues.is_empty() {
        parts.push(text::count(summary.issues.len(), "issue"));
    }
    parts.join(" · ")
}

async fn warm(session: &Session, id: &str) -> Result<String, String> {
    let shelf = session.shelf().await;
    let record = shelf
        .iter()
        .find(|record| record.id == id)
        .ok_or_else(|| "no longer on the shelf".to_owned())?;
    let (capability, payload) = warm_request(record).ok_or_else(|| "can't be warmed".to_owned())?;
    let mut stream = session
        .kernel
        .invoke(&record.id, capability, payload)
        .await
        .map_err(|error| error.to_string())?;
    while let Some(result) = stream.recv().await {
        result.map_err(|error| error.to_string())?;
    }
    Ok(match is_resident(session, record).await {
        Ok(true) => "warm in this process".to_owned(),
        Ok(false) => "loaded · residency not tracked".to_owned(),
        Err(reason) => format!("loaded · {reason}"),
    })
}

/// A one-token chat through the gateway, so the model loads where it serves.
/// The record id is the one name the gateway can never find ambiguous.
async fn warm_via_gateway(id: &str, port: u16) -> Result<String, String> {
    let body = json!({
        "model": id,
        "messages": [{ "role": "user", "content": "hi" }],
        "stream": false,
        "options": { "num_predict": 1 },
    });
    let response = reqwest::Client::builder()
        .timeout(GATEWAY_WARM_TIMEOUT)
        .build()
        .map_err(|error| error.to_string())?
        .post(format!("http://127.0.0.1:{port}/api/chat"))
        .json(&body)
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if response.status().is_success() {
        return Ok(format!("warm on the gateway :{port}"));
    }
    let status = response.status();
    let reason = response.text().await.unwrap_or_default();
    Err(if reason.is_empty() {
        format!("gateway answered {status}")
    } else {
        reason
    })
}

async fn unload(session: &Session, id: &str) -> Result<String, String> {
    let shelf = session.shelf().await;
    let record = shelf
        .iter()
        .find(|record| record.id == id)
        .ok_or_else(|| "no longer on the shelf".to_owned())?;
    let resident = unload_anywhere(session, record)
        .await
        .map_err(|error| error.to_string())?;
    if resident {
        Err("still resident; something is using it".to_owned())
    } else {
        Ok("unloaded".to_owned())
    }
}

/// Delete the weights the way `hedos rm` does, then forget the record so the
/// shelf stops listing it. The warm check is repeated here on live state: the
/// facts the modal read may be a refresh behind.
async fn remove(session: &Session, id: &str) -> Result<String, String> {
    let shelf = session.shelf().await;
    let record = shelf
        .iter()
        .find(|record| record.id == id)
        .ok_or_else(|| "no longer on the shelf".to_owned())?;
    let gateway_holds = session
        .live_gateway()
        .await
        .is_some_and(|live| live.residents.iter().any(|resident| resident.id == id));
    if gateway_holds || is_resident(session, record).await? {
        return Err("is warm; unload it first".to_owned());
    }
    let report = remove_and_forget(session, record)
        .await
        .map_err(|error| error.to_string())?;
    Ok(if report.daemon_deleted {
        format!(
            "removed through the Ollama daemon, up to {} freed",
            text::bytes(report.freed_bytes_estimate)
        )
    } else {
        format!("freed {}", text::bytes(report.freed_bytes_estimate))
    })
}

/// The gateway audit log, re-read only when the live file changes.
struct AuditReader {
    log: GatewayAuditLog,
    cache: Mutex<Option<AuditSnapshot>>,
}

/// The entries as read when the live file had this size and mtime.
struct AuditSnapshot {
    stamp: (u64, Option<SystemTime>),
    entries: Arc<[GatewayAuditEntry]>,
}

impl AuditReader {
    /// A reader over the audit log in `directory`.
    fn new(directory: PathBuf) -> Self {
        Self {
            log: GatewayAuditLog::new(directory),
            cache: Mutex::new(None),
        }
    }

    /// Every entry, from the cache unless the live file's size or mtime moved.
    /// Reads and parses on the calling thread; run it off the async workers.
    fn entries(&self) -> Arc<[GatewayAuditEntry]> {
        let stamp = fs::metadata(self.log.path())
            .map(|meta| (meta.len(), meta.modified().ok()))
            .unwrap_or((0, None));
        let mut cache = self.cache.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(snapshot) = cache.as_ref()
            && snapshot.stamp == stamp
        {
            return Arc::clone(&snapshot.entries);
        }
        let entries: Arc<[GatewayAuditEntry]> = self.log.read_all().into();
        *cache = Some(AuditSnapshot {
            stamp,
            entries: Arc::clone(&entries),
        });
        entries
    }
}

#[cfg(test)]
mod tests;
