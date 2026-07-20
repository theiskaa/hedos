//! [`DiscoveryService`]: the discovery capstone. It runs the store scanners and
//! reconciles what they found against the registry — updating known records,
//! registering new ones, marking on-disk-gone models missing (guarded by a
//! weights-present check), migrating a moved model's saved config onto its new
//! record, and summarizing the result.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::discovery::duplicates::{
    DEFAULT_THRESHOLD, DuplicateGroup, content_fingerprint, detect,
};
use crate::discovery::scanner::{DiscoveredModel, StoreScanner};
use crate::records::{Modality, ModelRecord, ModelState, SourceKind, format_bytes, stable_id};
use crate::registry::{Registry, RegistryError};

const BYTES_PER_MB: i64 = 1 << 20;

/// The count and byte total of models found for one source kind.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KindStat {
    /// How many models were found.
    pub count: usize,
    /// Their combined on-disk size in bytes.
    pub bytes: i64,
}

/// The outcome of a discovery pass: per-kind counts, totals, duplicate groups,
/// non-fatal issues, and the store kinds whose scan failed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DiscoverySummary {
    /// Per source-kind stats.
    pub per_kind: BTreeMap<SourceKind, KindStat>,
    /// Total models found across all stores.
    pub total_count: usize,
    /// Total bytes across all found models.
    pub total_bytes: i64,
    /// Groups of duplicate weight files.
    pub duplicates: Vec<DuplicateGroup>,
    /// Non-fatal issues surfaced during the scan.
    pub issues: Vec<String>,
    /// Store kinds whose scan failed wholesale.
    pub failed_kinds: Vec<SourceKind>,
}

impl DiscoverySummary {
    /// A one-line, human-readable summary of what was found.
    pub fn headline(&self) -> String {
        if self.total_count == 0 {
            return "No models found on this Mac yet.".to_owned();
        }
        let ordered = [
            (SourceKind::ollama(), "in Ollama"),
            (SourceKind::huggingface_cache(), "in the Hugging Face cache"),
            (SourceKind::lm_studio(), "in LM Studio"),
            (SourceKind::builtin(), "built in"),
        ];
        let mut parts = Vec::new();
        for (kind, label) in ordered {
            if let Some(stat) = self.per_kind.get(&kind)
                && stat.count > 0
            {
                parts.push(format!("{} {label}", stat.count));
            }
        }
        let loose = self.kind_count(&SourceKind::file()) + self.kind_count(&SourceKind::folder());
        if loose == 1 {
            parts.push("1 loose file".to_owned());
        } else if loose > 1 {
            parts.push(format!("{loose} loose files"));
        }
        let models = if self.total_count == 1 {
            "1 model".to_owned()
        } else {
            format!("{} models", self.total_count)
        };
        let breakdown = if parts.is_empty() {
            String::new()
        } else {
            format!(" — {}", parts.join(", "))
        };
        format!(
            "Found {models} on this Mac{breakdown}. Total: {}.",
            format_bytes(self.total_bytes)
        )
    }

    fn kind_count(&self, kind: &SourceKind) -> usize {
        self.per_kind.get(kind).map_or(0, |stat| stat.count)
    }
}

/// Runs a set of scanners and reconciles their findings into a registry.
pub struct DiscoveryService {
    scanners: Vec<Box<dyn StoreScanner>>,
    duplicate_threshold: i64,
}

impl DiscoveryService {
    /// A service over `scanners`, using the default duplicate threshold.
    pub fn new(scanners: Vec<Box<dyn StoreScanner>>) -> Self {
        Self::with_threshold(scanners, DEFAULT_THRESHOLD)
    }

    /// A service with an explicit duplicate-detection size threshold.
    pub fn with_threshold(scanners: Vec<Box<dyn StoreScanner>>, duplicate_threshold: i64) -> Self {
        Self {
            scanners,
            duplicate_threshold,
        }
    }

