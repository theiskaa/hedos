//! What the reducer asks the loop to do. The reducer stays pure by returning
//! these instead of performing them.

use std::path::PathBuf;

use kernel::install::provider::InstallProviderId;
use kernel::records::{JsonValue, ModelRecord};

use super::tasks::{TaskId, TaskKind, TaskLabel};
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
    /// A conversation with a model, through `hedos chat`.
    Chat { record: Box<ModelRecord> },
    /// The gateway in the foreground, through `hedos serve`.
    Serve,
}

impl HandOff {
    /// The label the strip shows once it is over.
    pub fn label(&self, gateway_port: u16) -> TaskLabel {
        match self {
            HandOff::Launch {
                harness, record, ..
            } => TaskLabel {
                verb: "launch",
                subject: format!("{} on {}", harness.display, record.display_name()),
            },
            HandOff::Chat { record } => TaskLabel {
                verb: "chat",
                subject: record.display_name().to_owned(),
            },
            HandOff::Serve => TaskLabel {
                verb: "serve",
                subject: format!(":{gateway_port}"),
            },
        }
    }
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
    /// Resolve an install plan; the number tells its answer from a stale
    /// one's.
    Plan(InstallProviderId, String, u64),
    /// Cancel a running pull.
    Cancel(TaskId),
    /// Put text on the clipboard.
    Copy(String),
    /// Step aside for something that owns the terminal until it ends.
    HandOff(Box<HandOff>),
    /// Stream a chat reply into the pane, reported as [`super::event::Reply`]
    /// events stamped with `generation`.
    Ask {
        record_id: String,
        payload: JsonValue,
        generation: u64,
    },
    /// Stop the reply in flight.
    StopAsk,
}
