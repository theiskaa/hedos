//! Background work: each task runs on the tokio runtime and reports over the
//! event channel, so the loop never blocks on the kernel. This is the only
//! module in the UI that awaits kernel calls.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use kernel::install::event::{InstallEvent, InstallProgress};
use kernel::install::plan::InstallPlan;
use kernel::install::provider::InstallProviderId;
use kernel::records::ModelRecord;
use runtime::install::service::InstallService;
use serde_json::json;
use tokio::sync::mpsc;

use super::event::{Event, Planned, Refreshed, Searched};
use super::facts::{AuditReader, Facts};
use super::pull::SEARCH_LIMIT;
use super::text;
use crate::commands::rm::remove_and_forget;
use crate::commands::warm::{is_resident, residency_outcome, warm_request};
use crate::support::session::Session;

/// The shelf and the facts about it, read together.
pub struct Snapshot {
    pub records: Vec<ModelRecord>,
    pub facts: Facts,
}

/// What tasks run against: the kernel session, the install service, and the
/// install ids of the pulls in flight, so a row in the strip can be cancelled.
pub struct TaskContext {
    session: Arc<Session>,
    install: InstallService,
    audit: AuditReader,
    installs: Mutex<HashMap<TaskId, String>>,
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
        }
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

    /// Cancel the pull running as task `id`, if it has begun downloading.
    pub fn cancel(&self, id: TaskId) {
        if let Some(install_id) = self.installs().get(&id) {
            self.install.cancel(install_id);
        }
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
    /// Evict a model from this process.
    Unload { id: String, name: String },
    /// Download a model along a resolved plan.
    Pull(InstallPlan),
    /// Delete a model's weights and forget its record.
    Remove { id: String, name: String },
}

impl TaskKind {
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
    tx: mpsc::UnboundedSender<Event>,
) -> TaskId {
    let id = TaskId::next();
    let context = Arc::clone(context);
    let kind = kind.clone();
    tokio::spawn(async move {
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
    id
}

/// Search the providers for `query`, reporting the hits.
pub fn spawn_search(query: String, context: &Arc<TaskContext>, tx: mpsc::UnboundedSender<Event>) {
    let context = Arc::clone(context);
    tokio::spawn(async move {
        let result = context.install.browse(&query, SEARCH_LIMIT).await;
        let _ = tx.send(Event::Searched(Searched {
            query,
            hits: result.hits,
            note: result.failure_hint,
        }));
    });
}

/// Resolve the plan for `reference`, reporting it or the reason there is none.
pub fn spawn_plan(
    provider: InstallProviderId,
    reference: String,
    context: &Arc<TaskContext>,
    tx: mpsc::UnboundedSender<Event>,
) {
    let context = Arc::clone(context);
    tokio::spawn(async move {
        let result = context
            .install
            .plan(&provider, &reference)
            .await
            .map_err(|error| error.to_string());
        let _ = tx.send(Event::Planned(Planned { reference, result }));
    });
}

/// Re-read the shelf and the machine facts, reporting them as one event
/// stamped with a sequence so a slow older refresh never overwrites a newer.
pub fn spawn_refresh(context: &Arc<TaskContext>, tx: mpsc::UnboundedSender<Event>) {
    let sequence = NEXT_REFRESH.fetch_add(1, Ordering::Relaxed);
    let context = Arc::clone(context);
    tokio::spawn(async move {
        let Snapshot { records, facts } = context.snapshot().await;
        let _ = tx.send(Event::Refreshed(Refreshed {
            sequence,
            records,
            facts,
        }));
    });
}

async fn scan(session: &Session) -> Result<String, String> {
    let summary = session
        .discover()
        .await
        .map_err(|error| error.to_string())?;
    let mut line = summary.headline();
    if !summary.issues.is_empty() {
        line.push_str(&format!(" · {} issue(s)", summary.issues.len()));
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
    Ok(residency_outcome(is_resident(session, id)).to_owned())
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
    let response = reqwest::Client::new()
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
    session.kernel.governor().residency().unload_now(id).await;
    if session.kernel.governor().is_resident(id) {
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
/// shelf stops listing it.
async fn remove(session: &Session, id: &str) -> Result<String, String> {
    let shelf = session.shelf().await;
    let record = shelf
        .iter()
        .find(|record| record.id == id)
        .ok_or_else(|| "no longer on the shelf".to_owned())?;
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
