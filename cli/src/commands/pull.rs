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

    interactive_pick(out, install, shelf).await
}

/// One installable option offered by the interactive picker.
struct Candidate {
    label: String,
    provider: InstallProviderId,
    reference: String,
}

/// The interactive install picker: a search prompt that flows into a list of
/// results — or the memory-fit recommendations when the query is blank — with a
/// "search again" row so switching between the two, or trying another query,
/// never needs a fresh command. Loops until a model is chosen; Escape at the list
/// or Ctrl-C at the prompt exits.
async fn interactive_pick(
    out: &Out,
    install: &InstallService,
    shelf: &[ModelRecord],
) -> Result<(InstallProviderId, String), CliError> {
    const SEARCH_AGAIN: &str = "‹ search again";
    loop {
        let query = interactive::input("search models (blank for recommendations)", true)?;
        let query = query.trim();

        let candidates = if query.is_empty() {
            recommended_candidates(shelf)
        } else {
            match search_candidates(install, query).await {
                Ok(candidates) => candidates,
                Err(note) => {
                    out.line(&note);
                    continue;
                }
            }
        };
        if candidates.is_empty() {
            out.line("every recommended model is already installed — type a name to search");
            continue;
        }

        let mut labels: Vec<String> = candidates
            .iter()
            .map(|candidate| candidate.label.clone())
            .collect();
        labels.push(SEARCH_AGAIN.to_owned());
        let index = interactive::select_index("model", &labels)?;
        // The "search again" row sits past the last candidate, so a `get` miss
        // means "go back to the prompt".
        match candidates.get(index) {
            Some(candidate) => {
                return Ok((candidate.provider.clone(), candidate.reference.clone()));
            }
            None => continue,
        }
    }
}

/// The memory-fit catalog models not already on the shelf, as picker candidates.
fn recommended_candidates(shelf: &[ModelRecord]) -> Vec<Candidate> {
    let installed = installed_names(shelf);
    recommended(None, machine::memory_budget_bytes(), None)
        .into_iter()
        .filter(|entry| !is_installed(&entry.reference, &installed))
        .map(|entry| Candidate {
            label: catalog_label(&entry),
            provider: entry.provider.clone(),
            reference: entry.reference.clone(),
        })
        .collect()
}

/// Hugging Face search hits for `query` as picker candidates, or a note to show
/// and return to the prompt when nothing matched.
async fn search_candidates(
    install: &InstallService,
    query: &str,
) -> Result<Vec<Candidate>, String> {
    let result = install.browse(query, 25).await;
    if result.hits.is_empty() {
        return Err(result
            .failure_hint
            .unwrap_or_else(|| format!("nothing matched \"{query}\"")));
    }
    Ok(result
        .hits
        .iter()
        .map(|hit| Candidate {
            label: hit_label(hit),
            provider: hit.provider.clone(),
            reference: hit.reference.clone(),
        })
        .collect())
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
