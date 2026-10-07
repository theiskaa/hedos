//! `hedos` — run and serve local models headlessly. A thin shell over the
//! kernel/runtime/gateway crates: it assembles a production kernel from the
//! user's data dir and settings, then drives one subcommand.

mod commands;
mod error;
mod support;
mod tui;

use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand};

use crate::error::CliError;
use crate::support::banner::BANNER;
use crate::support::output::{self, Out};
use crate::support::signals;

/// The `hedos` command line.
#[derive(Parser)]
#[command(
    name = "hedos",
    version,
    about = "Run and serve local models headlessly.",
    before_help = BANNER.as_str()
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
    /// Run the OpenAI/Ollama/Anthropic-compatible gateway on loopback.
    Serve(commands::serve::ServeArgs),
    /// Run a coding harness (opencode, claude, aider) on a local model.
    Launch(commands::launch::LaunchArgs),
    /// Fetch a model from Ollama or Hugging Face.
    Pull(commands::pull::PullArgs),
    /// Run one pull, as the process `hedos pull` spawns for it.
    #[command(hide = true)]
    PullWorker(commands::pull::worker::PullWorkerArgs),
    /// Remove an installed model.
    Rm(commands::rm::RmArgs),
    /// List the manifest runtimes, and approve or revoke one.
    Runtimes(commands::runtimes::RuntimesArgs),
    /// Discover models on this machine and refresh the shelf.
    Scan(commands::scan::ScanArgs),
    /// Synthesize speech to a WAV file.
    Speak(commands::speak::SpeakArgs),
    /// Transcribe an audio file to text.
    Transcribe(commands::transcribe::TranscribeArgs),
    /// Generate an image to a PNG file.
    Image(commands::image::ImageArgs),
    /// Show aggregate statistics from the gateway audit log.
    Stats(commands::stats::StatsArgs),
    /// Measure what each model does on this machine.
    Bench(commands::bench::BenchArgs),
    /// Open the shelf as a terminal screen.
    Shelf(commands::shelf::ShelfArgs),
    /// Load a model into residency.
    Warm(commands::warm::WarmArgs),
    /// Evict a model from residency.
    Unload(commands::unload::UnloadArgs),
}

/// How long an exit forced by a termination waits for blocking work, such as a
/// read of the terminal that will never complete, before leaving it behind.
const TERMINATION_GRACE: Duration = Duration::from_secs(2);

fn main() -> ExitCode {
    let cli = Cli::parse();
    let out = Out::new(cli.json);
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            out.err(&format!("could not start the async runtime: {error}"));
            return ExitCode::FAILURE;
        }
    };
    let result = runtime.block_on(run(cli.command, &out));
    // The model servers a command started are stopped when the tasks holding
    // them are dropped, which shutting the runtime down does. After a
    // termination a blocking read of the terminal may never return, so the
    // runtime is not waited on for it past a moment.
    if signals::termination().is_some() {
        runtime.shutdown_timeout(TERMINATION_GRACE);
    } else {
        drop(runtime);
    }
    match result {
        Ok(Ended::Finished) => match output::write_failure() {
            Some(failure) => {
                out.err(&format!("could not write the output: {failure}"));
                ExitCode::FAILURE
            }
            None => ExitCode::SUCCESS,
        },
        Ok(Ended::Terminated(signal)) => ExitCode::from((128 + signal) as u8),
        Err(error) => {
            out.err(&error.message);
            // As `exit` would: the status is the low byte of the code.
            ExitCode::from(error.code as u8)
        }
    }
}

/// How a command ended.
enum Ended {
    /// It ran to its end, a stop it handled itself included.
    Finished,
    /// A termination signal, this number, cut it short.
    Terminated(i32),
}

/// Run `command`. SIGTERM and SIGHUP stop it the way Ctrl-C stops `serve`:
/// `serve` and the shelf screen stop themselves in order, a pull worker keeps
/// the default disposition its controller relies on, and every other command
/// is dropped where it stands, which stops the servers it started.
async fn run(command: Command, out: &Out) -> Result<Ended, CliError> {
    if matches!(command, Command::PullWorker(_)) {
        return dispatch(command, out).await.map(|()| Ended::Finished);
    }
    signals::watch_termination();
    if matches!(command, Command::Serve(_) | Command::Shelf(_)) {
        return dispatch(command, out).await.map(|()| Ended::Finished);
    }
    tokio::select! {
        result = dispatch(command, out) => result.map(|()| Ended::Finished),
        signal = signals::terminated() => Ok(Ended::Terminated(signal)),
    }
}

async fn dispatch(command: Command, out: &Out) -> Result<(), CliError> {
    match command {
        Command::Ls(args) => commands::ls::run(args, out).await,
        Command::Run(args) => commands::run::run(args, out).await,
        Command::Chat(args) => commands::chat::run(args, out).await,
        Command::Serve(args) => commands::serve::run(args, out).await,
        Command::Launch(args) => commands::launch::run(args, out).await,
        Command::Pull(args) => commands::pull::run(args, out).await,
        Command::PullWorker(args) => commands::pull::worker::run(args).await,
        Command::Rm(args) => commands::rm::run(args, out).await,
        Command::Runtimes(args) => commands::runtimes::run(args, out).await,
        Command::Scan(args) => commands::scan::run(args, out).await,
        Command::Speak(args) => commands::speak::run(args, out).await,
        Command::Transcribe(args) => commands::transcribe::run(args, out).await,
        Command::Image(args) => commands::image::run(args, out).await,
        Command::Stats(args) => commands::stats::run(args, out).await,
        Command::Bench(args) => commands::bench::run(args, out).await,
        Command::Shelf(args) => commands::shelf::run(args, out).await,
        Command::Warm(args) => commands::warm::run(args, out).await,
        Command::Unload(args) => commands::unload::run(args, out).await,
    }
}
