//! Starting a pull: resolve what to fetch, hand it to a worker of its own, and
//! then either watch that worker or walk away from it.
//!
//! Asking for a model that is already being pulled joins the pull that exists
//! instead of starting a second one onto the same half-written files. The
//! lookup happens twice, before and after the plan is resolved, because a
//! provider rewrites what the user typed: `gemma3` becomes `gemma3:latest` and a
//! hub URL becomes a repo. Only the second lookup can match a job created from a
//! rewritten reference, and only the first works with the network down.
//!
//! The plan is resolved here as well as in the worker. The descriptor needs a
//! display name, a destination, and a size to show before a byte moves, and by
//! the time the worker runs the plan's remaining bytes are stale anyway.

use kernel::install::pulls::PullJobDir;
use kernel::install::{InstallError, InstallPlan};
use kernel::time::now_millis;
use runtime::boot::{self, HedosDirs};
use runtime::install::{Started, WorkerError, collect_ended, restart, start_or_join};
use runtime::settings::SettingsStore;

use crate::error::CliError;
use crate::support::interactive;
use crate::support::output::Out;

use super::attach::{self, Attached};
use super::{PullArgs, pick, view};

/// Run `hedos pull [reference]`.
pub(super) async fn run(args: &PullArgs, out: &Out) -> Result<(), CliError> {
    // No kernel is opened for a reference given on the command line: the worker
    // is what registers the model, and only the interactive picker needs a shelf.
    let settings = SettingsStore::discover().load();
    let dirs = HedosDirs::detect();
    let install = boot::install_service(&settings);
    let (provider, reference) = pick::target(out, &install, args).await?;
    let store = boot::pull_store(&dirs);
    collect_ended(&store, &settings.pull);

    if let Some(job) = store.under_way(&provider, &reference, now_millis())
        && rejoin(out, &job, args.detach).await?
    {
        return Ok(());
    }

    let plan = install.plan(&provider, &reference).await?;
    if plan.requires_auth {
        // One canonical voice for a gated repo: the same guidance the download
        // path surfaces, rather than a second, thinner message here.
        return Err(InstallError::AuthRequired(reference).into());
    }
    if let Some(job) = store.under_way(&provider, &plan.reference, now_millis())
        && rejoin(out, &job, args.detach).await?
    {
        return Ok(());
    }
    if interactive::is_interactive(out) && !confirmed(out, &plan)? {
        out.line("Cancelled.");
        return Ok(());
    }

    let started = start_or_join(&store, &plan)
        .map_err(|error| CliError::new(format!("{}: {error}", plan.reference)))?;
    let job = match started {
        Started::Created(id) => store.open(&id)?,
        // A pull of this model turned up between the lookup and the start; it
        // is that one, wherever it is.
        Started::Joined | Started::Resumed => store
            .under_way(&provider, &plan.reference, now_millis())
            .ok_or_else(|| {
                CliError::new(format!(
                    "{} ended before it could be joined",
                    plan.reference
                ))
            })?,
    };
    hand_off(out, &job, args.detach).await
}

/// Show what will be fetched and where, and ask before it is.
fn confirmed(out: &Out, plan: &InstallPlan) -> Result<bool, CliError> {
    let size = plan
        .remaining_bytes
        .or(plan.total_bytes)
        .map(|bytes| format!(", ~{} MB", bytes / 1_000_000))
        .unwrap_or_default();
    out.line(&format!(
        "{} → {}{size}",
        plan.display_name, plan.destination
    ));
    interactive::confirm("Download now?", true)
}

/// Join a pull that already exists, starting a worker again when it had
/// stopped: asking for the model is asking for it to go on, whatever
/// `pull.auto_resume` says about pulls nobody asked about. `false` when the
/// job turned out to be over, which leaves the way clear for a new one.
async fn rejoin(out: &Out, job: &PullJobDir, detach: bool) -> Result<bool, CliError> {
    if job.status().state.is_resumable() {
        match restart(job) {
            Ok(_) => out.line(&format!("resuming {} ({})", job.id(), job.job().reference)),
            // A cancel its worker never read has just been honoured.
            Err(WorkerError::Cancelled) => {
                out.line(&format!("{} was cancelled; starting afresh", job.id()));
                return Ok(false);
            }
            Err(error) => return Err(CliError::new(format!("{}: {error}", job.id()))),
        }
    } else {
        out.line(&format!(
            "{} is already being pulled as {}",
            job.job().reference,
            job.id()
        ));
    }
    hand_off(out, job, detach).await?;
    Ok(true)
}

/// Watch the worker, or leave it to itself.
async fn hand_off(out: &Out, job: &PullJobDir, detach: bool) -> Result<(), CliError> {
    if detach {
        return detached(out, job);
    }
    match attach::follow(out, job).await {
        Attached::Ended(status) => attach::report(out, job, &status),
        Attached::Detached => detached(out, job),
    }
}

/// Say where the download went and what commands reach it there.
fn detached(out: &Out, job: &PullJobDir) -> Result<(), CliError> {
    out.line(&view::detached(job));
    out.json(&view::json(job, &job.status()));
    Ok(())
}
