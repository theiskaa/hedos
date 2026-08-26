//! Background work: each task runs on the tokio runtime and reports over the
//! event channel, so the loop never blocks on the kernel. This is the only
//! module in the UI that awaits kernel calls.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::json;
use tokio::sync::mpsc;

use super::event::{Event, Refreshed};
use super::facts::Facts;
use crate::commands::warm::{is_resident, residency_outcome, warm_request};
use crate::support::session::Session;

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
}

impl TaskKind {
    /// The verb the strip shows.
    pub fn verb(&self) -> &'static str {
        match self {
            TaskKind::Scan => "scan",
            TaskKind::Warm { .. } | TaskKind::WarmViaGateway { .. } => "warm",
            TaskKind::Unload { .. } => "unload",
        }
    }

    /// The subject the strip shows after the verb.
    pub fn subject(&self) -> &str {
        match self {
            TaskKind::Scan => "this machine",
            TaskKind::Warm { name, .. }
            | TaskKind::WarmViaGateway { name, .. }
            | TaskKind::Unload { name, .. } => name,
        }
    }

    /// The model this task concerns, if it concerns one.
    pub fn model_id(&self) -> Option<&str> {
        match self {
            TaskKind::Scan => None,
            TaskKind::Warm { id, .. }
            | TaskKind::WarmViaGateway { id, .. }
            | TaskKind::Unload { id, .. } => Some(id),
        }
    }

    /// What the strip says while the task runs.
    pub fn activity(&self) -> &'static str {
        match self {
            TaskKind::Scan => "looking through the stores",
            TaskKind::Warm { .. } => "loading in this process",
            TaskKind::WarmViaGateway { .. } => "loading on the gateway",
            TaskKind::Unload { .. } => "evicting",
        }
    }
}

/// Where a task is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskState {
    /// Still going.
    Running,
    /// Finished, with a one-line result.
    Done(String),
    /// Gave up, with the reason.
    Failed(String),
}

/// A task's progress, as sent over the channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskEvent {
    pub id: TaskId,
    pub state: TaskState,
}

/// Start `kind` on the runtime, reporting to `tx`; the returned id names it in
/// the strip. Every task ends with a `Done` or `Failed` event.
pub fn spawn(kind: &TaskKind, session: &Arc<Session>, tx: mpsc::UnboundedSender<Event>) -> TaskId {
    let id = TaskId::next();
    let session = Arc::clone(session);
    let kind = kind.clone();
    tokio::spawn(async move {
        let outcome = match &kind {
            TaskKind::Scan => scan(&session).await,
            TaskKind::Warm { id, .. } => warm(&session, id).await,
            TaskKind::WarmViaGateway { id, port, .. } => warm_via_gateway(id, *port).await,
            TaskKind::Unload { id, .. } => unload(&session, id).await,
        };
        let state = match outcome {
            Ok(summary) => TaskState::Done(summary),
            Err(reason) => TaskState::Failed(reason),
        };
        let _ = tx.send(Event::Task(TaskEvent { id, state }));
    });
    id
}

/// Re-read the shelf and the machine facts, reporting them as one event
/// stamped with a sequence so a slow older refresh never overwrites a newer.
pub fn spawn_refresh(session: &Arc<Session>, tx: mpsc::UnboundedSender<Event>) {
    let sequence = NEXT_REFRESH.fetch_add(1, Ordering::Relaxed);
    let session = Arc::clone(session);
    tokio::spawn(async move {
        let records = session.shelf().await.to_vec();
        let facts = Facts::collect(&session, &records).await;
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
