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
use super::text;
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

/// Every job in the store, oldest first; a store that was never made is
/// empty. A store that cannot be read is `None`, so a poll that saw nothing
/// is told apart from one that could not look.
pub fn poll(store: &PullStore, now_ms: i64) -> Option<Vec<JobRow>> {
    let jobs = store.jobs().ok()?;
    Some(rows_of(jobs, now_ms))
}

/// [`poll`] with a store that cannot be read reading as empty.
#[cfg(test)]
pub fn rows(store: &PullStore, now_ms: i64) -> Vec<JobRow> {
    poll(store, now_ms).unwrap_or_default()
}

fn rows_of(jobs: Vec<kernel::install::pulls::PullJobDir>, now_ms: i64) -> Vec<JobRow> {
    jobs.into_iter()
        .map(|job| {
            let status = job.status();
            // A job queued with nobody coming for it is stopped, whatever the
            // record says: the kernel already refuses to join one, and a strip
            // that called it live would never let the model be pulled again.
            let abandoned = job.abandoned_by(&status, now_ms, START_GRACE_MS);
            let pull_state = match abandoned {
                true => PullState::Interrupted,
                false => status.state,
            };
            let aged_out = pull_state.is_terminal()
                && now_ms.saturating_sub(status.updated_at_ms) >= ENDED_LINGER_MS;
            // The whole note: the painter cuts it to the row it has, where
            // `ls` cuts it to its column.
            let note = pulls::full_note(&status, abandoned, now_ms);
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
    let every_byte_landed = status.progress.fraction() == Some(1.0);
    match pull_state {
        // A bar at the full width says nothing more; what the worker is
        // doing with the bytes now does, and that is its own line rather
        // than the note, which would say "attempt 2" over it.
        PullState::Running if every_byte_landed => TaskState::Status(
            status
                .status_line
                .clone()
                .unwrap_or_else(|| "finishing".to_owned()),
        ),
        PullState::Running if status.progress.bytes_downloaded > 0 => {
            TaskState::Downloading(status.progress.clone())
        }
        PullState::Running | PullState::Queued => TaskState::Status(said(note, "queued")),
        PullState::Done => TaskState::Done(format!("pulled {reference}")),
        PullState::Cancelled => TaskState::Done("cancelled".to_owned()),
        // The row names the model already; a reason that opens with it
        // would name it twice.
        PullState::Failed => TaskState::Failed(said(without_subject(note, reference), "failed")),
        // What is on disk is why the row is worth going on from.
        PullState::Paused | PullState::Interrupted => {
            let mut how = pull_state.to_string();
            if let Some(figures) = text::landed(&status.progress) {
                how.push_str(" · ");
                how.push_str(&figures);
            }
            if !note.is_empty() {
                how.push_str(", ");
                how.push_str(&note);
            }
            TaskState::Stopped(how)
        }
    }
}

/// `note` without `reference` at its head: the words after it, or the note
/// as it was when it did not begin with the reference as a whole word.
fn without_subject(note: String, reference: &str) -> String {
    let Some(rest) = note.strip_prefix(reference) else {
        return note;
    };
    let joins = |c: char| c.is_whitespace() || matches!(c, ':' | ',' | ';');
    match rest.chars().next() {
        None => String::new(),
        Some(c) if joins(c) => rest.trim_start_matches(joins).to_owned(),
        Some(_) => note,
    }
}

/// `note`, or `fallback` when there is none.
fn said(note: String, fallback: &str) -> String {
    match note.is_empty() {
        true => fallback.to_owned(),
        false => note,
    }
}

#[cfg(test)]
mod tests;
