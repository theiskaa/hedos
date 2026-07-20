//! `hedos pull [reference]` — fetch a model from Ollama or Hugging Face. With a
//! reference it installs directly; without one it searches Hugging Face (or offers
//! RAM-fit recommendations) and lets you pick. Ctrl-C cancels a download.

use clap::Args;
use kernel::install::event::{InstallEvent, InstallProgress};
use kernel::install::reference::{hugging_face_repo, ollama_install_tag};
use kernel::install::{InstallCatalogEntry, InstallProviderId, InstallSearchHit, recommended};
use runtime::governor::GovernorConfig;
use runtime::install::service::InstallService;

use crate::error::CliError;
use crate::support::output::Out;
use crate::support::session::Session;
use crate::support::{interactive, progress, signals};

/// Arguments for `pull`.
#[derive(Args)]
pub struct PullArgs {
    /// The model reference: a Hugging Face repo (`org/model`) or an Ollama tag
    /// (`gemma3:4b`). Omit to search interactively.
    reference: Option<String>,
    /// Force a provider: `ollama` or `hf`.
    #[arg(long)]
    from: Option<String>,
}

/// Run the `pull` command.
pub async fn run(args: PullArgs, out: &Out) -> Result<(), CliError> {
    let session = Session::open()?;
    let install = runtime::boot::default_install_service();

    let (provider, reference) = resolve_target(out, &install, &args).await?;
    let plan = install.plan(&provider, &reference).await?;
    if plan.requires_auth {
        return Err(CliError::new(
            "this model is gated — set HF_TOKEN in the environment and retry",
        ));
    }

    if interactive::is_interactive(out) {
        let size = plan
            .remaining_bytes
            .or(plan.total_bytes)
            .map(|bytes| format!(", ~{} MB", bytes / 1_000_000))
            .unwrap_or_default();
        out.line(&format!(
            "{} → {}{size}",
            plan.display_name, plan.destination
        ));
        if !interactive::confirm("Download now?", true)? {
            out.line("Cancelled.");
            return Ok(());
        }
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
    out.line(&format!("pulled {reference}"));
    out.line(&summary.headline());
    out.json(&serde_json::json!({
        "pulled": reference,
        "provider": provider.as_str(),
    }));
    Ok(())
}

/// The provider and reference to install: taken from the argument, or chosen
/// through an interactive search when no reference was given.
async fn resolve_target(
    out: &Out,
    install: &InstallService,
    args: &PullArgs,
) -> Result<(InstallProviderId, String), CliError> {
    if let Some(reference) = &args.reference {
        let provider = provider_for(reference, args.from.as_deref())?;
        return Ok((provider, reference.clone()));
    }
    if !interactive::is_interactive(out) {
        return Err(CliError::new(
            "no reference given — pass one (org/model or name:tag), or run in a terminal to search",
        ));
    }

    let query = interactive::input("search models (enter/return for recommendations)", true)?;
    if query.trim().is_empty() {
        return pick_recommended();
    }

    let result = install.browse(query.trim(), 25).await;
    if result.hits.is_empty() {
        let hint = result
            .failure_hint
            .unwrap_or_else(|| format!("nothing matched \"{}\"", query.trim()));
        return Err(CliError::new(hint));
    }
    let labels: Vec<String> = result.hits.iter().map(hit_label).collect();
    let index = interactive::select_index("model", &labels)?;
    let hit = &result.hits[index];
    Ok((hit.provider.clone(), hit.reference.clone()))
}

/// Offer the catalog models that fit this machine's RAM and return the pick.
fn pick_recommended() -> Result<(InstallProviderId, String), CliError> {
    let total_mb = GovernorConfig::detect().total_memory_mb.max(1) as u64;
    let entries = recommended(None, total_mb * 1024 * 1024, None);
    if entries.is_empty() {
        return Err(CliError::new(
            "no recommendations available — pass a reference instead",
        ));
    }
    let labels: Vec<String> = entries.iter().map(catalog_label).collect();
    let index = interactive::select_index("recommended", &labels)?;
    let entry = &entries[index];
    Ok((entry.provider.clone(), entry.reference.clone()))
}

/// A picker label for a search hit: the reference plus download and like counts.
fn hit_label(hit: &InstallSearchHit) -> String {
    let downloads = hit
        .downloads
        .map(|count| format!("  ↓{count}"))
        .unwrap_or_default();
    let likes = hit
        .likes
        .map(|count| format!("  ♥{count}"))
        .unwrap_or_default();
    format!("{}{downloads}{likes}", hit.reference)
}

/// A picker label for a catalog entry: the reference, its size, and a blurb.
fn catalog_label(entry: &InstallCatalogEntry) -> String {
    format!(
        "{}  ({:.1} GB)  {}",
        entry.reference, entry.size_gb, entry.blurb
    )
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

/// A one-line progress string for the current download, with a bar when the total
/// size is known.
fn progress_line(progress: &InstallProgress) -> String {
    let file = progress
        .current_file
        .as_deref()
        .map(|name| format!("  {name}"))
        .unwrap_or_default();
    // Only draw a bar against a reliable total; `fraction()` returns None while
    // the total is still a growing estimate (Ollama layer manifests), where a
    // computed percentage would jump backward as more layers appear.
    match progress.fraction() {
        Some(fraction) => format!(
            "{} {:>3}%  {} / {} MB{file}",
            progress::bar(fraction, 24),
            (fraction * 100.0) as i64,
            progress.bytes_downloaded / 1_000_000,
            progress.total_bytes.unwrap_or(0) / 1_000_000,
        ),
        None => format!("{} MB{file}", progress.bytes_downloaded / 1_000_000),
    }
}
