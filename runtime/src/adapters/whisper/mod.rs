//! The whisper.cpp transcription runtime: turning a transcribe request's audio
//! into text. Audio decoding and request options, the loadable backend, the
//! governed engine, and the adapter.

mod adapter;
mod audio;
mod backend;
mod engine;
mod options;

use std::path::Path;

pub use adapter::WhisperCppAdapter;
pub use audio::{TranscriptionAudio, TranscriptionError};
pub use backend::{MissingWhisperBackend, SidecarWhisperBackend, WhisperBackend};
pub use engine::{TranscriptionJob, WhisperEngine};
pub use options::{TranscriptionOptions, TranscriptionSegment};

/// Expand a leading `~` or `~/` in `path` against `$HOME`, leaving it unchanged
/// if `$HOME` is unset or the path is not tilde-prefixed.
pub(super) fn expand_tilde(path: &str) -> String {
    match std::env::var("HOME") {
        Ok(home) => kernel::fs::expand_tilde(path, Path::new(&home))
            .to_string_lossy()
            .into_owned(),
        Err(_) => path.to_owned(),
    }
}
