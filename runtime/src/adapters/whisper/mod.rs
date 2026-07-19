//! The whisper.cpp transcription runtime: turning a transcribe request's audio
//! into text. The audio decoding and request options live here; the governed
//! engine, its backend, and the adapter build on them.

mod audio;
mod options;

pub use audio::{TranscriptionAudio, TranscriptionError};
pub use options::{TranscriptionOptions, TranscriptionSegment};
