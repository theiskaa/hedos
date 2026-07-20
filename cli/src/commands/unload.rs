//! `hedos unload <model>` — evict a model from in-process residency.

use std::collections::HashSet;

use clap::Args;
use kernel::records::ModelRecord;

use crate::error::CliError;
use crate::support::interactive;
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
    let warm = session.warm_set();
    let record = match args.model.as_deref() {
        Some(query) => session::resolve(query, &shelf, None)?,
        None => pick_resident(out, &shelf, &warm)?,
    };

    session
        .kernel
        .governor()
        .residency()
        .unload_now(&record.id)
        .await;
    let resident = session.kernel.governor().is_resident(&record.id);

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
