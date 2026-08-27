//! Matching an install reference against what the shelf already has, shared
//! by `hedos pull`'s picker and the UI's pull modal.

use std::collections::HashSet;

use kernel::records::ModelRecord;

/// The lowercased ids, names, and display names of every model on the shelf, for
/// matching an install reference against what is already present.
pub(crate) fn installed_names(shelf: &[ModelRecord]) -> HashSet<String> {
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

/// The last path segment of `reference`, lowercased: `org/Model` → `model`.
fn tail(reference: &str) -> String {
    reference
        .rsplit('/')
        .next()
        .unwrap_or(reference)
        .to_lowercase()
}

/// Whether `reference` names a model already on the shelf: a direct match, or a
/// match on its last path segment (so `org/Model` matches an installed `Model`).
pub(crate) fn is_installed(reference: &str, installed: &HashSet<String>) -> bool {
    installed.contains(&reference.to_lowercase()) || installed.contains(&tail(reference))
}

/// The record `reference` names on `shelf`, by the same rule as
/// [`is_installed`]: a direct match on id, name, or display name wins over a
/// match on the last path segment.
pub(crate) fn find_installed<'a>(
    shelf: &'a [ModelRecord],
    reference: &str,
) -> Option<&'a ModelRecord> {
    let names = |record: &'a ModelRecord| {
        [
            record.id.as_str(),
            record.name.as_str(),
            record.display_name(),
        ]
    };
    let exact = shelf.iter().find(|record| {
        names(record)
            .iter()
            .any(|name| name.eq_ignore_ascii_case(reference))
    });
    exact.or_else(|| {
        let tail = tail(reference);
        shelf.iter().find(|record| {
            names(record)
                .iter()
                .any(|name| name.eq_ignore_ascii_case(&tail))
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel::records::{Capability, Modality, ModelSource, SourceKind};

    fn record(name: &str) -> ModelRecord {
        ModelRecord::new(
            name,
            Modality::text(),
            vec![Capability::chat()],
            ModelSource::new(SourceKind::ollama(), name),
        )
    }

    #[test]
    fn a_direct_match_beats_a_tail_match() {
        let shelf = [record("foo"), record("owner/foo")];
        assert_eq!(
            find_installed(&shelf, "owner/foo").map(|r| r.name.as_str()),
            Some("owner/foo")
        );
        assert_eq!(
            find_installed(&shelf, "Other/FOO").map(|r| r.name.as_str()),
            Some("foo")
        );
        assert!(find_installed(&shelf, "bar").is_none());
    }
}
