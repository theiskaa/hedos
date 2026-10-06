//! Following a pull from outside the process running it.
//!
//! There is no channel to subscribe to: the worker writes its record and a
//! client reads it. Attaching is therefore a poll, and detaching costs nothing
//! because the reader was never what kept the download alive.

use std::time::Duration;

use kernel::install::pulls::{PullJobDir, PullReading, PullState, PullStatus};
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
    /// The pull stopped, reading this way.
    Ended(PullReading),
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
        let reading = job.reading(now);
        // A job nothing ever took up reads interrupted rather than queued, since
        // waiting on it would look exactly like waiting for a free slot.
        if !reading.status.state.is_live() {
            download.finish();
            return Attached::Ended(reading);
        }
        show(&mut download, &reading.status, now);
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
fn show(download: &mut Download, status: &PullStatus, now_ms: i64) {
    match status.state {
        PullState::Running => {
            download.progress(&status.progress);
            if status.progress.current_file.is_none()
                && let Some(line) = &status.status_line
            {
                download.status(line);
            }
        }
        _ => download.status(&waiting(status, now_ms)),
    }
}

/// What a queued pull some worker is coming for is waiting for.
fn waiting(status: &PullStatus, now_ms: i64) -> String {
    let note = pulls::note(status, false, now_ms);
    match note.is_empty() {
        true => "queued".to_owned(),
        false => format!("queued · {note}"),
    }
}

/// Say how the pull ended. A pull that did not happen is the command's failure;
/// one the user stopped is not.
pub(super) fn report(out: &Out, job: &PullJobDir, reading: &PullReading) -> Result<(), CliError> {
    let reference = &job.job().reference;
    let status = &reading.status;
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
        PullState::Paused => out.err(&view::resumable(job, status)),
        PullState::Interrupted if reading.abandoned => return never_taken_up(out, job, reading),
        // Nobody chose this one: the worker went away, so the model was not
        // fetched, and a script that chains onto this command must not carry on
        // to run weights that are not there.
        PullState::Interrupted => {
            out.json(&view::json(job, reading));
            return Err(CliError::new(view::resumable(job, status)));
        }
        // A failure and a job no worker ever took up both mean the model was not
        // fetched, which a script has to be able to tell from a pull that landed.
        PullState::Failed => {
            out.json(&view::json(job, reading));
            return Err(CliError::new(
                status
                    .message
                    .clone()
                    .unwrap_or_else(|| format!("pulling {reference} failed")),
            ));
        }
        PullState::Queued | PullState::Running => return never_taken_up(out, job, reading),
        PullState::Unreadable => {
            out.json(&view::json(job, reading));
            return Err(CliError::new(format!(
                "{}'s record could not be read",
                job.id()
            )));
        }
    }
    out.json(&view::json(job, reading));
    Ok(())
}

/// The failure of a pull no worker ever took up.
fn never_taken_up(out: &Out, job: &PullJobDir, reading: &PullReading) -> Result<(), CliError> {
    out.json(&view::json(job, reading));
    let id = job.id();
    Err(CliError::new(format!(
        "no worker took up {id}. start it again with `hedos pull resume {id}`"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    use kernel::install::pulls::START_GRACE_MS;

    use crate::support::pulls::testing::{TempDir, job as make_job, shown, status, unreadable_job};

    fn ended(state: PullState) -> PullReading {
        shown(status(state))
    }

    #[test]
    fn a_pull_nobody_stopped_is_the_commands_failure() {
        let directory = TempDir::new("attach-interrupted");
        let store = directory.store();
        let job = make_job(&store, "Qwen/Qwen3-8B", 1_000);
        let out = Out::new(false);

        // The worker went away, so the model was not fetched and a script that
        // chains onto this must not carry on.
        let error =
            report(&out, &job, &ended(PullState::Interrupted)).expect_err("the model is not there");
        assert!(error.message.contains("resume with"), "{error:?}");

        // The user asking for it is a different thing, and stays a success.
        report(&out, &job, &ended(PullState::Paused)).expect("the user chose this");
        report(&out, &job, &ended(PullState::Cancelled)).expect("so did they choose this");
        report(&out, &job, &ended(PullState::Done)).expect("and this is the point");
    }

    #[test]
    fn a_pull_no_worker_took_up_says_so_rather_than_offering_a_resume_of_nothing() {
        let directory = TempDir::new("attach-abandoned");
        let job = make_job(&directory.store(), "Qwen/Qwen3-8B", 1_000);
        let out = Out::new(false);

        let error = report(&out, &job, &job.reading(1_000 + START_GRACE_MS))
            .expect_err("the model was never fetched");

        assert!(error.message.starts_with("no worker took up"), "{error:?}");
    }

    #[test]
    fn a_pull_whose_record_cannot_be_read_is_the_commands_failure() {
        let directory = TempDir::new("attach-unreadable");
        let job = unreadable_job(&directory.store(), "Qwen/Qwen3-8B", 1_000);
        let out = Out::new(false);

        let error = report(&out, &job, &job.reading(now_millis()))
            .expect_err("nothing says the model was fetched");

        assert_eq!(
            error.message,
            format!("{}'s record could not be read", job.id())
        );
    }
}
