//! The loopback gateway: an OpenAI (`/v1`) and Ollama (`/api`) compatible HTTP
//! server that fronts the local kernel so other tools can drive installed models.
//!
//! This crate is being built up surface by surface. The foundation here — the
//! wire [`surface`], the [`error`] type, per-client [`scopes`], and the fixed
//! [`defaults`] — is what the routers, handlers, and server build on.

pub mod defaults;
pub mod error;
pub mod scopes;
pub mod surface;

pub use error::{GatewayError, GatewayErrorKind};
pub use scopes::GatewayScopes;
pub use surface::GatewaySurface;
