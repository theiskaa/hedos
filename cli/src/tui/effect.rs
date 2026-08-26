//! What the reducer asks the loop to do. The reducer stays pure by returning
//! these instead of performing them.

use super::tasks::TaskKind;

/// A side effect requested by [`crate::tui::app::App::reduce`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Leave the UI.
    Quit,
    /// Start a background task.
    Spawn(TaskKind),
    /// Re-read the shelf and the machine facts.
    Refresh,
}
