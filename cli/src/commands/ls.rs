//! `hedos ls` — list the models on the shelf, with their runtime, store,
//! capabilities, and whether each is currently warm.

use std::collections::HashSet;

use clap::Args;
use kernel::records::{Capability, ModelRecord};

use crate::error::CliError;
use crate::support::output::Out;
use crate::support::session::Session;

/// Arguments for `ls`.
#[derive(Args)]
pub struct LsArgs {
    /// Rescan the machine's model stores before listing.
    #[arg(long)]
    scan: bool,
    /// Only show models serving this capability (chat, complete, embed, …).
    #[arg(long)]
    capability: Option<String>,
}

/// Run the `ls` command.
pub async fn run(args: LsArgs, out: &Out) -> Result<(), CliError> {
    let session = Session::open()?;
    let mut shelf = session.shelf(args.scan).await?;
    if let Some(name) = &args.capability {
        let capability = Capability::from(name.as_str());
        shelf.retain(|record| record.capabilities.contains(&capability));
    }

    if out.is_json() {
        out.json(&serde_json::to_value(&shelf).unwrap_or_default());
        return Ok(());
    }

    if shelf.is_empty() {
        out.line("No models on the shelf — install one with `hedos pull <ref>`.");
        return Ok(());
    }

    let warm: HashSet<String> = session
        .kernel
        .resident_models()
        .into_iter()
        .filter_map(|entry| entry.model_id)
        .collect();

    out.line(&table(&shelf, &warm));
    Ok(())
}

/// Render the shelf as an aligned table.
fn table(shelf: &[ModelRecord], warm: &HashSet<String>) -> String {
    let rows: Vec<[String; 5]> = shelf
        .iter()
        .map(|record| {
            [
                if warm.contains(&record.id) {
                    "●"
                } else {
                    "○"
                }
                .to_owned(),
                record.display_name().to_owned(),
                record
                    .runtime
                    .id
                    .as_ref()
                    .map_or("—", |id| id.as_str())
                    .to_owned(),
                record.source.kind.as_str().to_owned(),
                record
                    .capabilities
                    .iter()
                    .map(Capability::as_str)
                    .collect::<Vec<_>>()
                    .join(", "),
            ]
        })
        .collect();

    let headers = ["", "NAME", "RUNTIME", "STORE", "CAPABILITIES"];
    let mut widths = headers.map(str::len);
    for row in &rows {
        for (column, cell) in row.iter().enumerate() {
            widths[column] = widths[column].max(cell.chars().count());
        }
    }

    let mut lines = Vec::with_capacity(rows.len() + 1);
    lines.push(format_row(&headers.map(String::from), &widths));
    for row in &rows {
        lines.push(format_row(row, &widths));
    }
    lines.join("\n")
}

/// Pad each cell to its column width and join with two spaces.
fn format_row(cells: &[String; 5], widths: &[usize; 5]) -> String {
    cells
        .iter()
        .enumerate()
        .map(|(column, cell)| {
            let pad = widths[column].saturating_sub(cell.chars().count());
            format!("{cell}{}", " ".repeat(pad))
        })
        .collect::<Vec<_>>()
        .join("  ")
        .trim_end()
        .to_owned()
}
