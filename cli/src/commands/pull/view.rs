//! How a pull's record reads on the terminal: the listing table, the cells it is
//! built from, and the JSON beside them.
//!
//! The JSON shape follows the command: one job is one object, several jobs are
//! an array of them, and only `resume`, which acts on many jobs and can refuse
//! some of them, wraps its two lists in an object.

use kernel::install::pulls::{PullJobDir, PullStatus};

use crate::support::pulls::{note, progress};
use crate::support::table;

const HEADERS: [&str; 5] = ["ID", "REFERENCE", "STATE", "PROGRESS", "NOTE"];

/// The listing: one row per job, oldest first, with a header.
pub(super) fn table(jobs: &[(PullJobDir, PullStatus)], now_ms: i64) -> String {
    let rows: Vec<Vec<String>> = jobs
        .iter()
        .map(|(job, status)| {
            vec![
                job.id().to_owned(),
                job.job().reference.clone(),
                status.state.to_string(),
                progress(status),
                note(job, status, now_ms),
            ]
        })
        .collect();
    table::render(&HEADERS, &rows)
}

/// What a client prints when it leaves a worker running.
pub(super) fn detached(job: &PullJobDir) -> String {
    let id = job.id();
    format!(
        "pulling {} in the background as {id}\n  watch:  hedos pull attach {id}\n  stop:   hedos pull cancel {id}",
        job.job().reference
    )
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
/// field added to both would silently lose the descriptor's copy.
pub(super) fn json(job: &PullJobDir, status: &PullStatus) -> serde_json::Value {
    let mut value = serde_json::to_value(job.job()).unwrap_or_default();
    if let (Some(object), Ok(serde_json::Value::Object(record))) =
        (value.as_object_mut(), serde_json::to_value(status))
    {
        object.extend(record);
    }
    value
}

/// Every job's record as an array, for the commands that act on many.
pub(super) fn json_list(jobs: &[(PullJobDir, PullStatus)]) -> serde_json::Value {
    serde_json::Value::Array(jobs.iter().map(|(job, status)| json(job, status)).collect())
}

#[cfg(test)]
mod tests;
