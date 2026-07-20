//! `hedos scan` — discover models on this machine and refresh the shelf.

use clap::Args;

use crate::error::CliError;
use crate::support::output::Out;
use crate::support::session::Session;

/// Arguments for `scan`.
#[derive(Args)]
pub struct ScanArgs {}

/// Run the `scan` command.
pub async fn run(_args: ScanArgs, out: &Out) -> Result<(), CliError> {
    let session = Session::open()?;
    let summary = session.discover().await?;
    for issue in &summary.issues {
        out.err(&format!("issue: {issue}"));
    }
    out.line(&summary.headline());
    out.json(&serde_json::json!({
        "totalCount": summary.total_count,
        "headline": summary.headline(),
        "issues": summary.issues,
    }));
    Ok(())
}
