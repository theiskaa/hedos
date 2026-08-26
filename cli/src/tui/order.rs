//! The order the shelf is shown in: a fuzzy filter over the records and a
//! sort key, applied together to give the row indices to draw.

use kernel::records::{Capability, ModelRecord};
use serde::{Deserialize, Serialize};

use super::facts::Facts;

/// What the shelf is sorted by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sort {
    /// Shelf order, which is by name.
    #[default]
    Name,
    /// Largest footprint first.
    Size,
    /// Most recently requested first.
    LastUsed,
    /// Loaded models first, then shelf order.
    WarmFirst,
}

impl Sort {
    /// The sort after this one in the `o` cycle.
    pub fn next(self) -> Self {
        match self {
            Sort::Name => Sort::Size,
            Sort::Size => Sort::LastUsed,
            Sort::LastUsed => Sort::WarmFirst,
            Sort::WarmFirst => Sort::Name,
        }
    }

    /// The label the shelf title shows.
    pub fn label(self) -> &'static str {
        match self {
            Sort::Name => "name",
            Sort::Size => "size",
            Sort::LastUsed => "last used",
            Sort::WarmFirst => "warm first",
        }
    }
}

/// Whether `query` matches `haystack` as a case-insensitive subsequence.
pub fn fuzzy(query: &str, haystack: &str) -> bool {
    let mut chars = haystack.chars().flat_map(char::to_lowercase);
    query
        .chars()
        .flat_map(char::to_lowercase)
        .all(|wanted| chars.any(|c| c == wanted))
}

/// Whether `record` matches `query` on any of its visible fields. The id is
/// lowercase hex, so it only matches as a substring of a lowercased query; a
/// fuzzy `b` would match every id.
pub fn matches(record: &ModelRecord, query: &str) -> bool {
    if query.is_empty() || record.id.contains(&query.to_lowercase()) {
        return true;
    }
    [
        record.display_name(),
        record.name.as_str(),
        record.source.kind.as_str(),
    ]
    .into_iter()
    .chain(record.runtime.id.as_ref().map(|id| id.as_str()))
    .chain(record.capabilities.iter().map(Capability::as_str))
    .any(|field| fuzzy(query, field))
}

/// The indices into `records` to show, filtered by `query` and ordered by
/// `sort`; the sort is stable, so ties keep shelf order.
pub fn order(records: &[ModelRecord], facts: &Facts, query: &str, sort: Sort) -> Vec<usize> {
    let mut indices: Vec<usize> = (0..records.len())
        .filter(|&index| matches(&records[index], query))
        .collect();
    match sort {
        Sort::Name => {}
        Sort::Size => indices
            .sort_by_key(|&index| std::cmp::Reverse(records[index].footprint_mb.unwrap_or(0))),
        Sort::LastUsed => indices.sort_by_key(|&index| {
            std::cmp::Reverse(
                facts
                    .activity
                    .for_record(&records[index])
                    .map_or(0, |activity| activity.last_seen_millis),
            )
        }),
        Sort::WarmFirst => indices.sort_by_key(|&index| !facts.is_warm(&records[index].id)),
    }
    indices
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::facts::{Holder, Resident};
    use kernel::records::{Modality, ModelSource, SourceKind};

    fn record(name: &str, footprint_mb: Option<i64>) -> ModelRecord {
        let mut record = ModelRecord::new(
            name,
            Modality::text(),
            vec![Capability::chat()],
            ModelSource::new(SourceKind::ollama(), name),
        );
        record.footprint_mb = footprint_mb;
        record
    }

    #[test]
    fn fuzzy_is_a_case_insensitive_subsequence() {
        assert!(fuzzy("qwn", "Qwen2.5-coder"));
        assert!(fuzzy("", "anything"));
        assert!(!fuzzy("qwx", "Qwen"));
        assert!(fuzzy("LLAMA", "llama3.1:8b"));
    }

    #[test]
    fn matching_looks_at_names_store_runtime_and_caps() {
        let record = record("gemma", Some(1));
        assert!(matches(&record, "gem"));
        assert!(matches(&record, "ollama"));
        assert!(matches(&record, "chat"));
        assert!(!matches(&record, "whisper"));
    }

    #[test]
    fn sorts_are_stable_and_reverse_where_it_reads_naturally() {
        let records = [
            record("alpha", Some(1)),
            record("bravo", Some(3)),
            record("charlie", Some(3)),
        ];
        let mut facts = Facts::default();
        assert_eq!(order(&records, &facts, "", Sort::Name), vec![0, 1, 2]);
        assert_eq!(order(&records, &facts, "", Sort::Size), vec![1, 2, 0]);
        facts.residents.push(Resident {
            id: records[2].id.clone(),
            name: "charlie".into(),
            bytes: 0,
            holder: Holder::Local,
            expires_at_millis: None,
        });
        assert_eq!(order(&records, &facts, "", Sort::WarmFirst), vec![2, 0, 1]);
        assert_eq!(order(&records, &facts, "brv", Sort::Name), vec![1]);
    }

    #[test]
    fn the_cycle_returns_to_name() {
        let mut sort = Sort::Name;
        for _ in 0..4 {
            sort = sort.next();
        }
        assert_eq!(sort, Sort::Name);
    }
}
