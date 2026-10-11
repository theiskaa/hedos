//! Modality/capability hints derived from a Hugging Face model's `config.json`:
//! a first guess (architecture list, model type, a vision block, or a few
//! speech config keys) at what a downloaded model can do, before full
//! identification, weighed against what the model card says ([`from_snapshot`]).
//! A diffusers `model_index.json` is hinted through the pipeline-family registry
//! ([`from_model_index`]).

use std::collections::BTreeSet;
use std::path::Path;

use super::card::ModelCard;
use crate::records::{Capability, ExecutionMode, JsonValue, Modality, ParamSpec};
use crate::resolution::pipelines::{
    PipelineFamilyRegistry, diffusers_pipeline_class, musicgen_params,
};

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
    /// The quantization the config names, as `4bit`, when it does.
    pub quantization: Option<String>,
    /// The parameter schema the model takes, when the hint knows it.
    pub params: Vec<ParamSpec>,
}

impl Hint {
    fn new(modality: Modality, capabilities: Vec<Capability>, execution: ExecutionMode) -> Self {
        Self {
            modality: Some(modality),
            capabilities,
            execution,
            context_length: None,
            quantization: None,
            params: Vec::new(),
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
            quantization: None,
            params: Vec::new(),
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

/// A model that makes audio (songs, music, sound effects) from a description,
/// with the parameters it takes.
fn audio_generator_hint(params: Vec<ParamSpec>) -> Hint {
    let mut hint = Hint::new(
        Modality::audio(),
        vec![Capability::audio()],
        ExecutionMode::Job,
    );
    hint.params = params;
    hint
}

/// A model recognized for what it is but claiming no capability of its own:
/// its own code drives it (YuE2, a TTS model built on a causal LM), so it must
/// not inherit `chat` from an architecture name, and whichever runtime can drive
/// it says what it does.
fn withheld_hint(modality: Modality, execution: ExecutionMode) -> Hint {
    Hint::new(modality, Vec::new(), execution)
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

/// A reranker: text in, a relevance score out. It chats with no one and embeds
/// nothing, so it claims no capability of its own; the runtime that can serve it
/// supplies what it does.
pub fn reranker_hint() -> Hint {
    Hint::new(Modality::text(), Vec::new(), ExecutionMode::Stream)
}

/// What a sentence-transformers layout in a snapshot is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SentenceTransformersLayout {
    /// A bi-encoder, pooling a text into an embedding.
    Embedder,
    /// A cross-encoder, scoring a pair of texts against each other.
    CrossEncoder,
}

/// The sentence-transformers layout in `dir`, if there is one. Both kinds ship a
/// `config_sentence_transformers.json`; only its `model_type` tells a
/// cross-encoder from an embedder, and one read as the other is served nonsense.
pub fn sentence_transformers_layout(dir: &Path) -> Option<SentenceTransformersLayout> {
    let config = dir.join("config_sentence_transformers.json");
    let declared = crate::fs::read_regular(&config)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<JsonValue>(&bytes).ok())
        .and_then(|json| {
            json.as_object()?
                .get("model_type")
                .and_then(JsonValue::as_str)
                .map(str::to_owned)
        });
    if declared.as_deref() == Some("CrossEncoder") {
        return Some(SentenceTransformersLayout::CrossEncoder);
    }
    (config.exists() || dir.join("1_Pooling").exists())
        .then_some(SentenceTransformersLayout::Embedder)
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
            quantization: None,
            params: Vec::new(),
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
    let bytes = crate::fs::read_regular(path).ok()?;
    let json = serde_json::from_slice::<JsonValue>(&bytes).ok()?;
    from_config(&json)
}

/// Model types that make audio with their own inference code, or are a part of
/// one (a decoder, a codec). None of them chats, whatever its architecture is
/// named (`YuE2ForCausalLM`), and nothing built in can drive them.
const AUDIO_GENERATOR_MODEL_TYPES: [&str; 7] = [
    "yue2",
    "yue2_vae",
    "acestep",
    "heartmula",
    "heartcodec",
    "diffrhythm",
    "minimax_music3",
];

/// Model types transformers generates audio with by itself.
const LIBRARY_AUDIO_MODEL_TYPES: [&str; 2] = ["musicgen", "musicgen_melody"];

/// The hint for a parsed `config.json` value, by the first rule that matches:
/// a speech model (by architecture or config keys), an audio generator (by
/// model type), a vision-language model (a `vision_config` block plus a
/// matching architecture), else the first architecture that matches an
/// [`architecture_hint`] rule, else `None`. Speech goes first so a voice is
/// never read as anything else.
pub fn from_config(json: &JsonValue) -> Option<Hint> {
    let object = json.as_object()?;
    let architectures: Vec<&str> = object
        .get("architectures")
        .and_then(JsonValue::as_array)
        .map(|items| items.iter().filter_map(JsonValue::as_str).collect())
        .unwrap_or_default();
    let model_type = object.get("model_type").and_then(JsonValue::as_str);

    // First present integer among the window keys, then require it positive
    // (a present-but-zero value voids the hint rather than falling through).
    let context_length = ["max_position_embeddings", "n_positions", "max_seq_len"]
        .into_iter()
        .find_map(|key| object.get(key).and_then(JsonValue::as_i64))
        .filter(|value| *value > 0);
    // MLX writes the bits it quantized to under `quantization`; a config without
    // the block is unquantized, or not MLX's.
    let quantization = object
        .get("quantization")
        .and_then(JsonValue::as_object)
        .and_then(|block| block.get("bits"))
        .and_then(JsonValue::as_i64)
        .filter(|bits| *bits > 0)
        .map(|bits| format!("{bits}bit"));
    let facts = |hint: Hint| with_facts(hint, context_length, quantization.clone());

    let keys: BTreeSet<&str> = object.keys().map(String::as_str).collect();
    if architectures
        .iter()
        .any(|architecture| is_speech_architecture(architecture))
    {
        return Some(facts(speech_hint()));
    }
    if let Some(hint) = config_key_hint(&keys) {
        return Some(facts(hint));
    }

    if let Some(model_type) = model_type {
        if AUDIO_GENERATOR_MODEL_TYPES.contains(&model_type) {
            return Some(facts(withheld_hint(Modality::audio(), ExecutionMode::Job)));
        }
        if LIBRARY_AUDIO_MODEL_TYPES.contains(&model_type) {
            return Some(facts(audio_generator_hint(musicgen_params())));
        }
    }

    if object.contains_key("vision_config")
        && architectures.iter().any(|architecture| {
            architecture.ends_with("ForConditionalGeneration")
                || VISION_LANGUAGE_ARCHITECTURES
                    .iter()
                    .any(|marker| architecture.contains(marker))
        })
    {
        return Some(facts(vision_chat_hint()));
    }

    architectures
        .iter()
        .find_map(|architecture| architecture_hint(architecture))
        .map(facts)
}

/// The hint for a model directory: its `config.json` ([`from_config_json`]),
/// weighed against its model card ([`ModelCard`]). The config's verdict stands
/// when it names a voice, an audio model, an embedder or anything else it is
/// sure of; the card only corrects a config that reads as text (or says
/// nothing), since that is the reading an architecture name gets wrong.
pub fn from_snapshot(dir: &Path) -> Option<Hint> {
    weighed(
        from_config_json(&dir.join("config.json")),
        ModelCard::read(dir).as_ref(),
        ships_chat_template(dir),
    )
}

/// A config hint corrected by a model card, as [`from_snapshot`] weighs them.
/// `chat_template` says whether the snapshot ships a chat template, which a
/// chat model filed with a `music` tag has and a music model does not.
pub fn weighed(
    config: Option<Hint>,
    card: Option<&ModelCard>,
    chat_template: bool,
) -> Option<Hint> {
    let open = config
        .as_ref()
        .is_none_or(|hint| hint.modality.is_none() || hint.modality == Some(Modality::text()));
    if !open {
        return config;
    }
    let Some(verdict) = card.and_then(|card| card_hint(card, chat_template)) else {
        return config;
    };
    Some(match config {
        Some(config) => with_facts(verdict, config.context_length, config.quantization),
        None => verdict,
    })
}

/// Card tags that mark a music generator.
const MUSIC_TAGS: [&str; 4] = ["music", "music-generation", "text-to-music", "text2music"];

/// What a model card alone says a model is, ranked: a card filed under
/// `text-to-speech` is a voice whatever its tags add (Bark and CSM also carry
/// `text-to-audio`); a card filed under `text-to-audio` or `text-to-music` makes
/// audio; a music tag on a card filed as text generation, or not filed, makes
/// audio only when no chat template ships (YuE v1's stage-1 Llama), since a
/// chat model tagged with its subject keeps one. A bare `text-to-audio` card
/// (ChatTTS, a voice) still only keeps the model out of chat: none of these
/// claims a capability.
fn card_hint(card: &ModelCard, chat_template: bool) -> Option<Hint> {
    let pipeline = card.pipeline_tag.as_deref();
    if pipeline == Some("text-to-speech") {
        return Some(withheld_hint(Modality::speech(), ExecutionMode::Stream));
    }
    let filed_as_audio = matches!(pipeline, Some("text-to-audio" | "text-to-music"));
    let tagged_music = matches!(pipeline, None | Some("text-generation"))
        && !chat_template
        && MUSIC_TAGS.iter().any(|tag| card.has_tag(tag));
    (filed_as_audio || tagged_music).then(|| withheld_hint(Modality::audio(), ExecutionMode::Job))
}

/// Whether `dir` ships a chat template: a `chat_template.jinja` or
/// `chat_template.json` file, or a `chat_template` entry in its
/// `tokenizer_config.json`.
fn ships_chat_template(dir: &Path) -> bool {
    if dir.join("chat_template.jinja").is_file() || dir.join("chat_template.json").is_file() {
        return true;
    }
    crate::fs::read_regular(&dir.join("tokenizer_config.json")).is_ok_and(|bytes| {
        bytes
            .windows(b"\"chat_template\"".len())
            .any(|window| window == b"\"chat_template\"")
    })
}

fn with_facts(mut hint: Hint, context_length: Option<i64>, quantization: Option<String>) -> Hint {
    hint.context_length = context_length;
    hint.quantization = quantization;
    hint
}

/// Whether `dir` holds a Kokoro checkpoint: its `config.json` carries the
/// `istftnet` decoder block that Kokoro (and nothing else hedos meets) is
/// built around. The shipped speech runtime drives Kokoro's own pipeline and
/// voice packs, so this is what it can load, not every voice.
pub fn is_kokoro_layout(dir: &Path) -> bool {
    crate::fs::read_regular(&dir.join("config.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<JsonValue>(&bytes).ok())
        .and_then(|json| {
            json.as_object()
                .map(|object| object.contains_key("istftnet"))
        })
        .unwrap_or(false)
}

/// Architecture-name substrings that mark a text-to-speech model.
const SPEECH_ARCHITECTURES: [&str; 6] = ["Kokoro", "StyleTTS", "Bark", "ParlerTTS", "Vits", "Xtts"];

fn is_speech_architecture(architecture: &str) -> bool {
    SPEECH_ARCHITECTURES
        .iter()
        .any(|marker| architecture.contains(marker))
}

/// The hint for a single architecture name, by substring/suffix rules (checked in
/// priority order: speech, audio, embedding, then causal-LM text).
fn architecture_hint(architecture: &str) -> Option<Hint> {
    const EMBEDDING: [&str; 5] = [
        "BertModel",
        "NomicBertModel",
        "ModernBertModel",
        "XLMRobertaModel",
        "MPNetModel",
    ];
    if is_speech_architecture(architecture) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_card_on_a_feature_extractor_tagged_music_says_nothing() {
        let card = ModelCard::parse("---\npipeline_tag: feature-extraction\ntags:\n- music\n---\n");
        assert_eq!(weighed(None, card.as_ref(), false), None);
    }
}
