//! `hedos pull [reference]` — fetch a model from Ollama or Hugging Face. With a
//! reference it installs directly; without one it searches Hugging Face (or offers
//! RAM-fit recommendations) and lets you pick. Ctrl-C cancels a download.

use std::collections::HashSet;

use clap::Args;
use kernel::install::event::InstallEvent;
use kernel::install::reference::{hugging_face_repo, ollama_install_tag};
use kernel::install::{InstallCatalogEntry, InstallProviderId, InstallSearchHit, recommended};
use kernel::records::ModelRecord;
use runtime::install::service::InstallService;

use crate::error::CliError;
use crate::support::download::Download;
use crate::support::output::Out;
use crate::support::session::Session;
use crate::support::{interactive, machine, signals};

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
    let shelf = session.shelf().await;

    let (provider, reference) = resolve_target(out, &install, &args, &shelf).await?;
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

    let mut download = Download::start(out);
    let mut cancelled = false;
    loop {
        tokio::select! {
            event = events.recv() => match event {
                Some(InstallEvent::Progress(progress)) => download.progress(&progress),
                Some(InstallEvent::Status(status)) => download.status(&status),
                Some(InstallEvent::Failed { message }) => {
                    download.finish();
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
    download.finish();
    if cancelled {
        out.err("cancelled");
        return Ok(());
    }

    // Re-scan so the new model registers on the shelf; the census itself is noise
    // after pulling a single model, so only the confirmation is printed.
    session.discover().await?;
    out.line(&format!("pulled {reference}"));
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
    shelf: &[ModelRecord],
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
        return pick_recommended(shelf);
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

/// Offer the catalog models that fit this machine's RAM and are not already on the
/// shelf, and return the pick.
fn pick_recommended(shelf: &[ModelRecord]) -> Result<(InstallProviderId, String), CliError> {
    let installed = installed_names(shelf);
    let entries: Vec<InstallCatalogEntry> = recommended(None, machine::memory_budget_bytes(), None)
        .into_iter()
        .filter(|entry| !is_installed(&entry.reference, &installed))
        .collect();
    if entries.is_empty() {
        return Err(CliError::new(
            "the recommended models are already installed — search by name or pass a reference",
        ));
    }
    let labels: Vec<String> = entries.iter().map(catalog_label).collect();
    let index = interactive::select_index("recommended", &labels)?;
    let entry = &entries[index];
    Ok((entry.provider.clone(), entry.reference.clone()))
}

/// The lowercased ids, names, and display names of every model on the shelf, for
/// matching an install reference against what is already present.
fn installed_names(shelf: &[ModelRecord]) -> HashSet<String> {
    shelf
        .iter()
        .flat_map(|record| {
            [
                record.id.to_lowercase(),
                record.name.to_lowercase(),
                record.display_name().to_lowercase(),
            ]
        })
        .collect()
}

/// Whether `reference` names a model already on the shelf: a direct match, or a
/// match on its last path segment (so `org/Model` matches an installed `Model`).
fn is_installed(reference: &str, installed: &HashSet<String>) -> bool {
    let reference = reference.to_lowercase();
    installed.contains(&reference)
        || installed.contains(reference.rsplit('/').next().unwrap_or(&reference))
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
