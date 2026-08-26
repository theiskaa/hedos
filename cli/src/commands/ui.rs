//! `hedos ui` — manage the shelf in a terminal UI.

use std::io::{self, IsTerminal};

use clap::Args;

use crate::error::CliError;
use crate::support::interactive;
use crate::support::machine;
use crate::support::output::Out;
use crate::support::session::Session;
use crate::tui::{self, App};

/// Arguments for `ui`.
#[derive(Args)]
pub struct UiArgs {}

/// Run the `ui` command.
pub async fn run(_args: UiArgs, out: &Out) -> Result<(), CliError> {
    // The UI reads keys from stdin and draws on stdout; both must be the tty.
    if !interactive::is_interactive(out) || !io::stdout().is_terminal() {
        return Err(CliError::new("hedos ui needs a terminal"));
    }
    let session = Session::open()?;
    let shelf = session.shelf_or_discover().await?;
    let warm = session.warm_set_with_gateway().await;
    let app = App::new(shelf.to_vec(), warm, machine::memory_budget_bytes());
    tui::run(app).await
}
