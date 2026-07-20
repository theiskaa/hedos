//! The loopback gateway: an OpenAI (`/v1`) and Ollama (`/api`) compatible HTTP
//! server that fronts the local kernel so other tools can drive installed models.
//!
//! This crate is being built up surface by surface. The foundation here — the
//! wire [`surface`], the [`error`] type, per-client [`scopes`], and the fixed
//! [`defaults`] — is what the routers, handlers, and server build on.

pub mod admission;
pub mod audit;
pub mod auth;
pub mod defaults;
pub mod error;
pub mod handlers;
pub mod identity;
pub mod kernel_gateway;
pub mod param_guard;
pub mod port;
pub mod request;
pub mod resolver;
pub mod responder;
pub mod router;
pub mod scopes;
pub mod server;
pub mod surface;
pub(crate) mod wire;

pub use error::{GatewayError, GatewayErrorKind};
pub use scopes::GatewayScopes;
pub use surface::GatewaySurface;
