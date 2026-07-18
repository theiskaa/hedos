//! Where an installed runtime came from — the `.provenance.json` the installer
//! stamps beside a manifest so the loader can tell community runtimes (which
//! must run contained) from first-party ones.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::persistence::{self, StoreError};
use crate::util::now_millis;

/// The origin marking a community-installed runtime.
pub const COMMUNITY_ORIGIN: &str = "community";
const FILE_NAME: &str = ".provenance.json";

/// The provenance of an installed runtime.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RuntimeProvenance {
    pub origin: String,
    /// Install time, epoch milliseconds.
    pub installed_at: i64,
}

impl RuntimeProvenance {
    /// A provenance for `origin`, stamped installed-now.
    pub fn new(origin: impl Into<String>) -> Self {
        Self {
            origin: origin.into(),
            installed_at: now_millis(),
        }
    }

    /// A provenance marking a community install.
    pub fn community() -> Self {
        Self::new(COMMUNITY_ORIGIN)
    }

    /// Whether this marks a community runtime (which must run contained).
    pub fn is_community(&self) -> bool {
        self.origin == COMMUNITY_ORIGIN
    }

    /// Read the provenance from `directory`, or `None` if absent or unreadable.
    /// Unlike the quarantining store reads, a malformed provenance is left in
    /// place (reading it must not mutate the runtime's directory).
    pub fn read(directory: &Path) -> Option<Self> {
        let bytes = std::fs::read(directory.join(FILE_NAME)).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    /// Write the provenance into `directory`.
    pub fn write(&self, directory: &Path) -> Result<(), StoreError> {
        persistence::write_json_atomic(&directory.join(FILE_NAME), self)
    }
}
