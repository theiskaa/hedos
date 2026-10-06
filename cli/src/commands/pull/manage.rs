//! The pulls already under way: what they are doing, and how to stop, restart,
//! or forget one.
//!
//! Nothing here talks to a worker directly. A stop is a control file the worker
//! reads; a resume is a fresh worker on the same job directory. When no worker
//! holds the job, the client settles the record itself, because there is nobody
//! left to hear the ask.

use std::time::Duration;

use kernel::install::pulls::{
    PullControl, PullEvent, PullJobDir, PullReading, PullStore, StopAnswer,
};
use kernel::time::now_millis;
use runtime::install::{
    STOP_ANSWER_WINDOW, Stopped, WorkerError, await_answer, restart, stop, sweep_claims,
};

use crate::error::CliError;
use crate::support::output::Out;
use crate::support::pulls::{event_line, too_late};

use super::attach::{self, Attached};
use super::view;
use super::{LogsArgs, ResumeArgs};

/// `hedos pull ls`.
pub(super) fn list(store: &PullStore, out: &Out) -> Result<(), CliError> {
    let now = now_millis();
    let jobs = records(store, now)?;
    if out.is_json() {
        out.json(&view::json_list(&jobs));
        return Ok(());
    }
    if jobs.is_empty() {
        out.line("no pulls yet. start one with `hedos pull <ref>`");
        return Ok(());
    }
    out.line(&view::table(&jobs, now));
    Ok(())
}

/// `hedos pull attach <job>`.
pub(super) async fn attach(store: &PullStore, query: &str, out: &Out) -> Result<(), CliError> {
    let job = store.resolve(query)?;
    let reading = job.reading(now_millis());
    if !reading.status.state.is_live() {
        return attach::report(out, &job, &reading);
    }
    match attach::follow(out, &job).await {
        Attached::Ended(reading) => attach::report(out, &job, &reading),
        Attached::Detached => {
            out.line(&view::detached(&job));
            out.json(&view::json(&job, &job.reading(now_millis())));
            Ok(())
        }
    }
}

/// `hedos pull pause <job>`.
///
/// Only a worker can pause a transfer, so this writes the ask rather than
/// stopping anything itself, then waits a moment for the worker to say what it
/// did. A job queued behind a busy slot still gets one: its worker reads the
/// control file before it takes that slot. A pull being registered gets none,
/// because that is the one stretch its worker spends not reading the file.
pub(super) async fn pause(store: &PullStore, query: &str, out: &Out) -> Result<(), CliError> {
    pause_within(store, query, out, STOP_ANSWER_WINDOW).await
}

/// [`pause`], waiting at most `within` for the worker's answer.
async fn pause_within(
    store: &PullStore,
    query: &str,
    out: &Out,
    within: Duration,
) -> Result<(), CliError> {
    let job = store.resolve(query)?;
    let reading = job.reading(now_millis());
    // Nothing will ever read an ask left for a worker that never arrived.
    if reading.abandoned {
        return Err(CliError::new(format!(
            "no worker took up {}. resume it, or cancel it",
            job.id()
        )));
    }
    if !reading.status.state.is_live() {
        return Err(CliError::new(format!(
            "{} is {}, not running",
            job.id(),
            reading.status.state
        )));
    }
    stop_and_report(&job, PullControl::Pause, out, within).await
}

/// `hedos pull cancel <job>`.
///
/// The ask is written whether or not a worker is there to read it, so one that
/// is still starting stops instead of transferring, and the worker is given a
/// moment to say what it did. With nothing holding the job, the record is
/// settled here too, because there is nobody left to settle it. A pull being
/// registered is refused, like a pause.
pub(super) async fn cancel(store: &PullStore, query: &str, out: &Out) -> Result<(), CliError> {
    cancel_within(store, query, out, STOP_ANSWER_WINDOW).await
}

/// [`cancel`], waiting at most `within` for the worker's answer.
async fn cancel_within(
    store: &PullStore,
    query: &str,
    out: &Out,
    within: Duration,
) -> Result<(), CliError> {
    let job = store.resolve(query)?;
    stop_and_report(&job, PullControl::Cancel, out, within).await
}

