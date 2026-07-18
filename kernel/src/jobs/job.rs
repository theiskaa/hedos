//! The `Job` record and its state/progress value types.

use serde::{Deserialize, Serialize};

use crate::records::{Capability, JsonValue};

/// A job's lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobState {
    Queued,
    Preparing,
    Running,
    Done,
    Failed,
    Cancelled,
}

impl JobState {
    /// Whether this is an end state (`Done`/`Failed`/`Cancelled`).
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            JobState::Done | JobState::Failed | JobState::Cancelled
        )
    }
}

/// A job's progress: a `[0, 1]` fraction plus the raw step counts it came from.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct JobProgress {
    pub fraction: f64,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub step: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub total_steps: Option<i64>,
}

impl JobProgress {
    /// Progress at `fraction` derived from `step`/`total_steps`.
    pub fn new(fraction: f64, step: Option<i64>, total_steps: Option<i64>) -> Self {
        Self {
            fraction,
            step,
            total_steps,
        }
    }
}

impl Default for JobProgress {
    fn default() -> Self {
        Self {
            fraction: 0.0,
            step: None,
            total_steps: None,
        }
    }
}

/// A discrete unit of work with progress and a persisted terminal result. Its
/// on-disk form (in `jobs.json`) is internal, so field names are the Rust-native
/// snake_case, not the Swift app's keys.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub model_id: String,
    pub capability: Capability,
    pub payload: JsonValue,
    pub state: JobState,
    pub progress: JobProgress,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub queue_reason: Option<String>,
    /// The latest preview frame. Held in memory only — never persisted.
    #[serde(skip)]
    pub preview: Option<Vec<u8>>,
    #[serde(default)]
    pub result: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub error: Option<String>,
    pub submitted_at: i64,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub started_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub finished_at: Option<i64>,
}

impl Job {
    /// A freshly-queued job submitted at `submitted_at` (epoch milliseconds).
    pub fn new(
        id: impl Into<String>,
        model_id: impl Into<String>,
        capability: Capability,
        payload: JsonValue,
        submitted_at: i64,
    ) -> Self {
        Self {
            id: id.into(),
            model_id: model_id.into(),
            capability,
            payload,
            state: JobState::Queued,
            progress: JobProgress::default(),
            queue_reason: None,
            preview: None,
            result: Vec::new(),
            error: None,
            submitted_at,
            started_at: None,
            finished_at: None,
        }
    }
}
