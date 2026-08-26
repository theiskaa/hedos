//! `hedos ui` — manage the shelf in a terminal UI.

use std::io::{self, IsTerminal};

use clap::Args;

use crate::error::CliError;
use crate::support::interactive;
use crate::support::output::Out;
use crate::support::session::Session;
use crate::tui;

/// Arguments for `ui`.
#[derive(Args)]
pub struct UiArgs {}

/// Run the `ui` command.
pub async fn run(_args: UiArgs, out: &Out) -> Result<(), CliError> {
    // The UI reads keys from stdin and draws on stdout; both must be the tty.
    if !interactive::is_interactive(out) || !io::stdout().is_terminal() {
        return Err(CliError::new("hedos ui needs a terminal"));
    }
    tui::run(Session::open()?, out).await
}
