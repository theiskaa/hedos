//! Format and capability facts derived from a model's files.

use serde::{Deserialize, Serialize};

use crate::records::{Capability, ExecutionMode, Modality};

/// A recognized model format — the on-disk weight formats plus the logical
/// sources (a diffusers pipeline directory, an Ollama store entry, a built-in or
/// remote-endpoint model), and `Unknown` for anything unrecognized.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ModelFormat {
    /// A GGUF weight file (llama.cpp).
    Gguf,
    /// A legacy GGML `.bin` weight file.
    GgmlBin,
    /// A safetensors weight directory.
    Safetensors,
    /// An MLX-format safetensors weight directory.
    MlxSafetensors,
    /// A diffusers pipeline directory (a `model_index.json`).
    Diffusers,
    /// An entry in a local Ollama store.
    OllamaStore,
    /// A built-in (platform-provided) model.
    Builtin,
    /// A remote inference endpoint.
    Endpoint,
    /// An unrecognized format.
    Unknown,
}

/// The modality, capabilities, and execution shape implied by a GGUF
/// architecture.
#[derive(Debug, Clone, PartialEq)]
pub struct GgufArchitectureProfile {
    /// The model's primary modality.
    pub modality: Modality,
    /// What the model can do.
    pub capabilities: Vec<Capability>,
    /// How its runtime delivers output.
    pub execution: ExecutionMode,
}

/// Facts read from a GGUF header.
#[derive(Debug, Clone, PartialEq)]
pub struct GgufFacts {
    /// The `general.architecture` value, if present.
    pub architecture: Option<String>,
    /// The resolved context length, if the header declared one.
    pub context_length: Option<i64>,
    /// Whether the header carries a chat template.
    pub has_chat_template: bool,
    /// The weight type `general.file_type` names, as llama.cpp spells it
    /// (`Q4_K_M`, `Q8_0`, `F16`), when the header carries a known one.
    pub quantization: Option<String>,
    /// How the model pools its token states into one output, from
    /// `{arch}.pooling_type`, when the header names a value llama.cpp knows.
    pub pooling: Option<GgufPooling>,
    /// The `{arch}.decision.type` a decision model's header names (`clef`,
    /// `laya`): a model that answers typed questions with probabilities, and
    /// neither chats nor embeds whatever its architecture.
    pub decision: Option<String>,
    /// Whether the weights carry a classification head (a `cls.weight` or
    /// `cls.output.weight` tensor), which is what a reranker scores a pair with.
    pub has_classifier_head: bool,
}

/// How a model pools its per-token states into one output, as llama.cpp's
/// `llama_pooling_type` names it. Which one a GGUF declares decides what
/// llama-server can hand back for it, whether the model is an encoder or a
/// decoder converted to embed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GgufPooling {
    /// No pooling: the model returns a state per token, never one vector.
    None,
    /// The mean of the token states.
    Mean,
    /// The state of the leading classification token.
    Cls,
    /// The state of the last token.
    Last,
    /// A relevance score for a query and document pair: a reranker.
    Rank,
}

impl GgufPooling {
    /// The pooling a `{arch}.pooling_type` value names; `None` for a value
    /// llama.cpp does not define, its `unspecified` (-1) included.
    pub(crate) fn from_llama(value: i64) -> Option<Self> {
        Some(match value {
            0 => Self::None,
            1 => Self::Mean,
            2 => Self::Cls,
            3 => Self::Last,
            4 => Self::Rank,
            _ => return None,
        })
    }
}

/// The known-architecture profile for a GGUF `general.architecture` value.
///
/// The table names the architectures that are *not* a text chat model, which is
/// what an architecture missing from it is taken to be: the great majority of
/// them are, and a name nobody has taught this table is far more likely to be
/// another text model than a component. What is listed here is the exceptions:
/// the encoders that only embed, the models that can also see, and the pieces
/// that are half of a pipeline and serve nothing on their own.
pub fn gguf_architecture_profile(architecture: &str) -> Option<GgufArchitectureProfile> {
    let profile = |modality, capabilities, execution| GgufArchitectureProfile {
        modality,
        capabilities,
        execution,
    };
    let embedding = || {
        Some(profile(
            Modality::embedding(),
            vec![Capability::embed()],
            ExecutionMode::Stream,
        ))
    };
    let sees = || {
        Some(profile(
            Modality::text(),
            vec![
                Capability::chat(),
                Capability::complete(),
                Capability::see(),
            ],
            ExecutionMode::Stream,
        ))
    };
    // A piece of a pipeline: it has a modality but nothing can be asked of it
    // directly, so no runtime offers to serve it and it is never mistaken for a
    // model that answers.
    let component = |modality| Some(profile(modality, vec![], ExecutionMode::Sync));
    match architecture {
        "whisper" => Some(profile(
            Modality::audio(),
            vec![Capability::transcribe()],
            ExecutionMode::Stream,
        )),
        "qwen2vl" | "qwen3vl" | "qwen3vlmoe" | "mllama" | "cogvlm" | "hunyuan-vl" => sees(),
        "bert" | "nomic-bert" | "nomic-bert-moe" | "jina-bert-v2" | "jina-bert-v3" | "neo-bert"
        | "modern-bert" | "eurobert" | "gemma-embedding" | "llama-embed" | "t5encoder" => {
            embedding()
        }
        "clip" => component(Modality::vision()),
        // The vocoder half of a text-to-speech pair: it turns another model's
        // tokens into audio and cannot be prompted.
        "wavtokenizer-dec" => component(Modality::audio()),
        _ => None,
    }
}

/// The default profile for an Ollama chat model with no more specific match.
pub fn ollama_chat_profile() -> GgufArchitectureProfile {
    GgufArchitectureProfile {
        modality: Modality::text(),
        capabilities: vec![Capability::chat(), Capability::complete()],
        execution: ExecutionMode::Stream,
    }
}

/// The default profile for an Ollama vision-capable chat model.
pub fn ollama_vision_profile() -> GgufArchitectureProfile {
    GgufArchitectureProfile {
        modality: Modality::text(),
        capabilities: vec![
            Capability::chat(),
            Capability::complete(),
            Capability::see(),
        ],
        execution: ExecutionMode::Stream,
    }
}

/// The profile for an Ollama model: vision when it ships a projector, otherwise
/// the profile of `architecture`, the one its weight blob's GGUF header names,
/// if recognized, else the plain chat default.
pub fn ollama_profile(has_projector: bool, architecture: Option<&str>) -> GgufArchitectureProfile {
    if has_projector {
        return ollama_vision_profile();
    }
    architecture
        .and_then(gguf_architecture_profile)
        .unwrap_or_else(ollama_chat_profile)
}
