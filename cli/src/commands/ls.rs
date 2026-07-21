//! `hedos ls` — list the models on the shelf, with their runtime, store,
//! capabilities, and whether each is currently warm.

use clap::Args;
use kernel::records::Capability;

use crate::error::CliError;
use crate::support::output::Out;
use crate::support::session::Session;
use crate::support::shelf_table;

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
    // `retain` below needs an owned, mutable `Vec`; this is the one cold CLI
    // boundary where the shared shelf snapshot is unpacked into one.
    let mut shelf = if args.scan {
        session.discover().await?;
        session.shelf().await.to_vec()
    } else {
        session.shelf_or_discover().await?.to_vec()
    };
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

    let warm = session.warm_set();
    let records: Vec<&_> = shelf.iter().collect();
    out.line(&shelf_table::table(&records, &warm));
    Ok(())
}
