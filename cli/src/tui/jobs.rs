//! The pull jobs the strip shows, read from the job directory rather than run
//! here. Named for the record they come from, to keep them apart from the pull
//! modal, which is about starting one.
//!
//! A download belongs to a worker process, not to this one, so the screen has
//! no channel to subscribe to and nothing to wait for on the way out. It reads
//! the same records `hedos pull ls` reads, which is why a pull started in a
//! terminal appears here, and why closing the screen leaves it running.

use kernel::install::pulls::{PullState, PullStatus, PullStore, START_GRACE_MS};

use super::strip::ENDED_LINGER_MS;
use super::tasks::TaskState;
use crate::support::pulls;

/// A pull as the strip needs it: which job to act on, what to call it, and
/// where it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobRow {
    /// The job id, which is what a stop or a resume is addressed to.
    pub job: String,
    /// The model being fetched.
    pub reference: String,
    /// Where the pull is, in the strip's own vocabulary.
    pub state: TaskState,
    /// Where the pull is in the record's own vocabulary, which is finer than
    /// the strip's.
    pub pull_state: PullState,
}

/// Every pull worth showing, oldest first.
///
/// A pull that ended long ago is left out rather than shown and then expired:
/// its record stays in the store until someone runs `hedos pull clean`, so a
/// strip that took every ended job would fill with last week's downloads and
/// put back every row it expired on the very next poll.
pub fn rows(store: &PullStore, now_ms: i64) -> Vec<JobRow> {
    // A store that cannot be read at all leaves the strip as it was. The poll
    // runs twice a second, so the alternative is the same unactionable notice
    // over and over on top of whatever the user was reading.
    store
        .jobs()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|job| {
            let status = job.status();
            // A job queued with nobody coming for it is stopped, whatever the
            // record says: the kernel already refuses to join one, and a strip
            // that called it live would never let the model be pulled again.
            let pull_state = match job.abandoned(now_ms, START_GRACE_MS) {
                true => PullState::Interrupted,
                false => status.state,
            };
            if pull_state.is_terminal()
                && now_ms.saturating_sub(status.updated_at_ms) >= ENDED_LINGER_MS
            {
                return None;
            }
            let note = pulls::note(&job, &status, now_ms);
            Some(JobRow {
                job: job.id().to_owned(),
                reference: job.job().reference.clone(),
                state: state(pull_state, &status, &job.job().reference, note),
                pull_state,
            })
        })
        .collect()
}

/// A record as the strip's own vocabulary: a bar while bytes move, a line while
/// they do not, and one of three endings.
///
/// `Paused` and `Interrupted` both become `Stopped`, because the strip offers
/// the same key for both: whoever stopped it, what is on disk is worth going on
/// from. `Cancelled` is an ending the user chose, so it reads as done rather
/// than as a failure.
fn state(pull_state: PullState, status: &PullStatus, reference: &str, note: String) -> TaskState {
    match pull_state {
        PullState::Running if status.progress.bytes_downloaded > 0 => {
            TaskState::Downloading(status.progress.clone())
        }
        PullState::Running | PullState::Queued => TaskState::Status(match note.is_empty() {
            true => "queued".to_owned(),
            false => note,
        }),
        PullState::Done => TaskState::Done(format!("pulled {reference}")),
        PullState::Cancelled => TaskState::Done("cancelled".to_owned()),
        PullState::Failed => TaskState::Failed(match note.is_empty() {
            true => "failed".to_owned(),
            false => note,
        }),
        PullState::Paused | PullState::Interrupted => TaskState::Stopped(match note.is_empty() {
            true => pull_state.to_string(),
            false => format!("{pull_state}, {note}"),
        }),
    }
}

#[cfg(test)]
mod tests;
