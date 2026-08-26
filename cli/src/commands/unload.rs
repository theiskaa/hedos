//! `hedos unload <model>` — evict a model from in-process residency, or from
//! the Ollama daemon when the daemon holds it.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use clap::Args;
use kernel::records::ModelRecord;

use crate::error::CliError;
use crate::support::interactive;
use crate::support::ollama;
use crate::support::output::Out;
use crate::support::session::{self, Session};

/// Arguments for `unload`.
#[derive(Args)]
pub struct UnloadArgs {
    /// The model to unload (name, alias, or id). Omit to pick a resident one.
    model: Option<String>,
}

/// Run the `unload` command.
pub async fn run(args: UnloadArgs, out: &Out) -> Result<(), CliError> {
    let session = Session::open()?;
    let shelf = session.shelf().await;
    let warm = session.warm_set_anywhere(&shelf).await;
    let record = match args.model.as_deref() {
        Some(query) => session::resolve(query, &shelf, None)?,
        None => pick_resident(out, &shelf, &warm)?,
    };

    let resident = unload_anywhere(&session, record).await?;

    out.line(&format!(
        "{} {}",
        record.display_name(),
        if resident {
            "is still resident"
        } else {
            "unloaded"
        },
    ));
    out.json(&serde_json::json!({ "model": record.id, "resident": resident }));
    Ok(())
}

/// How long to give the Ollama daemon to let a model go after it agrees to;
/// it unloads after answering, so an immediate `/api/ps` still lists it.
const DAEMON_UNLOAD_GRACE: Duration = Duration::from_secs(5);
const DAEMON_POLL: Duration = Duration::from_millis(250);

/// Evict `record` from wherever it is loaded: this process's governor, or the
/// Ollama daemon when the daemon holds it. Whether it is still resident after.
pub(crate) async fn unload_anywhere(
    session: &Session,
    record: &ModelRecord,
) -> Result<bool, CliError> {
    session
        .kernel
        .governor()
        .residency()
        .unload_now(&record.id)
        .await;
    if !ollama::holds_now(record).await {
        return Ok(session.kernel.governor().is_resident(&record.id));
    }
    let tag = ollama::tag_of(record).unwrap_or(&record.name);
    ollama::unload(tag).await.map_err(CliError::new)?;
    let deadline = Instant::now() + DAEMON_UNLOAD_GRACE;
    while ollama::holds_now(record).await {
        if Instant::now() >= deadline {
            return Ok(true);
        }
        tokio::time::sleep(DAEMON_POLL).await;
    }
    Ok(false)
}

/// Pick from the currently resident models, since unloading a cold one is a no-op.
fn pick_resident<'a>(
    out: &Out,
    shelf: &'a [ModelRecord],
    warm: &HashSet<String>,
) -> Result<&'a ModelRecord, CliError> {
    let resident: Vec<&ModelRecord> = shelf
        .iter()
        .filter(|record| warm.contains(&record.id))
        .collect();
    if resident.is_empty() {
        return Err(CliError::new("no models are warm — nothing to unload"));
    }
    if !interactive::is_interactive(out) {
        return Err(CliError::new(
            "no model given — pass a name, or run in a terminal to pick one",
        ));
    }
    interactive::select_model("unload", &resident, warm)
}