    /// Scan every store and reconcile the results into `registry`, returning a
    /// summary. Records are updated/inserted; a scanned model no longer on disk
    /// is marked missing; a model whose files moved carries its saved config to
    /// the new record.
    pub fn discover(&self, registry: &mut Registry) -> Result<DiscoverySummary, RegistryError> {
        let mut discovered: Vec<DiscoveredModel> = Vec::new();
        let mut issues: Vec<String> = Vec::new();
        let mut failed_kinds: BTreeSet<SourceKind> = BTreeSet::new();
        for scanner in &self.scanners {
            let result = scanner.scan();
            discovered.extend(result.discovered);
            issues.extend(result.issues);
            failed_kinds.extend(result.failed_kinds);
        }
        for kind in &failed_kinds {
            issues.push(format!(
                "skipped the missing check for {} — its store could not be read",
                kind.as_str()
            ));
        }

        let existing: Vec<ModelRecord> = registry.list().into_iter().cloned().collect();
        let existing_by_id: HashMap<&str, &ModelRecord> = existing
            .iter()
            .map(|record| (record.id.as_str(), record))
            .collect();

        let mut seen_ids: HashSet<String> = HashSet::new();
        let mut to_register: Vec<ModelRecord> = Vec::new();
        let mut new_record_ids: HashSet<String> = HashSet::new();

        for model in &discovered {
            let id = stable_id(&model.source);
            if !seen_ids.insert(id.clone()) {
                continue;
            }
            match existing_by_id.get(id.as_str()) {
                Some(existing_record) => {
                    let mut record = (*existing_record).clone();
                    record.content_fingerprint = fingerprint(model, Some(existing_record));
                    record.name = model.name.clone();
                    record.source = model.source.clone();
                    record.footprint_mb = Some(model.footprint_bytes / BYTES_PER_MB);
                    record.primary_weight_path = model.primary_weight_path.clone();
                    if let Some(modality) = &model.modality_hint {
                        record.modality = modality.clone();
                    }
                    if !model.capabilities_hint.is_empty() {
                        record.capabilities = model.capabilities_hint.clone();
                    }
                    if let Some(context) = model.context_length_hint {
                        record.context_length = Some(context);
                    }
                    if let Some(template) = model.has_chat_template_hint {
                        record.has_chat_template = Some(template);
                    }
                    if model.tool_capable_hint.is_some() {
                        record.supports_tools = model.tool_capable_hint;
                    }
                    if let Some(stops) = &model.stop_tokens_hint {
                        record.stop_tokens = Some(stops.clone());
                    }
                    record.execution = model.execution_hint;
                    record.downloading = model.downloading;
                    if record.state == ModelState::Missing {
                        record.state = ModelState::Unresolved;
                    }
                    to_register.push(record);
                }
                None => {
                    let mut record = ModelRecord::new(
                        &model.name,
                        model
                            .modality_hint
                            .clone()
                            .unwrap_or_else(Modality::unknown),
                        model.capabilities_hint.clone(),
                        model.source.clone(),
                    );
                    record.execution = model.execution_hint;
                    record.footprint_mb = Some(model.footprint_bytes / BYTES_PER_MB);
                    record.state = ModelState::Unresolved;
                    record.context_length = model.context_length_hint;
                    record.has_chat_template = model.has_chat_template_hint;
                    record.supports_tools = model.tool_capable_hint;
                    record.stop_tokens = model.stop_tokens_hint.clone();
                    record.primary_weight_path = model.primary_weight_path.clone();
                    record.downloading = model.downloading;
                    record.content_fingerprint = fingerprint(model, None);
                    to_register.push(record);
                    new_record_ids.insert(id);
                }
            }
        }

        let scanned_kinds: HashSet<SourceKind> = self
            .scanners
            .iter()
            .flat_map(|scanner| scanner.kinds())
            .filter(|kind| !failed_kinds.contains(kind))
            .collect();

        for record in &existing {
            if scanned_kinds.contains(&record.source.kind)
                && !seen_ids.contains(&record.id)
                && record.state != ModelState::Missing
                && !weights_present(record)
            {
                let mut stale = record.clone();
                stale.state = ModelState::Missing;
                to_register.push(stale);
            }
        }

        let missing_candidates: Vec<&ModelRecord> = existing
            .iter()
            .filter(|record| {
                !seen_ids.contains(&record.id)
                    && (record.state == ModelState::Missing
                        || scanned_kinds.contains(&record.source.kind))
            })
            .collect();
        let migrated_away =
            migrate_moved_config(&mut to_register, &new_record_ids, &missing_candidates);

        let keep: Vec<ModelRecord> = to_register
            .into_iter()
            .filter(|record| !migrated_away.contains(&record.id))
            .collect();
        registry.register_all(keep)?;
        for id in &migrated_away {
            registry.unregister(id)?;
        }

        let mut per_kind: BTreeMap<SourceKind, KindStat> = BTreeMap::new();
        for model in &discovered {
            let stat = per_kind.entry(model.source.kind.clone()).or_default();
            stat.count += 1;
            stat.bytes += model.footprint_bytes;
        }
        let duplicate_candidates: Vec<(String, PathBuf)> = discovered
            .iter()
            .filter_map(|model| {
                model
                    .primary_weight_path
                    .as_ref()
                    .map(|path| (model.name.clone(), PathBuf::from(path)))
            })
            .collect();

        Ok(DiscoverySummary {
            total_count: discovered.len(),
            total_bytes: discovered.iter().map(|model| model.footprint_bytes).sum(),
            duplicates: detect(&duplicate_candidates, self.duplicate_threshold),
            per_kind,
            issues,
            failed_kinds: failed_kinds.into_iter().collect(),
        })
    }
}

