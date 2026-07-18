//! The streamed output vocabulary: one `CapabilityChunk` per thing a runtime
//! emits — text, separated thinking, timestamped segments, audio, embedding
//! vectors, free-text status, and a terminal `Done` carrying generation stats.

use serde::{Deserialize, Serialize};

/// A chunk of a raw binary audio payload plus the sample rate it was produced
/// at (captured from the sidecar's ready handshake).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AudioFrame {
    pub data: Vec<u8>,
    pub sample_rate: i64,
}

impl AudioFrame {
    /// An audio frame carrying `data` at `sample_rate` hertz.
    pub fn new(data: Vec<u8>, sample_rate: i64) -> Self {
        Self { data, sample_rate }
    }
}

/// Metrics reported when a generation finishes. Every field is optional — a
/// runtime fills in what it can measure.
#[derive(Debug, Default, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationStats {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub prompt_tokens: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub completion_tokens: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub duration_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub ttft_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub load_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub finish_reason: Option<String>,
    #[serde(default)]
    pub token_counts_estimated: bool,
}

/// One item in a capability stream (chat, vision, speech, transcription,
/// embeddings). The sidecar's tool-call vocabulary is not yet ported, so there
/// is no tool-call variant here.
#[derive(Debug, Clone, PartialEq)]
pub enum CapabilityChunk {
    /// Visible generated text.
    Text(String),
    /// Reasoning the runtime separated from the visible answer.
    Thinking(String),
    /// A transcription segment with its millisecond time span.
    Segment {
        text: String,
        start_ms: i64,
        end_ms: i64,
    },
    /// A raw audio payload at the handshake sample rate.
    Audio(AudioFrame),
    /// An embedding vector.
    Vector(Vec<f64>),
    /// A free-text status notice.
    Status(String),
    /// The terminal marker, carrying stats when the runtime reported any.
    Done(Option<GenerationStats>),
}
