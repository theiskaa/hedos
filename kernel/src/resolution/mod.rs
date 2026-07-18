//! Identifying a model's format and capabilities from its files. The full
//! runtime-bid auction lives with the runtime adapters; this module holds the
//! file-format detection those decisions build on.

pub mod format;
pub mod gguf;
pub mod safetensors;

pub use format::{
    GgufArchitectureProfile, GgufFacts, ModelFormat, gguf_architecture_profile,
    ollama_chat_profile, ollama_vision_profile,
};
pub use gguf::{gguf_facts, gguf_general_architecture, has_ggml_magic, has_gguf_magic};
pub use safetensors::{safetensors_format, safetensors_header_format};
