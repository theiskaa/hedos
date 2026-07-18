//! The plan for installing a model: which files will be fetched, and where.

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
