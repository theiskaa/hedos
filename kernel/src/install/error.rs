//! Install failures.

use crate::install::provider::InstallProviderId;
use crate::records::format_bytes;

/// Why a model install could not proceed or complete.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InstallError {
    /// No provider is registered under this id.
    #[error("No install provider is registered as {0}.")]
    ProviderUnknown(InstallProviderId),
    /// The provider exists but can't be used (with a reason).
    #[error("{0}")]
    ProviderUnavailable(String),
    /// The reference isn't one this provider understands.
    #[error("{0} is not a reference this provider understands.")]
    ReferenceInvalid(String),
    /// The reference was not found on the platform.
    #[error("{0} was not found on the platform.")]
    ReferenceNotFound(String),
    /// The model is gated: either no token was supplied, or the token that was
    /// supplied has not been granted access to this repo.
    #[error(
        "{0} is gated. Accept its terms and request access at https://huggingface.co/{0}, then set HF_TOKEN to a token with read access (or sign in with `huggingface-cli login`)."
    )]
    AuthRequired(String),
    /// Not enough free disk space to complete the download.
    #[error(
        "Not enough free disk space: this model needs {} and {} is available.",
        format_bytes(*required_bytes),
        format_bytes(*available_bytes)
    )]
    InsufficientDisk {
        /// Bytes the model needs.
        required_bytes: i64,
        /// Bytes currently free.
        available_bytes: i64,
    },
    /// A downloaded file failed checksum verification.
    #[error("{0} failed checksum verification after download.")]
    ChecksumMismatch(String),
    /// The transfer failed with a message.
    #[error("{0}")]
    TransferFailed(String),
}
