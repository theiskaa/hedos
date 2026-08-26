//! The pull modal's state: what was typed, what matched it, and the plan
//! preview before a download starts. Pure, like the rest of the app state; the
//! searches and plans it asks for run as tasks.

use std::collections::HashSet;

use kernel::install::catalog::{InstallCatalogEntry, InstallCategory, recommended};
use kernel::install::plan::{InstallPlan, InstallSearchHit};
use kernel::install::provider::InstallProviderId;
use kernel::install::reference::{hugging_face_repo, ollama_direct_tag};
use kernel::profiles::FitVerdict;
use kernel::records::ModelRecord;
use kernel::records::byte_format::{BYTES_PER_GIB, BYTES_PER_MIB};

use crate::support::install::{installed_names, is_installed};
use crate::support::shelf_table::verdict;

/// How many quiet ticks after a keystroke before the typed query is searched.
pub const SEARCH_DEBOUNCE_TICKS: u64 = 2;
/// The most matches the list keeps; search hits always keep their places.
/// The grouped catalog can fill it: up to three per category.
pub(crate) const MAX_MATCHES: usize = 12;
/// Hugging Face hits requested per search.
pub const SEARCH_LIMIT: usize = 8;
/// How a model of `bytes` fits in `memory_bytes`, when its size is known.
pub fn fit(bytes: Option<i64>, memory_bytes: u64) -> Option<FitVerdict> {
    verdict(bytes.map(|bytes| bytes / BYTES_PER_MIB), memory_bytes)
}

/// The catalog's groups, in the order the list shows them.
pub(crate) const CATEGORIES: [InstallCategory; 4] = [
    InstallCategory::Code,
    InstallCategory::Chat,
    InstallCategory::Voice,
    InstallCategory::Image,
];

/// One installable model the list offers.
#[derive(Debug, Clone, PartialEq)]
pub struct PullMatch {
    pub provider: InstallProviderId,
    pub reference: String,
    /// The size in bytes when the catalog knows it; search hits don't.
    pub bytes: Option<i64>,
    /// A one-line note: the catalog blurb, or the hit's popularity.
    pub note: String,
    /// The catalog group; set only on a blank query, where the list is
    /// grouped, so an eyebrow goes wherever it changes.
    pub category: Option<InstallCategory>,
    /// Whether a pull of it is already running.
    pub pulling: bool,
}

impl PullMatch {
    fn new(provider: InstallProviderId, reference: String, note: String) -> Self {
        Self {
            provider,
            reference,
            bytes: None,
            note,
            category: None,
            pulling: false,
        }
    }

    fn from_catalog(entry: &InstallCatalogEntry, grouped: bool) -> Self {
        Self {
            bytes: Some((entry.size_gb * BYTES_PER_GIB as f64) as i64),
            category: grouped.then_some(entry.category),
            ..Self::new(
                entry.provider.clone(),
                entry.reference.clone(),
                entry.blurb.clone(),
            )
        }
    }

    fn from_hit(hit: &InstallSearchHit) -> Self {
        let mut note = Vec::new();
        if let Some(downloads) = hit.downloads {
            note.push(format!("↓{downloads}"));
        }
        if let Some(likes) = hit.likes {
            note.push(format!("♥{likes}"));
        }
        Self::new(hit.provider.clone(), hit.reference.clone(), note.join("  "))
    }

    /// A row for a reference typed in full: `owner/repo` or `name:tag`. A bare
    /// word is a search, not a tag.
    fn direct(query: &str) -> Option<Self> {
        let (provider, reference) = if let Some(repo) = hugging_face_repo(query) {
            (InstallProviderId::huggingface(), repo)
        } else if let Some(tag) = ollama_direct_tag(query) {
            (InstallProviderId::ollama(), tag)
        } else {
            return None;
        };
        Some(Self::new(provider, reference, "as typed".to_owned()))
    }

    /// How the model fits in `memory_bytes`, when its size is known.
    pub fn fit(&self, memory_bytes: u64) -> Option<FitVerdict> {
        fit(self.bytes, memory_bytes)
    }
}

/// Where the modal is in its flow.
#[derive(Debug, Clone, PartialEq)]
pub enum Stage {
    /// Typing and choosing from the list.
    Listing,
    /// Waiting for the plan of the chosen match.
    Planning(String),
    /// The plan came back; confirm or step back.
    Preview(InstallPlan),
    /// Something to read before going back to the list.
    Note(String),
}

