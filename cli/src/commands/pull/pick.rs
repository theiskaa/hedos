//! Choosing what to pull when the command line did not say.
//!
//! With a reference given this is a shape test and nothing else. Without one it
//! is a search prompt over Hugging Face, or the models that fit this machine's
//! memory, which is the one path here that needs a shelf and therefore a kernel.

use kernel::install::{InstallCatalogEntry, InstallProviderId, InstallSearchHit, recommended};
use kernel::records::ModelRecord;
use runtime::install::service::InstallService;

use crate::error::CliError;
use crate::support::install::{installed_names, is_installed};
use crate::support::output::Out;
use crate::support::session::Session;
use crate::support::{interactive, machine};

use super::{PullArgs, provider_for};

/// The provider and reference to install: taken from the argument, or chosen
/// through an interactive search when no reference was given.
pub(super) async fn target(
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
            "no reference given. pass one (org/model or name:tag), or run in a terminal to search",
        ));
    }
    // Only this path needs the shelf, to leave out what is already installed.
    let session = Session::open()?;
    let shelf = session.shelf().await;
    interactive_pick(out, install, &shelf).await
}

/// One installable option offered by the interactive picker.
struct Candidate {
    label: String,
    provider: InstallProviderId,
    reference: String,
}

/// The interactive install picker: a search prompt that flows into a list of
/// results, or the memory-fit recommendations when the query is blank, with a
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
            let recommendations = recommended_candidates(shelf);
            if recommendations.is_empty() {
                out.line("every recommended model is already installed — type a name to search");
                continue;
            }
            recommendations
        } else {
            match search_candidates(install, query).await {
                Ok(candidates) => candidates,
                Err(note) => {
                    out.line(&note);
                    continue;
                }
            }
        };

        let mut labels: Vec<String> = candidates
            .iter()
            .map(|candidate| candidate.label.clone())
            .collect();
        labels.push(SEARCH_AGAIN.to_owned());
        let index = interactive::select_index("model", &labels)?;
        // The "search again" row sits past the last candidate, so an out-of-range
        // index (that row) yields `None` and drops back to the prompt.
        match candidates.into_iter().nth(index) {
            Some(candidate) => return Ok((candidate.provider, candidate.reference)),
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
