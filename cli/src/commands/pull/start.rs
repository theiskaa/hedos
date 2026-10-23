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

use kernel::install::provider::InstallProviderId;
use kernel::install::pulls::{PullJobDir, PullReading, PullState, PullStore};
use kernel::install::{InstallError, InstallPlan};
use kernel::records::format_bytes;
use kernel::records::{ModelRecord, ModelState, RuntimeId};
use kernel::time::now_millis;
use runtime::boot::{self, HedosDirs};
use runtime::engines::uv;
use runtime::install::{Started, WorkerError, collect_ended, restart, start_or_join};
use runtime::settings::SettingsStore;

use crate::error::CliError;
use crate::support::engines::{Installable, offer_after_pull, offer_uv_after_pull};
use crate::support::interactive;
use crate::support::machine;
use crate::support::output::Out;
use crate::support::session::Session;

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

    if let Some(seen) = rejoined(out, &store, &provider, &reference, args.detach).await? {
        return after(seen, &reference, out).await;
    }

    let plan = install.plan(&provider, &reference).await?;
    if plan.requires_auth {
        // One canonical voice for a gated repo: the same guidance the download
        // path surfaces, rather than a second, thinner message here.
        return Err(InstallError::AuthRequired(reference).into());
    }
    if let Some(seen) = rejoined(out, &store, &provider, &plan.reference, args.detach).await? {
        return after(seen, &plan.reference, out).await;
    }
    if interactive::is_interactive(out) && !args.confirmed && !confirmed(out, &plan)? {
        out.line("Cancelled.");
        return Ok(());
    }

    let started = start_or_join(&store, &plan)
        .map_err(|error| CliError::new(format!("{}: {error}", plan.reference)))?;
    // A pull of this model can turn up between the lookup and the start; the
    // runtime names the job either way.
    let (job, named) = match &started {
        Started::Created(id) => (store.open(id)?, Named::NotYet),
        Started::Joined(id) => {
            let job = store.open(id)?;
            announce_joined(out, &job);
            (job, Named::Already)
        }
        Started::Resumed(id) => {
            let job = store.open(id)?;
            announce_resumed(out, &job);
            (job, Named::Already)
        }
    };
    let seen = hand_off(out, &job, args.detach, named).await?;
    after(seen, &plan.reference, out).await
}

/// How a pull ended for the person running `hedos pull`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Seen {
    /// They watched it land.
    Landed,
    /// Anything else: it failed, was left to run in the background, or was
    /// stopped.
    Otherwise,
}

/// After a pull of `reference` they watched land, ask once whether to install
/// the engine the model needs and this machine lacks ([`engine_to_offer`]).
async fn after(seen: Seen, reference: &str, out: &Out) -> Result<(), CliError> {
    if seen != Seen::Landed {
        return Ok(());
    }
    let Ok(session) = Session::open() else {
        return Ok(());
    };
    let shelf = session.shelf().await;
    let Some((name, engine)) = engine_to_offer(&shelf, reference, uv::needed) else {
        return Ok(());
    };
    let offered = match engine {
        Installable::LlamaCpp => offer_after_pull(name, &machine::machine(), out).await,
        Installable::Uv => offer_uv_after_pull(name, out).await,
    };
    install_after_landing(name, engine, offered, out);
    Ok(())
}

/// The pull landed whatever became of the install offered after it: one
/// that failed is said, and the pull still succeeds.
fn install_after_landing(
    name: &str,
    engine: Installable,
    offered: Result<(), CliError>,
    out: &Out,
) {
    if let Err(error) = offered {
        out.err(&format!(
            "{name} was pulled, but installing {} did not finish: {error}",
            engine.name()
        ));
    }
}

/// The name of the model the pull of `reference` registered on `shelf`, the
/// one whose repo it is, and the engine to offer for it: llama.cpp when that
/// serves it (offered only when none is here), uv when its runtime needs uv
/// (`needs_uv`, which says whether the runtime's environment is unbuilt and
/// no uv is here).
fn engine_to_offer<'a>(
    shelf: &'a [ModelRecord],
    reference: &str,
    needs_uv: impl Fn(&RuntimeId) -> bool,
) -> Option<(&'a str, Installable)> {
    let record = shelf.iter().find(|record| {
        record.state != ModelState::Missing
            && record
                .source
                .repo
                .as_deref()
                .is_some_and(|repo| repo.eq_ignore_ascii_case(reference))
    })?;
    let runtime = record.runtime.id.as_ref()?;
    let engine = if *runtime == RuntimeId::llama_cpp() {
        Installable::LlamaCpp
    } else if needs_uv(runtime) {
        Installable::Uv
    } else {
        return None;
    };
    Some((record.display_name(), engine))
}

