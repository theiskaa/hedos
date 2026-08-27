//! `hedos shelf` — the shelf as a terminal screen: browse, pull, warm, chat,
//! launch, all in one place.

use std::io::{self, IsTerminal};

use clap::Args;

use crate::error::CliError;
use crate::support::interactive;
use crate::support::output::Out;
use crate::support::session::Session;
use crate::tui;

/// Arguments for `shelf`.
#[derive(Args)]
pub struct ShelfArgs {}

/// Run the `shelf` command.
pub async fn run(_args: ShelfArgs, out: &Out) -> Result<(), CliError> {
    // The UI reads keys from stdin and draws on stdout; both must be the tty.
    if !interactive::is_interactive(out) || !io::stdout().is_terminal() {
        return Err(CliError::new("hedos shelf needs a terminal"));
    }
    tui::run(Session::open()?, out).await
}
