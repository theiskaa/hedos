//! Background work: each task runs on the tokio runtime and reports over the
//! event channel, so the loop never blocks on the kernel. This is the only
//! module in the UI that awaits kernel calls.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::future::Future;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime};

use gateway::audit::{GatewayAuditEntry, GatewayAuditLog};
use kernel::capabilities::CapabilityChunk;
use kernel::install::event::{InstallEvent, InstallProgress};
use kernel::install::plan::InstallPlan;
use kernel::install::provider::InstallProviderId;
use kernel::records::{Capability, JsonValue, ModelRecord};
use runtime::install::service::InstallService;
use serde_json::json;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::event::{Event, Planned, Refreshed, Reply, ReplyStep, Searched};
use super::facts::Facts;
use super::pull::SEARCH_LIMIT;
use super::text;
use crate::support::removal::remove_and_forget;
use crate::support::residency::{is_resident, residency_outcome, unload_anywhere, warm_request};
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

/// What tasks run against: the kernel session, the install service, and the
/// install ids of the pulls in flight, so a row in the strip can be cancelled.
pub struct TaskContext {
    session: Arc<Session>,
    install: InstallService,
    audit: AuditReader,
    installs: Mutex<HashMap<TaskId, String>>,
    /// Pulls cancelled before their download had an id to cancel by; the
    /// pull honours it the moment it has one.
    cancelled: Mutex<HashSet<TaskId>>,
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
            installs: Mutex::new(HashMap::new()),
            cancelled: Mutex::new(HashSet::new()),
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

    /// Cancel the pull running as task `id`; one that has not begun
    /// downloading yet is cancelled as soon as it does.
    pub fn cancel(&self, id: TaskId) {
        match self.installs().get(&id) {
            Some(install_id) => self.install.cancel(install_id),
            None => {
                self.cancelled().insert(id);
            }
        }
    }

    fn cancelled(&self) -> MutexGuard<'_, HashSet<TaskId>> {
        self.cancelled
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn installs(&self) -> MutexGuard<'_, HashMap<TaskId, String>> {
        self.installs.lock().unwrap_or_else(PoisonError::into_inner)
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
    /// Download a model along a resolved plan.
    Pull(InstallPlan),
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
            TaskKind::Pull(_) => "pull",
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
            TaskKind::Pull(plan) => &plan.reference,
        }
    }

    /// The model this task concerns, if it concerns one.
    pub fn model_id(&self) -> Option<&str> {
        match self {
            TaskKind::Scan | TaskKind::Pull(_) => None,
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
            TaskKind::Pull(_) => "starting",
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
    /// Gave up, with the reason.
    Failed(String),
}

impl TaskState {
    /// Whether the task is still going.
    pub fn running(&self) -> bool {
        !matches!(self, TaskState::Done(_) | TaskState::Failed(_))
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
    let mutating = matches!(kind, TaskKind::Pull(_) | TaskKind::Remove { .. });
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
            TaskKind::Pull(plan) => {
                let outcome = pull(&context, id, plan, &report).await;
                context.installs().remove(&id);
                outcome
            }
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
        Event::Refreshed(context.snapshot().await.stamped(sequence))
    });
}

async fn scan(session: &Session) -> Result<String, String> {
    let summary = session
        .discover()
        .await
        .map_err(|error| error.to_string())?;
    let mut line = summary.headline();
    if !summary.issues.is_empty() {
        line.push_str(&format!(
            " · {}",
            text::count(summary.issues.len(), "issue")
        ));
    }
    Ok(line)
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
        Ok(resident) => residency_outcome(resident).to_owned(),
        Err(reason) => format!("loaded; {reason}"),
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
        return Ok(format!("warm on the gateway at :{port}"));
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

/// Begin `plan` and follow the download to its end, then rescan so the new
/// model lands on the shelf.
async fn pull(
    context: &TaskContext,
    task: TaskId,
    plan: InstallPlan,
    report: &impl Fn(TaskState),
) -> Result<String, String> {
    let reference = plan.reference.clone();
    let install_id = context
        .install
        .begin(plan)
        .map_err(|error| error.to_string())?;
    context.installs().insert(task, install_id.clone());
    if context.cancelled().remove(&task) {
        context.install.cancel(&install_id);
    }
    let mut events = context.install.events(&install_id);
    while let Some(event) = events.recv().await {
        match event {
            InstallEvent::Progress(progress) => report(TaskState::Downloading(progress)),
            InstallEvent::Status(status) => report(TaskState::Status(status)),
            InstallEvent::Failed { message } => return Err(message),
            InstallEvent::Cancelled => return Err("cancelled".to_owned()),
            InstallEvent::Done => break,
            InstallEvent::Queued | InstallEvent::Preparing => {}
        }
    }
    report(TaskState::Status("adding to the shelf".to_owned()));
    context
        .session
        .discover()
        .await
        .map_err(|error| error.to_string())?;
    Ok(format!("pulled {reference}"))
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
