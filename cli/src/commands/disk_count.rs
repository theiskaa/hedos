//! `hedos disk-count`: the process the shelf counts its bytes on disk in.
//!
//! Nothing calls this by hand. Counting stats every model file, and a stat on
//! a mount that stopped answering can wait in the kernel where nothing
//! cancels it. A process with a thread waiting there cannot finish exiting,
//! so a shelf that counted in a thread of its own could not quit; it counts in
//! this child instead, which it can leave behind. The child reads the shelf's
//! records as JSON on stdin and prints the bytes on disk per store as JSON.

use std::io::Read;

use kernel::records::ModelRecord;

use crate::error::CliError;
use crate::support::output::Out;
use crate::tui::facts::disk_by_store;

/// Count the bytes on disk per store of the records on stdin.
pub fn run(out: &Out) -> Result<(), CliError> {
    let mut input = Vec::new();
    std::io::stdin()
        .read_to_end(&mut input)
        .map_err(|error| CliError::new(format!("could not read the records: {error}")))?;
    let records: Vec<ModelRecord> = serde_json::from_slice(&input)
        .map_err(|error| CliError::new(format!("could not read the records: {error}")))?;
    let figures = disk_by_store(&records);
    let printed = serde_json::to_string(&figures)
        .map_err(|error| CliError::new(format!("could not write the figures: {error}")))?;
    out.line(&printed);
    Ok(())
}
