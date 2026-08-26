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

/// Whether `reference` names a model already on the shelf: a direct match, or a
/// match on its last path segment (so `org/Model` matches an installed `Model`).
pub(crate) fn is_installed(reference: &str, installed: &HashSet<String>) -> bool {
    let reference = reference.to_lowercase();
    installed.contains(&reference)
        || installed.contains(reference.rsplit('/').next().unwrap_or(&reference))
}
