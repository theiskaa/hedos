//! Installing models: resolving a user-typed reference to a provider, and the
//! provider-facing error/identity types. The Hugging Face / Ollama fetch providers
//! and the install service build on this foundation.

mod bytes;
pub mod catalog;
pub mod error;
pub mod event;
pub mod file_selection;
pub mod installed;
pub mod ollama_pull;
pub mod plan;
pub mod provider;
pub mod pulls;
pub mod recommend;
pub mod reference;

pub use catalog::{InstallCatalogEntry, InstallCategory};
pub use error::InstallError;
pub use event::{InstallEvent, InstallProgress, InstallStreamEvent};
pub use file_selection::{HFSibling, file_extension, is_weight_path, select};
pub use plan::{
    ActiveInstall, InstallBrowseResult, InstallPlan, InstallPlanFile, InstallSearchHit,
};
pub use provider::{InstallAvailability, InstallProviderId};
pub use pulls::{
    PullControl, PullError, PullEvent, PullEventKind, PullJob, PullJobDir, PullLock, PullReading,
    PullState, PullStatus, PullStore, StopAnswer, take_lock,
};
