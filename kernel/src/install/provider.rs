//! The install-provider identity and availability types.

use serde::{Deserialize, Serialize};

/// Which install backend fetches a model: Hugging Face or Ollama.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct InstallProviderId(String);

impl InstallProviderId {
    /// The Hugging Face provider.
    pub fn huggingface() -> Self {
        Self("huggingface".to_owned())
    }

    /// The Ollama provider.
    pub fn ollama() -> Self {
        Self("ollama".to_owned())
    }

    /// The underlying identifier string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for InstallProviderId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl std::fmt::Display for InstallProviderId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Whether an install provider can run right now.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum InstallAvailability {
    /// Ready to install.
    Ready,
    /// Not usable, with a reason to show the user.
    Unavailable {
        /// Why the provider can't be used.
        hint: String,
    },
}
