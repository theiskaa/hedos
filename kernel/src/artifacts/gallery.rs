//! Pure arrangement over a list of artifacts: the distinct model list and
//! newest/oldest-first ordering. All comparisons use `(created_at, id)` so ties
//! are total and stable.

use super::artifact::Artifact;

/// Newest-first or oldest-first ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GallerySort {
    NewestFirst,
    OldestFirst,
}

/// A model that owns at least one artifact.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GalleryModel {
    pub id: String,
    pub name: String,
}

/// Gallery arrangement helpers.
pub struct Gallery;

impl Gallery {
    /// The distinct models across `artifacts`, ordered by each model's newest
    /// artifact (dedup keyed on model id, first-seen after a newest-first sort).
    pub fn models(artifacts: &[Artifact]) -> Vec<GalleryModel> {
        let mut sorted: Vec<&Artifact> = artifacts.iter().collect();
        sorted.sort_by(|a, b| newest(a, b));
        let mut seen = std::collections::HashSet::new();
        sorted
            .into_iter()
            .filter(|artifact| seen.insert(artifact.model_id.clone()))
            .map(|artifact| GalleryModel {
                id: artifact.model_id.clone(),
                name: artifact.model.clone(),
            })
            .collect()
    }

    /// Filter `artifacts` to `model_id` (when given) and sort by `sort`.
    pub fn arrange(
        artifacts: &[Artifact],
        model_id: Option<&str>,
        sort: GallerySort,
    ) -> Vec<Artifact> {
        let mut filtered: Vec<Artifact> = artifacts
            .iter()
            .filter(|artifact| model_id.is_none_or(|id| artifact.model_id == id))
            .cloned()
            .collect();
        match sort {
            GallerySort::NewestFirst => filtered.sort_by(newest),
            GallerySort::OldestFirst => filtered.sort_by(|a, b| newest(b, a)),
        }
        filtered
    }
}

/// The newest-first total order (`(created_at, id)` descending) shared by the
/// gallery and the store's listing.
pub(crate) fn newest(a: &Artifact, b: &Artifact) -> std::cmp::Ordering {
    (b.created_at, &b.id).cmp(&(a.created_at, &a.id))
}
