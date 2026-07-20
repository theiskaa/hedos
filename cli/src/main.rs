//! `hedos` — run and serve local models headlessly. A thin shell over the
//! kernel/runtime/gateway crates: it assembles a production kernel from the
//! user's data dir and settings, then drives one subcommand.

// The command surface is built out in phases; the shared support (name
// resolution, streamed output, interrupt handling) lands before the commands
// that use it. Lifted once every command is in place.
#![allow(dead_code)]

mod commands;
mod error;
mod support;

use clap::{Parser, Subcommand};

use crate::support::output::Out;

/// The `hedos` command line.
#[derive(Parser)]
#[command(
    name = "hedos",
    version,
    about = "Run and serve local models headlessly."
)]
struct Cli {
    /// Emit machine-readable JSON instead of formatted text.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

/// The subcommands.
#[derive(Subcommand)]
enum Command {
    /// List the models on the shelf.
    Ls(commands::ls::LsArgs),
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let out = Out::new(cli.json);
    let result = match cli.command {
        Command::Ls(args) => commands::ls::run(args, &out).await,
    };
    if let Err(error) = result {
        out.err(&error.message);
        std::process::exit(error.code);
    }
}
