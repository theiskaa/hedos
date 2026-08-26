//! What the reducer asks the loop to do. The reducer stays pure by returning
//! these instead of performing them.

use kernel::install::provider::InstallProviderId;

use super::tasks::{TaskId, TaskKind};

/// A side effect requested by [`crate::tui::app::App::reduce`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Leave the UI.
    Quit,
    /// Start a background task.
    Spawn(TaskKind),
    /// Re-read the shelf and the machine facts.
    Refresh,
    /// Search the providers for a query.
    Search(String),
    /// Resolve an install plan.
    Plan(InstallProviderId, String),
    /// Cancel a running pull.
    Cancel(TaskId),
    /// Put text on the clipboard.
    Copy(String),
}
