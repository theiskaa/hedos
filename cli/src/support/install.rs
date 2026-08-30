//! Matching an install reference against what the shelf already has, shared
//! by `hedos pull`'s picker and the UI's pull modal.

use std::collections::HashSet;

use kernel::records::{ModelRecord, ModelState};

/// The records on `shelf` whose weights are still on disk: a record whose
/// weights are gone can be pulled again, so it does not count as installed.
fn present(shelf: &[ModelRecord]) -> impl Iterator<Item = &ModelRecord> {
    shelf
        .iter()
        .filter(|record| record.state != ModelState::Missing)
}

/// The lowercased ids, names, and display names of every model on the shelf
/// whose weights are present, for matching an install reference against what
/// is already there.
pub(crate) fn installed_names(shelf: &[ModelRecord]) -> HashSet<String> {
    present(shelf)
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
/// match on the last path segment, and a record whose weights are gone is
/// never named.
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
    let exact = present(shelf).find(|record| {
        names(record)
            .iter()
            .any(|name| name.eq_ignore_ascii_case(reference))
    });
    exact.or_else(|| {
        let tail = tail(reference);
        present(shelf).find(|record| {
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

    #[test]
    fn a_gone_record_is_not_installed() {
        let mut gone = record("foo");
        gone.state = ModelState::Missing;
        let shelf = [gone, record("bar")];
        let installed = installed_names(&shelf);
        assert!(!is_installed("foo", &installed));
        assert!(!is_installed("owner/foo", &installed));
        assert!(is_installed("bar", &installed));
        assert!(find_installed(&shelf, "foo").is_none());
        assert!(find_installed(&shelf, "owner/foo").is_none());
    }
}
