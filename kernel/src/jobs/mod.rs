//! Jobs: discrete non-conversational units of work (image generation and other
//! generators) with progress and a persisted terminal result. This module holds
//! the pure data model, the seed injection, and the terminal-job history store;
//! the async scheduler that drives runners lives in the `runtime` crate.

mod event;
mod history;
mod job;
mod seeding;

pub use event::{JobEvent, JobRuntimeEvent};
pub use history::JobHistoryStore;
pub use job::{Job, JobProgress, JobState};
pub use seeding::{reseeded, seeded};
