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
    /// Count the shelf's bytes on disk, as the process `hedos shelf` starts
    /// for it.
    #[command(hide = true)]
    DiskCount,
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
    // them are dropped, which shutting the runtime down does. A command cut
    // short may leave a blocking read of the terminal that never returns, so
    // the runtime is not waited on for it past a moment.
    if signals::termination().is_some() || matches!(result, Ok(Ended::OutputClosed)) {
        runtime.shutdown_timeout(TERMINATION_GRACE);
    } else {
        drop(runtime);
    }
    match result {
        Ok(Ended::Finished | Ended::OutputClosed) => match output::write_failure() {
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
    /// The reader of its stdout left, so nothing would read the rest.
    OutputClosed,
}

/// Run `command`. SIGTERM and SIGHUP stop it the way Ctrl-C stops `serve`:
/// `serve` and the shelf screen stop themselves in order, a pull worker keeps
/// the default disposition its controller relies on (as does the shelf's
/// disk count, which a signal simply ends), and every other command
/// is dropped where it stands, which stops the servers it started. A closed
/// stdout stops those other commands the same way, as a success: `serve`
/// keeps serving after its address was read, and the shelf draws on a
/// terminal.
async fn run(command: Command, out: &Out) -> Result<Ended, CliError> {
    if matches!(command, Command::PullWorker(_) | Command::DiskCount) {
        return dispatch(command, out).await.map(|()| Ended::Finished);
    }
    signals::watch_termination();
    if matches!(command, Command::Serve(_) | Command::Shelf(_)) {
        return dispatch(command, out).await.map(|()| Ended::Finished);
    }
    // Biased, so a signal that lands as the reader leaves (a shell closing
    // a pipeline) still ends with the signal's code.
    tokio::select! {
        biased;
        signal = signals::terminated() => Ok(Ended::Terminated(signal)),
        () = output::stdout_closed() => Ok(Ended::OutputClosed),
        result = dispatch(command, out) => result.map(|()| Ended::Finished),
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
        Command::DiskCount => commands::disk_count::run(out),
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
