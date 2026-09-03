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

use kernel::install::pulls::{PullEventKind, PullJobDir, PullState};
use kernel::install::{InstallError, InstallPlan};
use kernel::time::now_millis;
use runtime::boot::{self, HedosDirs};
use runtime::install::{restart, spawn_detached};
use runtime::settings::{Settings, SettingsStore};

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

    if let Some(job) = store.under_way(&provider, &reference, now_millis()) {
        return rejoin(out, &job, args.detach, &settings).await;
    }

    let plan = install.plan(&provider, &reference).await?;
    if plan.requires_auth {
        // One canonical voice for a gated repo: the same guidance the download
        // path surfaces, rather than a second, thinner message here.
        return Err(InstallError::AuthRequired(reference).into());
    }
    if let Some(job) = store.under_way(&provider, &plan.reference, now_millis()) {
        return rejoin(out, &job, args.detach, &settings).await;
    }
    if interactive::is_interactive(out) && !confirmed(out, &plan)? {
        out.line("Cancelled.");
        return Ok(());
    }

    let job = store.create(&plan, now_millis())?;
    spawn(&job)?;
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

/// Start the worker, and settle the job if it cannot be started: a job left
/// queued for a process that was never spawned would wait for good.
fn spawn(job: &PullJobDir) -> Result<(), CliError> {
    let Err(error) = spawn_detached(job) else {
        return Ok(());
    };
    let message = format!("could not start a worker: {error}");
    let now = now_millis();
    job.update_status(now, |status| {
        status.state = PullState::Failed;
        status.message = Some(message.clone());
    })?;
    job.append(
        PullEventKind::State {
            state: PullState::Failed,
        },
        now,
    )?;
    Err(CliError::new(message))
}

/// Join a pull that already exists, starting a worker again when it had
/// stopped.
async fn rejoin(
    out: &Out,
    job: &PullJobDir,
    detach: bool,
    settings: &Settings,
) -> Result<(), CliError> {
    let status = job.status();
    if status.state.is_resumable() {
        // A pull the user stopped stays stopped when they have said pulls are
        // not to be picked back up on their own.
        if !settings.pull.auto_resume {
            out.err(&view::resumable(job, &status));
            out.json(&view::json(job, &status));
            return Ok(());
        }
        restart(job).map_err(|error| CliError::new(format!("{}: {error}", job.id())))?;
        out.line(&format!("resuming {} ({})", job.id(), job.job().reference));
    } else {
        out.line(&format!(
            "{} is already being pulled as {}",
            job.job().reference,
            job.id()
        ));
    }
    hand_off(out, job, detach).await
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