/// The pull modal.
#[derive(Debug, Clone, PartialEq)]
pub struct PullModal {
    pub input: String,
    pub matches: Vec<PullMatch>,
    pub selected: usize,
    pub stage: Stage,
    /// The tick a search of `input` falls due, if one is pending.
    search_due: Option<u64>,
    /// Hits from the last search; dropped on the next edit.
    hits: Vec<PullMatch>,
    installed: HashSet<String>,
    /// Lowercased references with a pull already running, shown but not
    /// choosable.
    pulling: HashSet<String>,
    memory_bytes: u64,
}

/// A line of the listing: a category eyebrow, or a match by its index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListingRow {
    Eyebrow(InstallCategory),
    Match(usize),
}

impl PullModal {
    /// A fresh modal offering the machine's recommendations; `pulling` names
    /// the references already downloading.
    pub fn open(shelf: &[ModelRecord], memory_bytes: u64, pulling: &[String]) -> Self {
        let mut modal = Self {
            input: String::new(),
            matches: Vec::new(),
            selected: 0,
            stage: Stage::Listing,
            search_due: None,
            hits: Vec::new(),
            installed: installed_names(shelf),
            pulling: lowercased(pulling),
            memory_bytes,
        };
        modal.rematch();
        modal
    }

    /// The shelf or the running pulls changed under the open modal.
    pub fn refresh(&mut self, shelf: &[ModelRecord], pulling: &[String]) {
        self.installed = installed_names(shelf);
        self.pulling = lowercased(pulling);
        self.rematch();
    }

    /// The matches with an eyebrow wherever the category changes.
    pub fn rows(&self) -> Vec<ListingRow> {
        let mut rows = Vec::new();
        let mut current = None;
        for (index, candidate) in self.matches.iter().enumerate() {
            if candidate.category.is_some() && candidate.category != current {
                current = candidate.category;
                rows.push(ListingRow::Eyebrow(
                    candidate.category.unwrap_or(InstallCategory::Chat),
                ));
            }
            rows.push(ListingRow::Match(index));
        }
        rows
    }

    /// The highlighted match.
    pub fn selected_match(&self) -> Option<&PullMatch> {
        self.matches.get(self.selected)
    }

    /// Add `c` to the query.
    pub fn type_char(&mut self, c: char, now: u64) {
        self.input.push(c);
        self.edited(now);
    }

    /// Drop the last character of the query.
    pub fn backspace(&mut self, now: u64) {
        self.input.pop();
        self.edited(now);
    }

    fn edited(&mut self, now: u64) {
        self.hits.clear();
        self.rematch();
        self.search_due = (!self.input.trim().is_empty()).then_some(now + SEARCH_DEBOUNCE_TICKS);
    }

    /// The query to search on `now`, once it has sat still long enough.
    pub fn search_due(&mut self, now: u64) -> Option<String> {
        if self.search_due.is_some_and(|due| now >= due) {
            self.search_due = None;
            Some(self.input.trim().to_owned())
        } else {
            None
        }
    }

    /// Fold in the hits for `query`; whether they applied, which they do not
    /// when the query has moved on.
    pub fn searched(&mut self, query: &str, hits: &[InstallSearchHit]) -> bool {
        if query != self.input.trim() {
            return false;
        }
        self.hits = hits.iter().map(PullMatch::from_hit).collect();
        self.rematch();
        true
    }

    /// Move the highlight by `delta` rows.
    pub fn step(&mut self, delta: isize) {
        let last = self.matches.len().saturating_sub(1) as isize;
        self.selected = (self.selected as isize + delta).clamp(0, last) as usize;
    }

    /// Ask for the plan of the highlighted match; the reference to plan, or
    /// why not.
    pub fn choose(&mut self) -> Result<(InstallProviderId, String), String> {
        let chosen = self
            .selected_match()
            .ok_or_else(|| "nothing to pull".to_owned())?
            .clone();
        if chosen.pulling {
            return Err(already_downloading(&chosen.reference));
        }
        self.stage = Stage::Planning(chosen.reference.clone());
        Ok((chosen.provider, chosen.reference))
    }

    /// The plan for `reference` came back.
    pub fn planned(&mut self, reference: &str, result: Result<InstallPlan, String>) {
        if !matches!(&self.stage, Stage::Planning(current) if current == reference) {
            return;
        }
        self.stage = match result {
            Ok(plan) if plan.requires_auth => Stage::Note(format!(
                "{} is gated; add a Hugging Face token first",
                plan.reference
            )),
            Ok(plan) => Stage::Preview(plan),
            Err(reason) => Stage::Note(reason),
        };
    }

