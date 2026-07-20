//! `hedos unload <model>` — evict a model from in-process residency.

use clap::Args;

use crate::error::CliError;
use crate::support::output::Out;
use crate::support::session::{self, Session};

/// Arguments for `unload`.
#[derive(Args)]
pub struct UnloadArgs {
    /// The model to unload (name, alias, or id).
    model: String,
}

/// Run the `unload` command.
pub async fn run(args: UnloadArgs, out: &Out) -> Result<(), CliError> {
    let session = Session::open()?;
    let shelf = session.shelf(false).await?;
    let record = session::resolve(&args.model, &shelf, None)?;

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
