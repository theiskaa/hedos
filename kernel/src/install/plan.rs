//! The plan for installing a model: which files will be fetched, and where.

use crate::install::event::InstallProgress;
use crate::install::file_selection::is_weight_path;
use crate::install::provider::InstallProviderId;

/// One file an install will fetch, with its size when the provider reported it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct InstallPlanFile {
    /// The file's path within the repository.
    pub path: String,
    /// The file's size in bytes, if known.
    pub bytes: Option<i64>,
}

impl InstallPlanFile {
    /// A plan file for `path`.
    pub fn new(path: impl Into<String>, bytes: Option<i64>) -> Self {
        Self {
            path: path.into(),
            bytes,
        }
    }

    /// Whether this file is a model weight (by extension).
    pub fn is_weight(&self) -> bool {
        is_weight_path(&self.path)
    }
}

/// A resolved install: the provider and reference, the files to fetch (and their
/// total/remaining bytes), where they land, and whether authentication is needed.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct InstallPlan {
    /// The provider that will fetch the model.
    pub provider: InstallProviderId,
    /// The reference being installed (repo or tag).
    pub reference: String,
    /// The name to show the user.
    pub display_name: String,
    /// The resolved revision/commit, if pinned.
    pub revision: Option<String>,
    /// The files to fetch.
    pub files: Vec<InstallPlanFile>,
    /// The total bytes across all files, if known.
    pub total_bytes: Option<i64>,
    /// The bytes still to fetch (total minus what's already on disk), if known.
    pub remaining_bytes: Option<i64>,
    /// Where the model will be written.
    pub destination: String,
    /// Whether the model is gated and needs a token.
    pub requires_auth: bool,
}

/// A search result: a model the user could install, with its popularity signals.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct InstallSearchHit {
    /// The provider that would install it.
    pub provider: InstallProviderId,
    /// The reference (repo or tag).
    pub reference: String,
    /// The name to show (the last path segment, usually).
    pub name: String,
    /// The download count, if reported.
    pub downloads: Option<i64>,
    /// The like count, if reported.
    pub likes: Option<i64>,
    /// When it was last updated, epoch milliseconds.
    pub updated_at: Option<i64>,
}

impl InstallSearchHit {
    /// A stable id: `provider|reference`.
    pub fn id(&self) -> String {
        format!("{}|{}", self.provider.as_str(), self.reference)
    }
}

impl InstallPlan {
    /// A plan with the required fields; the optional fields default to empty.
    pub fn new(
        provider: InstallProviderId,
        reference: impl Into<String>,
        display_name: impl Into<String>,
        destination: impl Into<String>,
    ) -> Self {
        Self {
            provider,
            reference: reference.into(),
            display_name: display_name.into(),
            revision: None,
            files: Vec::new(),
            total_bytes: None,
            remaining_bytes: None,
            destination: destination.into(),
            requires_auth: false,
        }
    }
}

/// The outcome of browsing for a model: the hits found, plus a hint when the
/// lookup failed (kept separate so a partial/failed browse can still show hits).
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct InstallBrowseResult {
    /// The models found.
    pub hits: Vec<InstallSearchHit>,
    /// Why the browse failed, when it did.
    pub failure_hint: Option<String>,
}

impl InstallBrowseResult {
    /// A result carrying `hits` and no failure.
    pub fn with_hits(hits: Vec<InstallSearchHit>) -> Self {
        Self {
            hits,
            failure_hint: None,
        }
    }

    /// A failed browse carrying only a hint.
    pub fn failure(hint: impl Into<String>) -> Self {
        Self {
            hits: Vec::new(),
            failure_hint: Some(hint.into()),
        }
    }
}

/// A running install: identity, what's being fetched, live progress, and when it
/// started (epoch milliseconds).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ActiveInstall {
    /// The install's opaque id.
    pub id: String,
    /// The provider fetching it.
    pub provider: InstallProviderId,
    /// The reference being installed.
    pub reference: String,
    /// The name to show.
    pub display_name: String,
    /// The total to download, if known.
    pub total_bytes: Option<i64>,
    /// The latest progress.
    pub progress: InstallProgress,
    /// When it started, epoch milliseconds.
    pub started_at: i64,
}

impl ActiveInstall {
    /// A fresh install at zero progress, started at `started_at` (epoch millis).
    pub fn new(
        id: impl Into<String>,
        provider: InstallProviderId,
        reference: impl Into<String>,
        display_name: impl Into<String>,
        total_bytes: Option<i64>,
        started_at: i64,
    ) -> Self {
        Self {
            id: id.into(),
            provider,
            reference: reference.into(),
            display_name: display_name.into(),
            total_bytes,
            progress: InstallProgress::default(),
            started_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_active_install_starts_at_zero_progress() {
        let active = ActiveInstall::new(
            "in-1",
            InstallProviderId::huggingface(),
            "org/Model",
            "Model",
            Some(1000),
            42,
        );
        assert_eq!(active.progress, InstallProgress::default());
        assert_eq!(active.progress.bytes_downloaded, 0);
        assert_eq!(active.total_bytes, Some(1000));
        assert_eq!(active.started_at, 42);
    }

    #[test]
    fn a_browse_result_separates_hits_from_a_failure() {
        assert!(InstallBrowseResult::default().hits.is_empty());
        assert_eq!(
            InstallBrowseResult::failure("down").failure_hint.as_deref(),
            Some("down")
        );
        assert!(InstallBrowseResult::failure("down").hits.is_empty());
    }
}
