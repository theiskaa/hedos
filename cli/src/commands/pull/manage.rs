//! The pulls already under way: what they are doing, and how to stop, restart,
//! or forget one.
//!
//! Nothing here talks to a worker directly. A stop is a control file the worker
//! reads; a resume is a fresh worker on the same job directory. When no worker
//! holds the job, the client settles the record itself, because there is nobody
//! left to hear the ask.

use kernel::install::pulls::{
    PullControl, PullEvent, PullJobDir, PullStatus, PullStore, START_GRACE_MS,
};
use kernel::time::now_millis;
use runtime::install::{restart, stop};

use crate::error::CliError;
use crate::support::output::Out;

use super::attach::{self, Attached};
use super::view;
use super::{CleanArgs, LogsArgs, ResumeArgs};

/// `hedos pull ls`.
pub(super) fn list(store: &PullStore, out: &Out) -> Result<(), CliError> {
    let jobs = records(store)?;
    if out.is_json() {
        out.json(&view::json_list(&jobs));
        return Ok(());
    }
    if jobs.is_empty() {
        out.line("no pulls yet. start one with `hedos pull <ref>`");
        return Ok(());
    }
    out.line(&view::table(&jobs, now_millis()));
    Ok(())
}

/// `hedos pull attach <job>`.
pub(super) async fn attach(store: &PullStore, query: &str, out: &Out) -> Result<(), CliError> {
    let job = store.resolve(query)?;
    let status = job.status();
    if !status.state.is_live() {
        return attach::report(out, &job, &status);
    }
    match attach::follow(out, &job).await {
        Attached::Ended(status) => attach::report(out, &job, &status),
        Attached::Detached => {
            out.line(&view::detached(&job));
            out.json(&view::json(&job, &job.status()));
            Ok(())
        }
    }
}

/// `hedos pull pause <job>`.
///
/// Only a worker can pause a transfer, so this only ever writes the ask. A job
/// queued behind a busy slot still gets one: its worker reads the control file
/// before it takes that slot.
pub(super) fn pause(store: &PullStore, query: &str, out: &Out) -> Result<(), CliError> {
    let job = store.resolve(query)?;
    let status = job.status();
    if !status.state.is_live() {
        return Err(CliError::new(format!(
            "{} is {}, not running",
            job.id(),
            status.state
        )));
    }
    // Nothing will ever read an ask left for a worker that never arrived.
    if job.abandoned(now_millis(), START_GRACE_MS) {
        return Err(CliError::new(format!(
            "no worker took up {}. resume it, or cancel it",
            job.id()
        )));
    }
    let stopped = stop(&job, PullControl::Pause)
        .map_err(|error| CliError::new(format!("{}: {error}", job.id())))?;
    out.line(&match stopped.settled() {
        true => format!("paused {}", job.id()),
        false => format!("pausing {}", job.id()),
    });
    out.json(&view::json(&job, &job.status()));
    Ok(())
}

/// `hedos pull cancel <job>`.
///
/// The ask is written whatever the state, so a worker that is still starting
/// stops instead of transferring. With nothing holding the job, the record is
/// settled here too, because there is nobody left to settle it.
pub(super) fn cancel(store: &PullStore, query: &str, out: &Out) -> Result<(), CliError> {
    let job = store.resolve(query)?;
    let stopped = stop(&job, PullControl::Cancel)
        .map_err(|error| CliError::new(format!("{}: {error}", job.id())))?;
    out.line(&match stopped.settled() {
        true => format!("cancelled {}", job.id()),
        false => format!("cancelling {}", job.id()),
    });
    out.json(&view::json(&job, &job.status()));
    Ok(())
}

/// `hedos pull resume [<job>|--all]`.
pub(super) fn resume(store: &PullStore, args: &ResumeArgs, out: &Out) -> Result<(), CliError> {
    let jobs = match (&args.job, args.all) {
        (Some(query), _) => vec![store.resolve(query)?],
        (None, true) => records(store)?
            .into_iter()
            .filter(|(_, status)| status.state.is_resumable())
            .map(|(job, _)| job)
            .collect(),
        (None, false) => {
            return Err(CliError::new(
                "name a pull to resume, or pass --all to resume every stopped one",
            ));
        }
    };

    let mut started: Vec<(PullJobDir, PullStatus)> = Vec::new();
    let mut refused: Vec<String> = Vec::new();
    for job in jobs {
        match restart(&job) {
            Ok(_) => {
                out.line(&format!("resuming {} ({})", job.id(), job.job().reference));
                let status = job.status();
                started.push((job, status));
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
        out.line(&view::event_line(event, now));
    }
    Ok(())
}

/// `hedos pull clean`.
///
/// Only the records go. The weights a pull fetched belong to the model store,
/// and a half-downloaded file belongs to whatever will resume it; the
/// `pull.partial_age` setting is what eventually collects those.
pub(super) fn clean(store: &PullStore, args: &CleanArgs, out: &Out) -> Result<(), CliError> {
    let removed = store.sweep(args.keep, now_millis());
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

/// Every job with the record it reads as right now.
fn records(store: &PullStore) -> Result<Vec<(PullJobDir, PullStatus)>, CliError> {
    Ok(store
        .jobs()?
        .into_iter()
        .map(|job| {
            let status = job.status();
            (job, status)
        })
        .collect())
}

#[cfg(test)]
mod tests;