    /// Step back from a preview or note to the list.
    pub fn back(&mut self) {
        self.stage = Stage::Listing;
    }

    /// The typed reference itself, the catalog's grouped recommendations
    /// (narrowed by the query when there is one), and the search hits, minus
    /// what is on the shelf already and any repeat.
    fn rematch(&mut self) {
        let typed = self.input.trim();
        let query = typed.to_lowercase();
        let grouped = query.is_empty();
        let catalog_room = MAX_MATCHES.saturating_sub(self.hits.len());
        let mut matches: Vec<PullMatch> = Vec::new();
        matches.extend(PullMatch::direct(typed));
        matches.extend(
            CATEGORIES
                .iter()
                .flat_map(|category| recommended(Some(*category), self.memory_bytes, None))
                .filter(|entry| {
                    grouped
                        || entry.reference.to_lowercase().contains(&query)
                        || entry.name.to_lowercase().contains(&query)
                })
                .map(|entry| PullMatch::from_catalog(&entry, grouped))
                .take(catalog_room),
        );
        matches.extend(self.hits.iter().cloned());
        let mut seen = HashSet::new();
        matches.retain(|candidate| {
            !is_installed(&candidate.reference, &self.installed)
                && seen.insert((
                    candidate.provider.clone(),
                    candidate.reference.to_lowercase(),
                ))
        });
        for candidate in &mut matches {
            candidate.pulling = self.pulling.contains(&candidate.reference.to_lowercase());
        }
        self.matches = matches;
        self.selected = self.selected.min(self.matches.len().saturating_sub(1));
    }
}

/// The notice for a reference that is already being pulled.
pub fn already_downloading(reference: &str) -> String {
    format!("{reference} is already downloading")
}

