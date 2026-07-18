//! Modality/capability hints derived from a Hugging Face model's `config.json`:
//! a first guess (architecture list, a vision block, or a few speech config keys)
//! at what a downloaded model can do, before full identification. A diffusers
//! `model_index.json` is hinted through the pipeline-family registry
//! ([`from_model_index`]).

use std::collections::BTreeSet;
use std::path::Path;

use crate::records::{Capability, ExecutionMode, JsonValue, Modality};
use crate::resolution::pipelines::{PipelineFamilyRegistry, diffusers_pipeline_class};

/// A best-effort guess at a model's shape from its config metadata.
#[derive(Debug, Clone, PartialEq)]
pub struct Hint {
    /// The guessed modality, if any.
    pub modality: Option<Modality>,
    /// The guessed capabilities.
    pub capabilities: Vec<Capability>,
    /// How the model executes.
    pub execution: ExecutionMode,
    /// A context-window hint pulled from the config.
    pub context_length: Option<i64>,
}

impl Hint {
    fn new(modality: Modality, capabilities: Vec<Capability>, execution: ExecutionMode) -> Self {
        Self {
            modality: Some(modality),
            capabilities,
            execution,
            context_length: None,
        }
    }

    /// An empty hint (no modality or capabilities) with the given execution shape
    /// — the starting point before any rule matches.
    pub fn unknown(execution: ExecutionMode) -> Self {
        Self {
            modality: None,
            capabilities: Vec::new(),
            execution,
            context_length: None,
        }
    }
}

/// A text-to-speech model.
pub fn speech_hint() -> Hint {
    Hint::new(
        Modality::speech(),
        vec![Capability::speak()],
        ExecutionMode::Stream,
    )
}

/// A speech-to-text (transcription) model.
pub fn audio_hint() -> Hint {
    Hint::new(
        Modality::audio(),
        vec![Capability::transcribe()],
        ExecutionMode::Stream,
    )
}

/// A text chat/completion model.
pub fn text_hint() -> Hint {
    Hint::new(
        Modality::text(),
        vec![Capability::chat(), Capability::complete()],
        ExecutionMode::Stream,
    )
}

/// An embedding model.
pub fn embedding_hint() -> Hint {
    Hint::new(
        Modality::embedding(),
        vec![Capability::embed()],
        ExecutionMode::Stream,
    )
}

/// A vision-capable chat model.
pub fn vision_chat_hint() -> Hint {
    Hint::new(
        Modality::text(),
        vec![
            Capability::chat(),
            Capability::complete(),
            Capability::see(),
        ],
        ExecutionMode::Stream,
    )
}

/// The default hint for a bare GGUF weight (text chat/completion).
pub fn gguf_hint() -> Hint {
    text_hint()
}

/// The default hint for a whisper `.bin` (transcription).
pub fn whisper_bin_hint() -> Hint {
    audio_hint()
}

/// The hint for a diffusers `model_index.json`: its pipeline family's modality and
/// capabilities as a job. An unknown or absent `_class_name` yields an empty job
/// hint (still a job — a diffusers bundle is never a streaming model).
pub fn from_model_index(path: &Path) -> Hint {
    if let Some(class) = diffusers_pipeline_class(path)
        && let Some(family) = PipelineFamilyRegistry::shared().family(&class)
    {
        return Hint {
            modality: Some(family.modality.clone()),
            capabilities: family.capabilities.clone(),
            execution: ExecutionMode::Job,
            context_length: None,
        };
    }
    Hint::unknown(ExecutionMode::Job)
}

/// Architecture-name substrings that mark a vision-language model when a
/// `vision_config` block is also present.
const VISION_LANGUAGE_ARCHITECTURES: [&str; 5] =
    ["Llava", "Qwen2VL", "Idefics", "PaliGemma", "Mllama"];

/// The hint for a Hugging Face `config.json` file, or `None` if it is unreadable,
/// unparseable, or matches no rule.
pub fn from_config_json(path: &Path) -> Option<Hint> {
    let bytes = std::fs::read(path).ok()?;
    let json = serde_json::from_slice::<JsonValue>(&bytes).ok()?;
    from_config(&json)
}

/// The hint for a parsed `config.json` value: a vision-language model (a
/// `vision_config` block plus a matching architecture), else the first
/// architecture that matches an [`architecture_hint`] rule, else a speech model
/// recognized by its config keys, else `None`.
pub fn from_config(json: &JsonValue) -> Option<Hint> {
    let object = json.as_object()?;
    let architectures: Vec<&str> = object
        .get("architectures")
        .and_then(JsonValue::as_array)
        .map(|items| items.iter().filter_map(JsonValue::as_str).collect())
        .unwrap_or_default();

    // First present integer among the window keys, then require it positive
    // (a present-but-zero value voids the hint rather than falling through).
    let context_length = ["max_position_embeddings", "n_positions", "max_seq_len"]
        .into_iter()
        .find_map(|key| object.get(key).and_then(JsonValue::as_i64))
        .filter(|value| *value > 0);

    if object.contains_key("vision_config")
        && architectures.iter().any(|architecture| {
            architecture.ends_with("ForConditionalGeneration")
                || VISION_LANGUAGE_ARCHITECTURES
                    .iter()
                    .any(|marker| architecture.contains(marker))
        })
    {
        return Some(with_context(vision_chat_hint(), context_length));
    }

    for architecture in &architectures {
        if let Some(hint) = architecture_hint(architecture) {
            return Some(with_context(hint, context_length));
        }
    }

    let keys: BTreeSet<&str> = object.keys().map(String::as_str).collect();
    config_key_hint(&keys).map(|hint| with_context(hint, context_length))
}

fn with_context(mut hint: Hint, context_length: Option<i64>) -> Hint {
    hint.context_length = context_length;
    hint
}

/// The hint for a single architecture name, by substring/suffix rules (checked in
/// priority order: speech, audio, embedding, then causal-LM text).
fn architecture_hint(architecture: &str) -> Option<Hint> {
    const SPEECH: [&str; 6] = ["Kokoro", "StyleTTS", "Bark", "ParlerTTS", "Vits", "Xtts"];
    const EMBEDDING: [&str; 5] = [
        "BertModel",
        "NomicBertModel",
        "ModernBertModel",
        "XLMRobertaModel",
        "MPNetModel",
    ];
    if SPEECH.iter().any(|marker| architecture.contains(marker)) {
        return Some(speech_hint());
    }
    if architecture.contains("Whisper") {
        return Some(audio_hint());
    }
    if EMBEDDING.iter().any(|marker| architecture.contains(marker)) {
        return Some(embedding_hint());
    }
    if architecture.contains("LMHead") || architecture.ends_with("ForCausalLM") {
        return Some(text_hint());
    }
    None
}

/// The hint for a set of config keys: a few speech models are recognized only by
/// their config shape (all required keys present).
fn config_key_hint(keys: &BTreeSet<&str>) -> Option<Hint> {
    if keys.contains("istftnet") || keys.contains("plbert") {
        return Some(speech_hint());
    }
    if keys.contains("style_dim") && keys.contains("n_mels") {
        return Some(speech_hint());
    }
    None
}
