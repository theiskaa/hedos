//! Following a pull from outside the process running it.
//!
//! There is no channel to subscribe to: the worker writes its record and a
//! client reads it. Attaching is therefore a poll, and detaching costs nothing
//! because the reader was never what kept the download alive.

use std::time::Duration;

use kernel::install::pulls::{PullJobDir, PullState, PullStatus, START_GRACE_MS};
use kernel::time::now_millis;

use crate::error::CliError;
use crate::support::download::Download;
use crate::support::output::Out;
use crate::support::pulls;
use crate::support::signals;

use super::view;

/// How often the record is re-read. Twice the rate the worker writes at, so a
/// new figure is on screen about as soon as it exists.
const POLL: Duration = Duration::from_millis(250);

/// How an attach ended.
pub(super) enum Attached {
    /// The pull stopped, in this state.
    Ended(PullStatus),
    /// The user let go; the worker was left running.
    Detached,
}

/// Follow `job` until it stops running, or until Ctrl-C detaches from it.
pub(super) async fn follow(out: &Out, job: &PullJobDir) -> Attached {
    let mut download = Download::start(out);
    // One interrupt future for the whole attach, rather than a fresh one each
    // time round: a Ctrl-C pressed between two polls would fall into the gap
    // before a newly made one had registered for it.
    let interrupt = signals::wait_for_ctrl_c();
    tokio::pin!(interrupt);
    loop {
        let now = now_millis();
        let status = job.status();
        // A job nothing ever took up will sit queued for good, and waiting on it
        // would look exactly like waiting for a free slot.
        if !status.state.is_live() || job.abandoned(now, START_GRACE_MS) {
            download.finish();
            return Attached::Ended(status);
        }
        show(&mut download, &status, job, now);
        tokio::select! {
            () = tokio::time::sleep(POLL) => {}
            () = &mut interrupt => {
                download.finish();
                return Attached::Detached;
            }
        }
    }
}

/// Put the record on the indicator: the bar while bytes move, the reason for the
/// wait while they do not.
fn show(download: &mut Download, status: &PullStatus, job: &PullJobDir, now_ms: i64) {
    match status.state {
        PullState::Running => {
            download.progress(&status.progress);
            if status.progress.current_file.is_none()
                && let Some(line) = &status.status_line
            {
                download.status(line);
            }
        }
        _ => download.status(&waiting(job, status, now_ms)),
    }
}

/// What a queued pull is waiting for.
fn waiting(job: &PullJobDir, status: &PullStatus, now_ms: i64) -> String {
    let note = pulls::note(job, status, now_ms);
    match note.is_empty() {
        true => "queued".to_owned(),
        false => format!("queued · {note}"),
    }
}

/// Say how the pull ended. A pull that did not happen is the command's failure;
/// one the user stopped is not.
pub(super) fn report(out: &Out, job: &PullJobDir, status: &PullStatus) -> Result<(), CliError> {
    let reference = &job.job().reference;
    match status.state {
        PullState::Done => {
            out.line(&format!("pulled {reference}"));
            // A registration that failed is the one thing a landed pull still
            // has to say: the weights are on disk, the shelf may not know it.
            if let Some(message) = &status.message {
                out.err(message);
            }
        }
        PullState::Cancelled => out.err("cancelled"),
        PullState::Paused | PullState::Interrupted => out.err(&view::resumable(job, status)),
        // A failure and a job no worker ever took up both mean the model was not
        // fetched, which a script has to be able to tell from a pull that landed.
        PullState::Failed => {
            out.json(&view::json(job, status));
            return Err(CliError::new(
                status
                    .message
                    .clone()
                    .unwrap_or_else(|| format!("pulling {reference} failed")),
            ));
        }
        PullState::Queued | PullState::Running => {
            out.json(&view::json(job, status));
            let id = job.id();
            return Err(CliError::new(format!(
                "no worker took up {id}. start it again with `hedos pull resume {id}`"
            )));
        }
    }
    out.json(&view::json(job, status));
    Ok(())
}
