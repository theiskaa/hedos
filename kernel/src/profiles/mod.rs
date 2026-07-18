//! Per-model configuration: the parameter schema a model exposes, merging saved
//! values and the system prompt into requests, and context-window budgeting.

pub mod configuration;
pub mod context_budget;
pub mod profile;

pub use configuration::{dropping_vanished_param_values, merged, normalized_param_values};
pub use context_budget::{
    COMPLETION_FLOOR, Verdict, assess, effective_window, estimated_tokens, prompt_characters,
    stored_context_length,
};
pub use profile::{ModelProfile, ProfileRegistry, context_length_spec};
