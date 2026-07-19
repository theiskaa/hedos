//! Transcription request options and the segment a backend streams back.

/// The knobs a transcription request carries: an optional forced language and
/// whether to translate the audio to English rather than transcribe verbatim.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct TranscriptionOptions {
    /// The forced language (e.g. `"en"`), or `None` to auto-detect.
    pub language: Option<String>,
    /// Translate to English instead of transcribing in the source language.
    pub translate: bool,
}

/// One transcribed span a backend emits: its text and, when the backend timed it,
/// the millisecond start/end offsets into the audio.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct TranscriptionSegment {
    /// The transcribed text.
    pub text: String,
    /// The segment's start offset in milliseconds, if timed.
    pub start_ms: Option<i64>,
    /// The segment's end offset in milliseconds, if timed.
    pub end_ms: Option<i64>,
}