fn lowercased(references: &[String]) -> HashSet<String> {
    references
        .iter()
        .map(|reference| reference.to_lowercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel::records::{Capability, Modality, ModelSource, SourceKind};

    const MEMORY: u64 = 64 << 30;

    fn hit(reference: &str) -> InstallSearchHit {
        InstallSearchHit {
            provider: InstallProviderId::huggingface(),
            reference: reference.to_owned(),
            name: reference.to_owned(),
            downloads: Some(10),
            likes: None,
            updated_at: None,
        }
    }

    #[test]
    fn a_blank_query_offers_recommendations() {
        let modal = PullModal::open(&[], MEMORY, &[]);
        assert!(!modal.matches.is_empty());
        assert!(modal.matches.iter().all(|m| m.bytes.is_some()));
        assert_eq!(fit(Some(4 << 30), MEMORY), Some(FitVerdict::RunsWell));
    }

    #[test]
    fn installed_models_are_left_out() {
        let first = PullModal::open(&[], MEMORY, &[]).matches[0].clone();
        let record = ModelRecord::new(
            &first.reference,
            Modality::text(),
            vec![Capability::chat()],
            ModelSource::new(SourceKind::ollama(), &first.reference),
        );
        let modal = PullModal::open(&[record], MEMORY, &[]);
        assert!(modal.matches.iter().all(|m| m.reference != first.reference));
    }

    #[test]
    fn a_typed_reference_leads_the_list_and_a_search_falls_due() {
        let mut modal = PullModal::open(&[], MEMORY, &[]);
        for c in "Qwen/Qwen2.5-14B".chars() {
            modal.type_char(c, 0);
        }
        assert_eq!(modal.matches[0].reference, "Qwen/Qwen2.5-14B");
        assert_eq!(modal.matches[0].note, "as typed");
        assert_eq!(modal.search_due(1), None);
        assert_eq!(
            modal.search_due(SEARCH_DEBOUNCE_TICKS),
            Some("Qwen/Qwen2.5-14B".to_owned())
        );
        assert_eq!(modal.search_due(SEARCH_DEBOUNCE_TICKS), None);
    }

    #[test]
    fn hits_apply_only_to_the_current_query() {
        let mut modal = PullModal::open(&[], MEMORY, &[]);
        for c in "smol".chars() {
            modal.type_char(c, 0);
        }
        modal.searched("stale", &[hit("x/stale")]);
        assert!(modal.matches.iter().all(|m| m.reference != "x/stale"));
        modal.searched("smol", &[hit("x/smol-1")]);
        assert!(modal.matches.iter().any(|m| m.reference == "x/smol-1"));
        modal.backspace(5);
        assert!(modal.matches.iter().all(|m| m.reference != "x/smol-1"));
    }

    fn plan(reference: &str, requires_auth: bool) -> InstallPlan {
        InstallPlan {
            provider: InstallProviderId::ollama(),
            reference: reference.to_owned(),
            display_name: reference.to_owned(),
            revision: None,
            files: Vec::new(),
            total_bytes: Some(1 << 30),
            remaining_bytes: Some(1 << 30),
            destination: "~/.ollama".to_owned(),
            requires_auth,
        }
    }

    #[test]
    fn choosing_plans_and_the_plan_moves_to_a_preview() {
        let mut modal = PullModal::open(&[], MEMORY, &[]);
        let (_, reference) = modal.choose().expect("a match");
        assert_eq!(modal.stage, Stage::Planning(reference.clone()));
        modal.planned("other", Err("ignored".to_owned()));
        assert_eq!(modal.stage, Stage::Planning(reference.clone()));
        modal.planned(&reference, Ok(plan(&reference, false)));
        assert_eq!(modal.stage, Stage::Preview(plan(&reference, false)));
        modal.back();
        assert_eq!(modal.stage, Stage::Listing);
    }

    #[test]
    fn a_gated_plan_becomes_a_note() {
        let mut modal = PullModal::open(&[], MEMORY, &[]);
        let (_, reference) = modal.choose().expect("a match");
        modal.planned(&reference, Ok(plan(&reference, true)));
        assert!(matches!(modal.stage, Stage::Note(ref note) if note.contains("gated")));
    }

    #[test]
    fn a_bare_word_is_a_search_not_a_tag() {
        let mut modal = PullModal::open(&[], MEMORY, &[]);
        for c in "smol".chars() {
            modal.type_char(c, 0);
        }
        assert!(modal.matches.iter().all(|m| m.note != "as typed"));
        for c in ":latest".chars() {
            modal.type_char(c, 0);
        }
        assert_eq!(modal.matches[0].reference, "smol:latest");
    }

    #[test]
    fn repeats_and_overflow_never_hide_search_hits() {
        let mut modal = PullModal::open(&[], MEMORY, &[]);
        let first = modal.matches[0].reference.clone();
        let mut hits: Vec<InstallSearchHit> = (0..SEARCH_LIMIT)
            .map(|index| hit(&format!("x/hit-{index}")))
            .collect();
        hits[0].reference = first.to_uppercase();
        hits[0].provider = modal.matches[0].provider.clone();
        modal.searched("", &hits);
        let repeats = modal
            .matches
            .iter()
            .filter(|m| m.reference.eq_ignore_ascii_case(&first))
            .count();
        assert_eq!(repeats, 1);
        assert!(modal.matches.len() <= MAX_MATCHES);
        assert!(modal.matches.iter().any(|m| m.reference == "x/hit-7"));
    }

    #[test]
    fn a_blank_query_is_grouped_by_category_in_order() {
        let modal = PullModal::open(&[], MEMORY, &[]);
        let categories: Vec<InstallCategory> =
            modal.matches.iter().filter_map(|m| m.category).collect();
        let mut order: Vec<usize> = categories
            .iter()
            .map(|c| CATEGORIES.iter().position(|k| k == c).unwrap())
            .collect();
        let sorted = {
            let mut s = order.clone();
            s.sort_unstable();
            s
        };
        assert_eq!(order, sorted);
        order.dedup();
        assert!(order.len() > 1);
        for category in CATEGORIES {
            assert!(
                modal
                    .matches
                    .iter()
                    .filter(|m| m.category == Some(category))
                    .count()
                    <= 3
            );
        }
    }

    #[test]
    fn an_in_flight_pull_is_listed_but_not_choosable() {
        let first = PullModal::open(&[], MEMORY, &[]).matches[0]
            .reference
            .clone();
        let mut modal = PullModal::open(&[], MEMORY, std::slice::from_ref(&first));
        assert!(modal.matches[0].pulling);
        assert!(modal.choose().unwrap_err().contains("already downloading"));
        assert_eq!(modal.stage, Stage::Listing);
    }

    #[test]
    fn stepping_clamps() {
        let mut modal = PullModal::open(&[], MEMORY, &[]);
        modal.step(-3);
        assert_eq!(modal.selected, 0);
        modal.step(100);
        assert_eq!(modal.selected, modal.matches.len() - 1);
    }
}
