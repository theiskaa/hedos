//! The shared session: open a production kernel from the user's data dir and
//! settings, list the shelf (discovering on first use), and resolve a
//! model-name query to a record.

use std::collections::HashSet;
use std::sync::Arc;

use kernel::discovery::{DiscoverySummary, ModelHabitat};
use kernel::records::{Capability, ModelRecord};
use runtime::boot::{self, HedosDirs};
use runtime::facade::Kernel;
use runtime::settings::{Settings, SettingsStore};

use crate::error::CliError;

/// An open kernel plus the settings and directories it was built from.
pub struct Session {
    /// The production kernel.
    pub kernel: Kernel,
    /// The loaded settings.
    pub settings: Settings,
    /// The data directories.
    pub dirs: HedosDirs,
}

impl Session {
    /// Build the production kernel from the detected data dir + settings file.
    pub fn open() -> Result<Self, CliError> {
        let dirs = HedosDirs::detect();
        let settings = SettingsStore::discover().load();
        let kernel = boot::build_kernel(&dirs, &settings)?;
        Ok(Self {
            kernel,
            settings,
            dirs,
        })
    }

    /// The current shelf, exactly as registered — a shared snapshot; cloning it
    /// is a refcount bump, not a deep copy of every record.
    pub async fn shelf(&self) -> Arc<[ModelRecord]> {
        self.kernel.shelf().await
    }

    /// The shelf, running discovery first when it is empty so a fresh install
    /// still finds the models already on disk.
    pub async fn shelf_or_discover(&self) -> Result<Arc<[ModelRecord]>, CliError> {
        let shelf = self.kernel.shelf().await;
        if !shelf.is_empty() {
            return Ok(shelf);
        }
        self.discover().await?;
        Ok(self.kernel.shelf().await)
    }

    /// The ids of the models currently held resident, for the warm marker.
    pub fn warm_set(&self) -> HashSet<String> {
        self.kernel
            .resident_models()
            .into_iter()
            .filter_map(|entry| entry.model_id)
            .collect()
    }

    /// Scan the machine's model stores, reconcile the registry, and resolve
    /// runtimes — returning the discovery summary.
    pub async fn discover(&self) -> Result<DiscoverySummary, CliError> {
        let scanners =
            ModelHabitat::detect().scanners(None, &boot::discovery_settings(&self.settings));
        Ok(self.kernel.discover(scanners).await?)
    }
}

/// Resolve a model-name `query` against `shelf`, optionally filtered by
/// `capability`: exact id, then exact case-insensitive name, then a unique
/// substring. Ambiguity or no match is a user-facing error.
///
/// This is registry-shaped matching, not session state; a future kernel/registry
/// resolver (which the TUI would share) is the natural home — it lives here for
/// now because no such kernel entry point exists yet.
pub fn resolve<'a>(
    query: &str,
    shelf: &'a [ModelRecord],
    capability: Option<&Capability>,
) -> Result<&'a ModelRecord, CliError> {
    let candidates: Vec<&ModelRecord> = shelf
        .iter()
        .filter(|record| capability.is_none_or(|cap| record.capabilities.contains(cap)))
        .collect();

    if let Some(record) = candidates.iter().find(|record| record.id == query) {
        return Ok(record);
    }

    let names_match = |record: &ModelRecord| {
        record.display_name().eq_ignore_ascii_case(query) || record.name.eq_ignore_ascii_case(query)
    };
    if let Some(record) = unique(query, &candidates, names_match)? {
        return Ok(record);
    }

    let lowered = query.to_lowercase();
    let contains = |record: &ModelRecord| {
        record.display_name().to_lowercase().contains(&lowered)
            || record.name.to_lowercase().contains(&lowered)
            || record.id.to_lowercase().contains(&lowered)
    };
    if let Some(record) = unique(query, &candidates, contains)? {
        return Ok(record);
    }

    let serving = capability.map_or_else(String::new, |cap| format!(" serving {}", cap.as_str()));
    Err(CliError::new(format!(
        "no model{serving} matched \"{query}\" — run `hedos ls`"
    )))
}

/// The single record matching `predicate`, `None` if none match, or an ambiguity
/// error naming `query` and listing up to 8 matches if more than one does.
fn unique<'a>(
    query: &str,
    candidates: &[&'a ModelRecord],
    predicate: impl Fn(&ModelRecord) -> bool,
) -> Result<Option<&'a ModelRecord>, CliError> {
    let matched: Vec<&ModelRecord> = candidates
        .iter()
        .copied()
        .filter(|record| predicate(record))
        .collect();
    match matched.as_slice() {
        [] => Ok(None),
        [only] => Ok(Some(only)),
        many => {
            let listing = many
                .iter()
                .take(8)
                .map(|record| format!("{} · {}", record.id, record.display_name()))
                .collect::<Vec<_>>()
                .join("\n  ");
            Err(CliError::new(format!(
                "\"{query}\" is ambiguous — matched:\n  {listing}"
            )))
        }
    }
}

#[cfg(test)]
mod resolve_tests {
    use super::*;
    use kernel::records::{Modality, ModelSource, SourceKind};

    fn model(name: &str, capabilities: Vec<Capability>) -> ModelRecord {
        ModelRecord::new(
            name,
            Modality::text(),
            capabilities,
            ModelSource::new(SourceKind::ollama(), name),
        )
    }

    fn shelf() -> Vec<ModelRecord> {
        vec![
            model("qwen3", vec![Capability::chat(), Capability::tools()]),
            model("qwen3-mini", vec![Capability::chat()]),
            model("gemma3", vec![Capability::chat()]),
        ]
    }

    #[test]
    fn exact_id_matches_regardless_of_query_casing() {
        let shelf = shelf();
        let target = &shelf[1];
        let found = resolve(&target.id, &shelf, None).expect("exact id should resolve");
        assert_eq!(found.id, target.id);
    }

    #[test]
    fn exact_name_match_is_case_insensitive() {
        let shelf = shelf();
        let found = resolve("QWEN3", &shelf, None).expect("exact name should resolve");
        assert_eq!(found.name, "qwen3");
    }

    #[test]
    fn unique_substring_resolves() {
        let shelf = shelf();
        let found = resolve("gemma", &shelf, None).expect("unique substring should resolve");
        assert_eq!(found.name, "gemma3");
    }

    #[test]
    fn ambiguous_substring_is_an_error() {
        let shelf = shelf();
        let error = resolve("qwen", &shelf, None).expect_err("ambiguous query should error");
        assert!(error.message.to_lowercase().contains("ambiguous"));
    }

    #[test]
    fn no_match_is_an_error() {
        let shelf = shelf();
        assert!(resolve("nonexistent-model", &shelf, None).is_err());
    }

    #[test]
    fn capability_filter_excludes_records_lacking_it() {
        let shelf = shelf();
        assert!(resolve("qwen3-mini", &shelf, None).is_ok());
        assert!(resolve("qwen3-mini", &shelf, Some(&Capability::tools())).is_err());
    }
}
