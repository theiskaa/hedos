//! Context-window budgeting: estimating prompt token cost, deciding whether a
//! request fits, and clamping the completion length to what remains.

use crate::profiles::configuration::normalized_param_values;
use crate::records::{JsonValue, ModelRecord, RuntimeId, SourceKind};

/// The number of tokens reserved for the completion when deciding if a prompt
/// fits the window, wherever the window can afford it. A window too small for
/// it reserves half of itself instead, so a model declaring fewer than twice
/// this is still usable.
pub const COMPLETION_FLOOR: i64 = 256;

/// The outcome of assessing a prompt against a context window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// The prompt fits; `clamped_max_tokens` is the completion length capped to
    /// what the window leaves free.
    Fits {
        /// The completion length after clamping to the remaining window.
        clamped_max_tokens: Option<i64>,
    },
    /// The prompt does not fit the window.
    Exceeds {
        /// The estimated token cost of the prompt.
        estimated: i64,
        /// The window it exceeded.
        window: i64,
    },
}

/// A rough token estimate for `characters` of text (about four characters per
/// token).
pub fn estimated_tokens(characters: i64) -> i64 {
    (characters + 3) / 4
}

/// The tokens held back for the reply out of a window of `window`: the
/// [`COMPLETION_FLOOR`], or half the window when the window cannot afford the
/// floor. A model declaring fewer than twice the floor would otherwise be
/// refused every prompt it was ever given, however short.
///
/// At least one token is always held back, so a prompt only ever fits a window
/// that has room to answer it.
fn reserved_for_completion(window: i64) -> i64 {
    COMPLETION_FLOOR.min(window / 2).max(1)
}

/// Decide whether a prompt of `prompt_characters` fits `window`, and clamp the
/// completion length to the space that remains.
pub fn assess(prompt_characters: i64, window: i64, requested_max_tokens: Option<i64>) -> Verdict {
    let estimated = estimated_tokens(prompt_characters);
    if window <= 0 || estimated + reserved_for_completion(window) > window {
        return Verdict::Exceeds { estimated, window };
    }
    let available = window - estimated;
    let clamped = requested_max_tokens.unwrap_or(available).min(available);
    Verdict::Fits {
        clamped_max_tokens: Some(clamped),
    }
}

/// The context window Apple's built-in model serves: fixed at 4096 tokens per
/// session. The single source for every consumer — the runtime facade's
/// budget, the apple-foundation scanner's record hint, and this module's
/// policy — so the value cannot drift apart.
pub const BUILTIN_CONTEXT_WINDOW: i64 = 4096;

/// The effective context window for `record`, honoring a per-runtime policy. A
/// built-in model has a fixed window; other runtimes derive it from the record's
/// declared context length (and, for Ollama, a caller override).
pub fn effective_window(
    record: &ModelRecord,
    requested_context_length: Option<i64>,
) -> Option<i64> {
    if record.source.kind == SourceKind::builtin() {
        return Some(BUILTIN_CONTEXT_WINDOW);
    }
    let window = record_policy_window(record, requested_context_length)?;
    (window > 0).then_some(window)
}

fn record_policy_window(record: &ModelRecord, requested: Option<i64>) -> Option<i64> {
    let id = record.runtime.id.as_ref()?;
    if *id == RuntimeId::ollama() {
        requested.or(record.context_length)
    } else if *id == RuntimeId::llama_cpp()
        || *id == RuntimeId::mlx_swift()
        || *id == RuntimeId::mlx_lm()
    {
        record.context_length
    } else {
        None
    }
}

/// The user-set context length stored in the record's parameter values, if any.
pub fn stored_context_length(record: &ModelRecord) -> Option<i64> {
    normalized_param_values(record)
        .get("context_length")
        .and_then(JsonValue::as_i64)
}

/// Count the characters a chat/completion payload will send: the `content` of
/// each message plus a top-level `prompt` string, if present.
pub fn prompt_characters(payload: &JsonValue) -> i64 {
    let JsonValue::Object(object) = payload else {
        return 0;
    };
    let mut total = 0i64;
    if let Some(JsonValue::Array(messages)) = object.get("messages") {
        for message in messages {
            if let JsonValue::Object(fields) = message
                && let Some(JsonValue::String(content)) = fields.get("content")
            {
                total += content.chars().count() as i64;
            }
        }
    }
    if let Some(JsonValue::String(prompt)) = object.get("prompt") {
        total += prompt.chars().count() as i64;
    }
    total
}
