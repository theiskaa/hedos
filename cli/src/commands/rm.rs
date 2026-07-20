//! `hedos rm <model>` — remove an installed model. In a terminal it previews and
//! asks for confirmation; non-interactively it only previews unless `-y` is given.

use clap::Args;
use runtime::removal::{ModelRemover, OllamaModelRemover, permanent_delete_trasher};

use crate::error::CliError;
use crate::support::interactive;
use crate::support::output::Out;
use crate::support::session::Session;

/// Arguments for `rm`.
#[derive(Args)]
pub struct RmArgs {
    /// The model to remove (name, alias, or id). Omit to pick one interactively.
    model: Option<String>,
    /// Skip the confirmation prompt (required to delete non-interactively).
    #[arg(short = 'y', long)]
    yes: bool,
}

/// Run the `rm` command.
pub async fn run(args: RmArgs, out: &Out) -> Result<(), CliError> {
    let session = Session::open()?;
    let shelf = session.shelf().await;
    let warm = session.warm_set();
    let record =
        interactive::choose_model(out, args.model.as_deref(), &shelf, None, "remove", &warm)?;

    let preview = kernel::removal::preview(record);
    let summary = format!(
        "{} — {} item(s), ~{} MB",
        preview.name,
        preview.paths.len(),
        preview.bytes_estimate / 1_000_000,
    );

    if !args.yes {
        // A real terminal gets a y/N confirmation so one command completes the
        // delete; without a terminal we refuse to delete and only preview, so a
        // pipe or script can never silently remove weights.
        if interactive::is_interactive(out) {
            out.line(&format!("Delete {summary}."));
            if !interactive::confirm("This permanently deletes the files. Continue?", false)? {
                out.line("Kept.");
                return Ok(());
            }
        } else {
            out.line(&format!(
                "Would delete {summary}. Re-run with -y to confirm."
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
