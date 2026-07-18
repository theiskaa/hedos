//! Identifying a model's format and capabilities from its files. The full
//! runtime-bid auction lives with the runtime adapters; this module holds the
//! file-format detection those decisions build on.

pub mod format;
pub mod gguf;
pub mod identification_cache;
pub mod identity;
pub mod pipelines;
pub mod safetensors;

pub use format::{
    GgufArchitectureProfile, GgufFacts, ModelFormat, gguf_architecture_profile,
    ollama_chat_profile, ollama_profile, ollama_vision_profile,
};
pub use gguf::{gguf_facts, gguf_general_architecture, has_ggml_magic, has_gguf_magic};
pub use identification_cache::IdentificationCache;
pub use identity::{IdentifiedModel, RuntimeBid, identify};
pub use pipelines::{
    DiffusersPipelineProfile, PipelineFamily, PipelineFamilyRegistry, PipelineRefinement,
    SchedulerFacts,
};
pub use safetensors::{safetensors_format, safetensors_header_format};
