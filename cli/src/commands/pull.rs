//! `hedos pull <reference>` — fetch a model from Ollama or Hugging Face,
//! showing download progress; Ctrl-C cancels.

use clap::Args;
use kernel::install::InstallProviderId;
use kernel::install::event::{InstallEvent, InstallProgress};
use kernel::install::reference::{hugging_face_repo, ollama_install_tag};

use crate::error::CliError;
use crate::support::output::Out;
use crate::support::session::Session;
use crate::support::signals;

/// Arguments for `pull`.
#[derive(Args)]
pub struct PullArgs {
    /// The model reference: a Hugging Face repo (`org/model`) or an Ollama tag
    /// (`gemma3:4b`).
    reference: String,
    /// Force a provider: `ollama` or `hf`.
    #[arg(long)]
    from: Option<String>,
}

/// Run the `pull` command.
pub async fn run(args: PullArgs, out: &Out) -> Result<(), CliError> {
    let provider = provider_for(&args.reference, args.from.as_deref())?;
    let session = Session::open()?;
    let install = runtime::boot::default_install_service();

    let plan = install.plan(&provider, &args.reference).await?;
    if plan.requires_auth {
        return Err(CliError::new(
            "this model is gated — set HF_TOKEN in the environment and retry",
        ));
    }
    let id = install.begin(plan)?;
    let mut events = install.events(&id);

    let mut cancelled = false;
    loop {
        tokio::select! {
            event = events.recv() => match event {
                Some(InstallEvent::Progress(progress)) => out.progress(&progress_line(&progress)),
                Some(InstallEvent::Status(status)) => out.progress(&status),
                Some(InstallEvent::Failed { message }) => {
                    out.progress_done();
                    return Err(CliError::new(message));
                }
                Some(InstallEvent::Cancelled) => {
                    cancelled = true;
                    break;
                }
                Some(InstallEvent::Done) => break,
                Some(_) => {}
                None => break,
            },
            () = signals::wait_for_ctrl_c() => install.cancel(&id),
        }
    }
    out.progress_done();
    if cancelled {
        out.err("cancelled");
        return Ok(());
    }

    let summary = session.discover().await?;
    out.line(&format!("pulled {}", args.reference));
    out.line(&summary.headline());
    out.json(&serde_json::json!({
        "pulled": args.reference,
        "provider": provider.as_str(),
    }));
    Ok(())
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

/// A one-line progress string for the current download.
fn progress_line(progress: &InstallProgress) -> String {
    let file = progress
        .current_file
        .as_deref()
        .map(|name| format!("  {name}"))
        .unwrap_or_default();
    match progress.total_bytes {
        Some(total) if total > 0 => {
            let percent = (progress.bytes_downloaded as f64 / total as f64 * 100.0) as i64;
            format!(
                "{percent}%  {} / {} MB{file}",
                progress.bytes_downloaded / 1_000_000,
                total / 1_000_000,
            )
        }
        _ => format!("{} MB{file}", progress.bytes_downloaded / 1_000_000),
    }
}
