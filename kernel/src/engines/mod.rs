//! The engines hedos can install for a person who asks, and the one it carries
//! inside itself: what it pins of each, and how it reads one that is installed.

pub mod bundle;
pub mod llama_cpp;
pub mod uv;

use serde::{Deserialize, Serialize};

/// The file beside an engine hedos installed that says what was installed.
pub const INSTALLED_FILE: &str = "installed.json";

/// One file of an engine hedos keeps, as it was put in place.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledFile {
    /// Its name in the engine's directory.
    pub name: String,
    /// Its size in bytes; `None` for a link to another file beside it.
    pub bytes: Option<u64>,
}
