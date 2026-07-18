//! The kernel's data model: model records and the value types they compose.

pub mod byte_format;
pub mod identifiers;
pub mod json_value;
pub mod model_record;
pub mod param_form;
pub mod text_budget;

pub use byte_format::format_bytes;
pub use identifiers::{
    BidPreference, Capability, ExecutionMode, Modality, ModelState, RunTier, RuntimeId, SourceKind,
};
pub use json_value::JsonValue;
pub use model_record::{
    ModelRecord, ModelSource, ParamSpec, ParamType, Resolution, RuntimeRef, stable_id,
};
pub use text_budget::{Clip, clip};
