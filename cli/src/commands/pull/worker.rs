//! `hedos pull-worker --job <dir>`: the process one pull runs in.
//!
//! Nothing calls this by hand. `hedos pull` spawns it detached, with its output
//! sent nowhere, and everything it has to say it says in the job directory it
//! was pointed at. It is a subcommand rather than a second binary so that
//! `current_exe()` is all a client needs to start one.

use std::path::PathBuf;
use std::sync::Arc;

use clap::Args;
use kernel::install::pulls::PullJobDir;
use runtime::boot;
use runtime::install::{PullWorker, Registrar, WorkerError};

use crate::error::CliError;
use crate::support::session::Session;

/// Arguments for `pull-worker`.
#[derive(Args)]
pub struct PullWorkerArgs {
    /// The job directory to run.
    #[arg(long)]
    job: PathBuf,
}

/// Run one pull to its end.
pub async fn run(args: PullWorkerArgs) -> Result<(), CliError> {
    // A worker outlives the terminal that started it, so the hangup that comes
    // with closing one must not reach it.
    runtime::install::ignore_hangup();
    let job = PullJobDir::open(&args.job)?;
    // The job is held before the session opens: opening one builds a kernel,
    // which on a cold disk can outlast the grace a client gives a worker to
    // show up, and a client that gave up would start a second pull.
    let claim = runtime::install::hold(&job)
        .await?
        .ok_or(WorkerError::AlreadyRunning)?;
    // Held and then unable to go on reads as interrupted, and would be started
    // again on every open with nothing to say why; the reason goes on the
    // record while the lock still makes the write this worker's.
    let session = match Session::open() {
        Ok(session) => session,
        Err(error) => {
            runtime::install::fail(&job, format!("could not open a session: {error}"))?;
            return Err(error);
        }
    };
    let install = boot::install_service(&session.settings);
    let worker = PullWorker::new(
        install,
        boot::pull_root(&session.dirs),
        &session.settings.pull,
    )
    .with_registrar(registrar(&session));
    worker.run_held(&job, claim).await?;
    Ok(())
}

/// What the worker does once the weights have landed: run discovery, so the
/// model reaches the shelf with nobody attached to watch it happen.
fn registrar(session: &Session) -> Registrar {
    let kernel = Arc::clone(&session.kernel);
    let settings = session.settings.clone();
    Arc::new(move || {
        let kernel = Arc::clone(&kernel);
        let scanners = boot::discovery_scanners(&settings);
        Box::pin(async move {
            kernel
                .discover(scanners)
                .await
                .map(|_| ())
                .map_err(|error| error.to_string())
        })
    })
}
