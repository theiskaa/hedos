//! Installing models: resolving a user-typed reference to a provider, and the
//! provider-facing error/identity types. The Hugging Face / Ollama fetch providers
//! and the install service build on this foundation.

pub mod error;
pub mod event;
pub mod ollama_pull;
pub mod provider;
pub mod reference;

pub use error::InstallError;
pub use event::{InstallEvent, InstallProgress, InstallStreamEvent};
pub use provider::{InstallAvailability, InstallProviderId};