/// Ask `job` to stop the way `control` says and report what happened, not
/// only that it was asked.
///
/// A stop that came too late (the pull landed, or ended some other way, before
/// its worker read the ask) is the command's failure, so a script can tell it
/// from one that took. A worker that has not answered within `within` is not:
/// the ask stands, and is said to. `--json` says which of the three it was in
/// `outcome`: `honoured`, `pending`, or `too_late`.
async fn stop_and_report(
    job: &PullJobDir,
    control: PullControl,
    out: &Out,
    within: Duration,
) -> Result<(), CliError> {
    let id = job.id();
    let refused = |error: WorkerError| CliError::new(format!("{id}: {error}"));
    let answer = match stop(job, control).map_err(refused)? {
        Stopped::Settled(state) => Some(StopAnswer::Honoured(state)),
        Stopped::Asked(_) => await_answer(job, control, within).await.map_err(refused)?,
    };
    out.json(&view::with_outcome(
        view::json(job, &job.reading(now_millis())),
        outcome(answer),
    ));
    if let Some(line) = too_late(id, control, answer) {
        return Err(CliError::new(line));
    }
    match answer {
        Some(StopAnswer::Honoured(state)) => out.line(&format!("{state} {id}")),
        Some(StopAnswer::Landed | StopAnswer::Ended(_)) => {}
        None => {
            let asking = match control {
                PullControl::Pause => "pausing",
                PullControl::Cancel => "cancelling",
            };
            out.line(&format!("{asking} {id}; its worker has not answered yet"));
        }
    }
    Ok(())
}

/// The word `--json` gives for what became of a stop.
fn outcome(answer: Option<StopAnswer>) -> &'static str {
    match answer {
        Some(StopAnswer::Honoured(_)) => "honoured",
        Some(StopAnswer::Landed | StopAnswer::Ended(_)) => "too_late",
        None => "pending",
    }
}

/// `hedos pull resume [<job>|--all]`.
pub(super) fn resume(store: &PullStore, args: &ResumeArgs, out: &Out) -> Result<(), CliError> {
    let jobs = match (&args.job, args.all) {
        (Some(query), _) => vec![store.resolve(query)?],
        (None, true) => {
            // A pull it skips as pulled again stays skipped, rather than
            // coming back once the pull that superseded it is forgotten.
            let now = now_millis();
            store.settle_superseded(now);
            store.stopped(now)?
        }
        (None, false) => {
            return Err(CliError::new(
                "name a pull to resume, or pass --all to resume every stopped one",
            ));
        }
    };

    let mut started: Vec<(PullJobDir, PullReading)> = Vec::new();
    let mut refused: Vec<String> = Vec::new();
    for job in jobs {
        match restart(&job) {
            Ok(_) => {
                out.line(&format!("resuming {} ({})", job.id(), job.job().reference));
                let reading = job.reading(now_millis());
                started.push((job, reading));
            }
            // One job that cannot be resumed does not stop the others; every
            // reason is reported at the end.
            Err(error) => refused.push(format!("{}: {error}", job.id())),
        }
    }

    if started.is_empty() {
        return match refused.as_slice() {
            [] => Err(CliError::new("no stopped pulls to resume")),
            [only] => Err(CliError::new(only.clone())),
            many => Err(CliError::new(many.join("\n"))),
        };
    }
    for reason in &refused {
        out.err(reason);
    }
    out.json(&serde_json::json!({
        "resumed": view::json_list(&started),
        "refused": refused,
    }));
    Ok(())
}

/// `hedos pull logs <job>`.
pub(super) fn logs(store: &PullStore, args: &LogsArgs, out: &Out) -> Result<(), CliError> {
    let job = store.resolve(&args.job)?;
    let events = job.events();
    if events.is_empty() {
        out.line(&format!("{} has no history yet", job.id()));
        out.json(&serde_json::Value::Array(Vec::new()));
        return Ok(());
    }
    let shown = tail(&events, args.lines);
    if out.is_json() {
        out.json(&serde_json::to_value(shown).unwrap_or_default());
        return Ok(());
    }
    let now = now_millis();
    for event in shown {
        out.line(&event_line(event, now));
    }
    Ok(())
}

/// `hedos pull clean`.
///
/// Only the records go, and the claim files no worker holds. The weights a
/// pull fetched belong to the model store, and a half-downloaded file belongs
/// to whatever will resume it; `pull.partial_age_hours` is what eventually
/// collects those.
pub(super) fn clean(store: &PullStore, keep: usize, out: &Out) -> Result<(), CliError> {
    sweep_claims(store.root());
    let removed = store.sweep(keep, now_millis());
    out.line(&match removed {
        1 => "removed 1 ended pull".to_owned(),
        count => format!("removed {count} ended pulls"),
    });
    out.json(&serde_json::json!({ "removed": removed }));
    Ok(())
}

/// The last `lines` of `events`, or all of them when no count was asked for.
fn tail(events: &[PullEvent], lines: Option<usize>) -> &[PullEvent] {
    match lines {
        Some(lines) => &events[events.len().saturating_sub(lines)..],
        None => events,
    }
}

/// Every job with the way it reads at `now_ms`.
fn records(store: &PullStore, now_ms: i64) -> Result<Vec<(PullJobDir, PullReading)>, CliError> {
    Ok(store
        .jobs()?
        .into_iter()
        .map(|job| {
            let reading = job.reading(now_ms);
            (job, reading)
        })
        .collect())
}

#[cfg(test)]
mod tests;
