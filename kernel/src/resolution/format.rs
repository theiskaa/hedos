//! Format and capability facts derived from a model's files.

use serde::{Deserialize, Serialize};

use crate::records::{Capability, ExecutionMode, Modality};

/// A recognized on-disk model format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
}

/// The known-architecture profile for a GGUF `general.architecture` value.
pub fn gguf_architecture_profile(architecture: &str) -> Option<GgufArchitectureProfile> {
    let profile = |modality, capabilities, execution| GgufArchitectureProfile {
        modality,
        capabilities,
        execution,
    };
    match architecture {
        "whisper" => Some(profile(
            Modality::audio(),
            vec![Capability::transcribe()],
            ExecutionMode::Stream,
        )),
        "qwen2vl" | "mllama" => Some(profile(
            Modality::text(),
            vec![
                Capability::chat(),
                Capability::complete(),
                Capability::see(),
            ],
            ExecutionMode::Stream,
        )),
        "clip" => Some(profile(Modality::vision(), vec![], ExecutionMode::Sync)),
        "bert" | "nomic-bert" => Some(profile(
            Modality::embedding(),
            vec![Capability::embed()],
            ExecutionMode::Stream,
        )),
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
/// the GGUF architecture's profile (read from `weight_path`) if recognized, else
/// the plain chat default.
///
/// `weight_path` must be absolute — unlike the Swift original this does not
/// expand a leading `~` (the Ollama scanner always passes a resolved blob path;
/// a `~`-relative path would fail to open and fall through to the chat default).
pub fn ollama_profile(has_projector: bool, weight_path: Option<&str>) -> GgufArchitectureProfile {
    if has_projector {
        return ollama_vision_profile();
    }
    if let Some(path) = weight_path
        && let Some(architecture) =
            crate::resolution::gguf::gguf_general_architecture(std::path::Path::new(path))
        && let Some(profile) = gguf_architecture_profile(&architecture)
    {
        return profile;
    }
    ollama_chat_profile()
}
