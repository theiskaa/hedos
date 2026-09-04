//! The pull jobs as the screen reads them from the job directory, rather than
//! run here. Named for the record they come from, to keep them apart from the
//! pull modal, which is about starting one.
//!
//! A download belongs to a worker process, not to this one, so the screen has
//! no channel to subscribe to and nothing to wait for on the way out. It reads
//! the same records `hedos pull ls` reads, which is why a pull started in a
//! terminal appears here, and why closing the screen leaves it running. One
//! poll feeds two surfaces: the task strip, which wants only the newest work,
//! and the pulls screen, which wants every job the store still holds.

use kernel::install::pulls::{PullJob, PullState, PullStatus, PullStore, START_GRACE_MS};

use super::strip::ENDED_LINGER_MS;
use super::tasks::TaskState;
use crate::support::clock;
use crate::support::pulls;

/// A pull as the screen needs it: which job to act on, what to call it, where
/// it is, and the record behind that for the surface that shows all of it.
#[derive(Debug, Clone, PartialEq)]
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
    /// The live record: what has landed, which attempt, why it stopped.
    pub status: PullStatus,
    /// What was asked for, written once when the job was created.
    pub descriptor: PullJob,
    /// The one thing worth saying beside the state, as `hedos pull ls` says it.
    pub note: String,
    /// How long ago the job was created, and how long ago its record last
    /// moved. Read at poll time, like the note, so the screen keeps no clock.
    pub started_ago: String,
    pub updated_ago: String,
    /// When the poll read the record, epoch milliseconds: the one clock the
    /// screen has for telling a transfer that stalled from one still moving.
    pub polled_at_ms: i64,
    /// Whether the pull ended long enough ago that the strip leaves it out;
    /// the pulls screen shows it until `hedos pull clean` takes it.
    pub aged_out: bool,
}

/// Every job in the store, oldest first.
///
/// A store that cannot be read at all reads as empty. The poll runs twice a
/// second, so the alternative is the same unactionable notice over and over on
/// top of whatever the user was reading.
pub fn rows(store: &PullStore, now_ms: i64) -> Vec<JobRow> {
    store
        .jobs()
        .unwrap_or_default()
        .into_iter()
        .map(|job| {
            let status = job.status();
            // A job queued with nobody coming for it is stopped, whatever the
            // record says: the kernel already refuses to join one, and a strip
            // that called it live would never let the model be pulled again.
            let pull_state = match job.abandoned(now_ms, START_GRACE_MS) {
                true => PullState::Interrupted,
                false => status.state,
            };
            let aged_out = pull_state.is_terminal()
                && now_ms.saturating_sub(status.updated_at_ms) >= ENDED_LINGER_MS;
            let note = pulls::note(&job, &status, now_ms);
            let descriptor = job.job().clone();
            JobRow {
                job: job.id().to_owned(),
                reference: descriptor.reference.clone(),
                state: state(pull_state, &status, &descriptor.reference, note.clone()),
                pull_state,
                started_ago: clock::millis(now_ms.saturating_sub(descriptor.created_at_ms)),
                updated_ago: clock::millis(now_ms.saturating_sub(status.updated_at_ms)),
                status,
                descriptor,
                note,
                polled_at_ms: now_ms,
                aged_out,
            }
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
