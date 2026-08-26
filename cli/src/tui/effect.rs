//! What the reducer asks the loop to do. The reducer stays pure by returning
//! these instead of performing them.

use std::path::PathBuf;

use kernel::install::provider::InstallProviderId;
use kernel::records::ModelRecord;

use super::tasks::{TaskId, TaskKind};
use crate::support::harnesses::HarnessSpec;

/// Something that needs the terminal for a while: the UI steps aside, it
/// runs, and the UI comes back when it ends.
#[derive(Debug, Clone, PartialEq)]
pub enum HandOff {
    /// A coding harness on a model, through `hedos launch`.
    Launch {
        harness: &'static HarnessSpec,
        program: PathBuf,
        record: Box<ModelRecord>,
    },
}

/// A side effect requested by [`crate::tui::app::App::reduce`].
#[derive(Debug, Clone, PartialEq)]
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
    /// Step aside for something that owns the terminal until it ends.
    HandOff(Box<HandOff>),
}