/// Whether a record's weights are still on disk: its primary weight file if it
/// names one (non-empty), otherwise the source path. A record with no weight
/// path is treated as absent.
fn weights_present(record: &ModelRecord) -> bool {
    let Some(path) = record
        .primary_weight_path
        .as_deref()
        .filter(|path| !path.is_empty())
    else {
        return false;
    };
    Path::new(path).exists() || Path::new(&record.source.path).exists()
}

/// The content fingerprint for a discovered model, reusing the existing record's
/// fingerprint when the weight path and footprint are unchanged (to avoid
/// re-hashing), and preserving it when the model has no weight path.
fn fingerprint(model: &DiscoveredModel, existing: Option<&ModelRecord>) -> Option<String> {
    let Some(path) = &model.primary_weight_path else {
        return existing.and_then(|record| record.content_fingerprint.clone());
    };
    if let Some(existing) = existing
        && let Some(known) = &existing.content_fingerprint
        && existing.primary_weight_path.as_deref() == Some(path.as_str())
        && existing.footprint_mb == Some(model.footprint_bytes / BYTES_PER_MB)
    {
        return Some(known.clone());
    }
    content_fingerprint(Path::new(path))
}

/// Move a newly-found model's saved config (params, system prompt, alias) from a
/// uniquely-matching missing record (same fingerprint and footprint) — the model
/// moved on disk. Returns the ids of the claimed (orphaned) records to remove.
fn migrate_moved_config(
    to_register: &mut [ModelRecord],
    new_record_ids: &HashSet<String>,
    missing_candidates: &[&ModelRecord],
) -> HashSet<String> {
    let mut claimed: HashSet<String> = HashSet::new();
    for record in to_register.iter_mut() {
        if !new_record_ids.contains(&record.id) {
            continue;
        }
        let Some(fingerprint) = record.content_fingerprint.clone() else {
            continue;
        };
        let footprint = record.footprint_mb;
        // Extract the unique orphan's config as owned values, then drop the
        // iterator (it borrows `claimed`) before mutating `claimed` below.
        let orphan = {
            let mut matches = missing_candidates.iter().filter(|candidate| {
                candidate.content_fingerprint.as_deref() == Some(fingerprint.as_str())
                    && candidate.footprint_mb == footprint
                    && !claimed.contains(&candidate.id)
            });
            match (matches.next(), matches.next()) {
                (Some(orphan), None) => Some((
                    orphan.id.clone(),
                    orphan.param_values.clone(),
                    orphan.system_prompt.clone(),
                    orphan.alias.clone(),
                )),
                _ => None,
            }
        };
        let Some((orphan_id, param_values, system_prompt, alias)) = orphan else {
            continue;
        };
        record.param_values = param_values;
        record.system_prompt = system_prompt;
        record.alias = alias;
        claimed.insert(orphan_id);
    }
    claimed
}
