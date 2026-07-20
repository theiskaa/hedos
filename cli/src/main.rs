//! `hedos` — run and serve local models headlessly. A thin shell over the
//! kernel/runtime/gateway crates: it assembles a production kernel from the
//! user's data dir and settings, then drives one subcommand.

mod commands;
mod error;
mod support;

use clap::{Parser, Subcommand};

use crate::support::banner::BANNER;
use crate::support::output::Out;

/// The `hedos` command line.
#[derive(Parser)]
#[command(
    name = "hedos",
    version,
    about = "Run and serve local models headlessly.",
    before_help = BANNER
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
    /// Stream a single completion.
    Run(commands::run::RunArgs),
    /// Chat interactively over stdin.
    Chat(commands::chat::ChatArgs),
    /// Run the OpenAI/Ollama-compatible gateway on loopback.
    Serve(commands::serve::ServeArgs),
    /// Fetch a model from Ollama or Hugging Face.
    Pull(commands::pull::PullArgs),
    /// Remove an installed model.
    Rm(commands::rm::RmArgs),
    /// Discover models on this machine and refresh the shelf.
    Scan(commands::scan::ScanArgs),
    /// Synthesize speech to a WAV file.
    Speak(commands::speak::SpeakArgs),
    /// Generate an image to a PNG file.
    Image(commands::image::ImageArgs),
    /// Load a model into residency.
    Warm(commands::warm::WarmArgs),
    /// Evict a model from residency.
    Unload(commands::unload::UnloadArgs),
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let out = Out::new(cli.json);
    let result = match cli.command {
        Command::Ls(args) => commands::ls::run(args, &out).await,
        Command::Run(args) => commands::run::run(args, &out).await,
        Command::Chat(args) => commands::chat::run(args, &out).await,
        Command::Serve(args) => commands::serve::run(args, &out).await,
        Command::Pull(args) => commands::pull::run(args, &out).await,
        Command::Rm(args) => commands::rm::run(args, &out).await,
        Command::Scan(args) => commands::scan::run(args, &out).await,
        Command::Speak(args) => commands::speak::run(args, &out).await,
        Command::Image(args) => commands::image::run(args, &out).await,
        Command::Warm(args) => commands::warm::run(args, &out).await,
        Command::Unload(args) => commands::unload::run(args, &out).await,
    };
    if let Err(error) = result {
        out.err(&error.message);
        std::process::exit(error.code);
    }
}
