//! `hedos rm <model>` — remove an installed model. Without `-y` it only previews.

use clap::Args;
use runtime::removal::{ModelRemover, OllamaModelRemover, permanent_delete_trasher};

use crate::error::CliError;
use crate::support::output::Out;
use crate::support::session::{self, Session};

/// Arguments for `rm`.
#[derive(Args)]
pub struct RmArgs {
    /// The model to remove (name, alias, or id).
    model: String,
    /// Actually delete the files (without this, only a dry-run preview is shown).
    #[arg(short = 'y', long)]
    yes: bool,
}

/// Run the `rm` command.
pub async fn run(args: RmArgs, out: &Out) -> Result<(), CliError> {
    let session = Session::open()?;
    let shelf = session.shelf(false).await?;
    let record = session::resolve(&args.model, &shelf, None)?;

    if !args.yes {
        let preview = kernel::removal::preview(record);
        out.line(&format!(
            "Would delete {} — {} item(s), ~{} MB. Re-run with -y to confirm.",
            preview.name,
            preview.paths.len(),
            preview.bytes_estimate / 1_000_000,
        ));
        out.json(&serde_json::json!({
            "model": record.id,
            "name": preview.name,
            "paths": preview.paths,
            "bytesEstimate": preview.bytes_estimate,
            "deleted": false,
        }));
        return Ok(());
    }

    let remover = ModelRemover::new(permanent_delete_trasher(), OllamaModelRemover::new());
    let report = remover.remove(record).await?;
    out.line(&format!(
        "Deleted {} — {} item(s), ~{} MB freed{}",
        report.name,
        report.trashed_paths.len(),
        report.freed_bytes_estimate / 1_000_000,
        if report.daemon_deleted {
            " (via the Ollama daemon)"
        } else {
            ""
        },
    ));
    out.json(&serde_json::json!({
        "model": report.model_id,
        "name": report.name,
        "trashedPaths": report.trashed_paths,
        "freedBytesEstimate": report.freed_bytes_estimate,
        "daemonDeleted": report.daemon_deleted,
        "deleted": true,
    }));
    Ok(())
}
