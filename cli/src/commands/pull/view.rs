//! How a pull's record reads on the terminal: the listing table, the cells it is
//! built from, and the JSON beside them.
//!
//! The JSON shape follows the command: one job is one object, several jobs are
//! an array of them, and only `resume`, which acts on many jobs and can refuse
//! some of them, wraps its two lists in an object.

use kernel::install::pulls::{PullJobDir, PullReading, PullStatus};

use crate::support::pulls::{note, progress};
use crate::support::table;

const HEADERS: [&str; 5] = ["ID", "REFERENCE", "STATE", "PROGRESS", "NOTE"];

/// The listing: one row per job, oldest first, with a header.
pub(super) fn table(jobs: &[(PullJobDir, PullReading)], now_ms: i64) -> String {
    let rows: Vec<Vec<String>> = jobs
        .iter()
        .map(|(job, reading)| {
            vec![
                job.id().to_owned(),
                job.job().reference.clone(),
                reading.status.state.to_string(),
                progress(&reading.status),
                note(&reading.status, reading.abandoned, now_ms),
            ]
        })
        .collect();
    table::render(&HEADERS, &rows)
}

/// What a client prints when it leaves a worker running.
pub(super) fn detached(job: &PullJobDir) -> String {
    format!(
        "pulling {} in the background as {}\n{}",
        job.job().reference,
        job.id(),
        reach(job)
    )
}

/// The commands that reach a pull left in the background, for a notice that
/// has already said which pull.
pub(super) fn reach(job: &PullJobDir) -> String {
    let id = job.id();
    format!("  watch:  hedos pull attach {id}\n  stop:   hedos pull cancel {id}")
}

/// The line a pull that stopped but could go on leaves behind, naming what
/// starts it again.
pub(super) fn resumable(job: &PullJobDir, status: &PullStatus) -> String {
    let why = status
        .message
        .clone()
        .map(|message| format!(": {message}"))
        .unwrap_or_default();
    format!(
        "{}{why}. resume with `hedos pull resume {}`",
        status.state,
        job.id()
    )
}

/// A job's descriptor and its live record as one object, so `--json` says
/// everything the table shows and everything it leaves out.
///
/// The two are merged rather than nested because no field name is shared; a
/// field added to both would silently lose the descriptor's copy. `state` is
/// the state as shown, `"abandoned": true` marks a job that reads
/// `interrupted` because no worker ever took it up, and `"superseded": true`
/// one that reads `failed` because no worker took it up and its model was
/// pulled since.
pub(super) fn json(job: &PullJobDir, reading: &PullReading) -> serde_json::Value {
    let mut value = serde_json::to_value(job.job()).unwrap_or_default();
    if let (Some(object), Ok(serde_json::Value::Object(record))) =
        (value.as_object_mut(), serde_json::to_value(&reading.status))
    {
        object.extend(record);
        if reading.abandoned {
            object.insert("abandoned".to_owned(), serde_json::Value::Bool(true));
        }
        if reading.superseded {
            object.insert("superseded".to_owned(), serde_json::Value::Bool(true));
        }
    }
    value
}

/// A job's object with `"outcome"` added: what became of a pause or a cancel
/// asked of it.
pub(super) fn with_outcome(mut value: serde_json::Value, outcome: &str) -> serde_json::Value {
    if let Some(object) = value.as_object_mut() {
        object.insert(
            "outcome".to_owned(),
            serde_json::Value::String(outcome.to_owned()),
        );
    }
    value
}

/// Every job's record as an array, for the commands that act on many.
pub(super) fn json_list(jobs: &[(PullJobDir, PullReading)]) -> serde_json::Value {
    serde_json::Value::Array(
        jobs.iter()
            .map(|(job, reading)| json(job, reading))
            .collect(),
    )
}

#[cfg(test)]
mod tests;
