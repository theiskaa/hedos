//! `hedos pull`: fetch a model, and manage the pulls already under way.
//!
//! A pull runs in a worker process of its own, so the download outlives the
//! terminal that asked for it. `hedos pull <ref>` creates the job, spawns that
//! worker, and attaches to the record it writes; Ctrl-C detaches and leaves the
//! transfer running. The subcommands are the same jobs seen from anywhere else:
//! what is running, what it is doing, and how to stop or restart it.
//!
//! A bare word is a valid Ollama reference, so a model whose name matches a
//! subcommand is shadowed by it. `hedos pull -- ls` is the way to say the name.

mod attach;
mod manage;
mod pick;
mod start;
mod view;
pub mod worker;

use clap::{Args, Subcommand};
use kernel::install::InstallProviderId;
use kernel::install::reference::{hugging_face_repo, ollama_install_tag};
use runtime::boot::{self, HedosDirs};

use crate::error::CliError;
use crate::support::output::Out;

/// Arguments for `pull`.
#[derive(Args)]
pub struct PullArgs {
    #[command(subcommand)]
    command: Option<PullCommand>,
    /// The model reference: a Hugging Face repo (`org/model`) or an Ollama tag
    /// (`gemma3:4b`). Omit to search interactively.
    reference: Option<String>,
    /// Force a provider: `ollama` or `hf`.
    #[arg(long)]
    from: Option<String>,
    /// Start the download and return instead of following it.
    #[arg(short, long)]
    detach: bool,
}

/// Managing the pulls that are already running.
#[derive(Subcommand)]
pub(super) enum PullCommand {
    /// List every pull and what it is doing.
    Ls,
    /// Follow a pull's progress; Ctrl-C detaches.
    Attach(JobArgs),
    /// Stop a pull, keeping what it has downloaded.
    Pause(JobArgs),
    /// Start a paused or interrupted pull again.
    Resume(ResumeArgs),
    /// Stop a pull for good.
    Cancel(JobArgs),
    /// Print a pull's history.
    Logs(LogsArgs),
    /// Drop the records of pulls that have ended.
    Clean(CleanArgs),
}

/// The one pull a command acts on.
#[derive(Args)]
pub(super) struct JobArgs {
    /// The pull: its id, an unambiguous prefix of one, or its reference.
    job: String,
}

/// Which pulls to start again.
#[derive(Args)]
pub(super) struct ResumeArgs {
    /// The pull: its id, an unambiguous prefix of one, or its reference.
    #[arg(conflicts_with = "all")]
    job: Option<String>,
    /// Start every paused or interrupted pull.
    #[arg(long)]
    all: bool,
}

/// How much of a pull's history to print.
#[derive(Args)]
pub(super) struct LogsArgs {
    /// The pull: its id, an unambiguous prefix of one, or its reference.
    job: String,
    /// Show only the last `n` lines.
    #[arg(short = 'n', long)]
    lines: Option<usize>,
}

/// How much of the ended pulls to keep.
#[derive(Args)]
pub(super) struct CleanArgs {
    /// Keep the newest `n` ended pulls, however old they are.
    #[arg(long, default_value_t = 0)]
    keep: usize,
}

/// Run the `pull` command.
pub async fn run(args: PullArgs, out: &Out) -> Result<(), CliError> {
    let Some(command) = &args.command else {
        return start::run(&args, out).await;
    };
    // The flags belong to a reference, and a subcommand name shadows one. Left
    // unsaid, `hedos pull -d ls` would list instead of pulling a model called
    // `ls`, and nothing would tell the user which of the two it did.
    if args.detach || args.from.is_some() {
        return Err(CliError::new(
            "-d and --from apply to a reference, not to a subcommand. \
             to pull a model named after one, write `hedos pull -d -- <name>`",
        ));
    }
    // Managing a pull reads and writes the job directory and nothing else, so it
    // opens no registry and boots no kernel.
    let store = boot::pull_store(&HedosDirs::detect());
    match command {
        PullCommand::Ls => manage::list(&store, out),
        PullCommand::Attach(args) => manage::attach(&store, &args.job, out).await,
        PullCommand::Pause(args) => manage::pause(&store, &args.job, out),
        PullCommand::Resume(args) => manage::resume(&store, args, out),
        PullCommand::Cancel(args) => manage::cancel(&store, &args.job, out),
        PullCommand::Logs(args) => manage::logs(&store, args, out),
        PullCommand::Clean(args) => manage::clean(&store, args, out),
    }
}

/// Pick the install provider: an explicit `--from`, else inferred from the
/// reference shape.
fn provider_for(reference: &str, from: Option<&str>) -> Result<InstallProviderId, CliError> {
    match from {
        Some("ollama") => Ok(InstallProviderId::ollama()),
        Some("hf" | "huggingface") => Ok(InstallProviderId::huggingface()),
        Some(other) => Err(CliError::new(format!(
            "unknown provider \"{other}\" — use ollama or hf"
        ))),
        None if hugging_face_repo(reference).is_some() => Ok(InstallProviderId::huggingface()),
        None if ollama_install_tag(reference).is_some() => Ok(InstallProviderId::ollama()),
        None => Err(CliError::new(format!(
            "can't tell what \"{reference}\" is — pass --from ollama|hf"
        ))),
    }
}

#[cfg(test)]
mod tests;