/// Join the pull of `reference` already under way, if there is one, and how
/// it ended for the person watching; `None` when there was none.
async fn rejoined(
    out: &Out,
    store: &PullStore,
    provider: &InstallProviderId,
    reference: &str,
    detach: bool,
) -> Result<Option<Seen>, CliError> {
    match store.under_way(provider, reference, now_millis()) {
        Some(job) => rejoin(out, &job, detach).await,
        None => Ok(None),
    }
}

/// Join a pull that already exists, starting a worker again when it had
/// stopped: asking for the model is asking for it to go on, whatever
/// `pull.auto_resume` says about pulls nobody asked about. `None` when the
/// job turned out to be over, which leaves the way clear for a new one.
async fn rejoin(out: &Out, job: &PullJobDir, detach: bool) -> Result<Option<Seen>, CliError> {
    if job.status().state.is_resumable() {
        match restart(job) {
            Ok(_) => announce_resumed(out, job),
            // A cancel its worker never read has just been honoured.
            Err(WorkerError::Cancelled) => {
                out.line(&format!("{} was cancelled; starting afresh", job.id()));
                return Ok(None);
            }
            // Another client put a worker on it in the same moment.
            Err(WorkerError::AlreadyRunning) => announce_joined(out, job),
            // It ended between the lookup and now, most likely landing.
            Err(WorkerError::Ended(_)) => {
                let reading = job.reading(now_millis());
                attach::report(out, job, &reading)?;
                return Ok(Some(seen(&reading)));
            }
            Err(error) => return Err(CliError::new(format!("{}: {error}", job.id()))),
        }
    } else {
        announce_joined(out, job);
    }
    hand_off(out, job, detach, Named::Already).await.map(Some)
}

fn announce_joined(out: &Out, job: &PullJobDir) {
    out.line(&format!(
        "{} is already being pulled as {}",
        job.job().reference,
        job.id()
    ));
}

fn announce_resumed(out: &Out, job: &PullJobDir) {
    out.line(&format!("resuming {} ({})", job.id(), job.job().reference));
}

/// Show what will be fetched and where, and ask before it is.
fn confirmed(out: &Out, plan: &InstallPlan) -> Result<bool, CliError> {
    let size = plan
        .remaining_bytes
        .or(plan.total_bytes)
        .map(|bytes| format!(", ~{}", format_bytes(bytes)))
        .unwrap_or_default();
    out.line(&format!(
        "{} → {}{size}",
        plan.display_name, plan.destination
    ));
    interactive::confirm("Download now?", true)
}

/// Whether a line has already named the pull, so leaving it in the
/// background does not name it a second time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Named {
    Already,
    NotYet,
}

/// Follow the pull, or leave it to its worker.
async fn hand_off(
    out: &Out,
    job: &PullJobDir,
    detach: bool,
    named: Named,
) -> Result<Seen, CliError> {
    if detach {
        detached(out, job, named)?;
        return Ok(Seen::Otherwise);
    }
    match attach::follow(out, job).await {
        Attached::Ended(reading) => {
            attach::report(out, job, &reading)?;
            Ok(seen(&reading))
        }
        Attached::Detached => {
            detached(out, job, Named::NotYet)?;
            Ok(Seen::Otherwise)
        }
    }
}

/// Whether `reading` is of a pull that landed.
fn seen(reading: &PullReading) -> Seen {
    if reading.status.state == PullState::Done {
        Seen::Landed
    } else {
        Seen::Otherwise
    }
}

/// Say where the download went, unless that was just said, and what
/// commands reach it there.
fn detached(out: &Out, job: &PullJobDir, named: Named) -> Result<(), CliError> {
    out.line(&match named {
        Named::Already => view::reach(job),
        Named::NotYet => view::detached(job),
    });
    out.json(&view::json(job, &job.reading(now_millis())));
    Ok(())
}

#[cfg(test)]
mod tests;
