//! The per-route request handlers, plus the streaming helpers they share.

use std::future::Future;
use std::pin::Pin;

use crate::error::GatewayError;
use crate::identity::{GatewayIdentity, GatewayOutcome};
use crate::port::GatewayPort;
use crate::request::GatewayRequest;
use crate::responder::GatewayResponder;

pub mod chat;
pub mod models;
pub mod stream;

/// The generic server error shown when a runtime stream fails mid-flight; the
/// runtime's own message may carry internals, so it is not surfaced.
pub(crate) fn runtime_failed(_: runtime::adapters::RuntimeError) -> GatewayError {
    GatewayError::new(
        crate::error::GatewayErrorKind::ServerError,
        "the runtime failed to complete the request",
    )
}

/// The future a [`GatewayHandling::handle`] returns, borrowing its inputs.
pub type HandlerFuture<'a> =
    Pin<Box<dyn Future<Output = Result<GatewayOutcome, GatewayError>> + Send + 'a>>;

/// A route handler: turn a request into a response written through `responder`,
/// reporting the outcome for the audit log.
pub trait GatewayHandling: Send + Sync {
    /// Handle one request.
    fn handle<'a>(
        &'a self,
        request: &'a GatewayRequest,
        identity: &'a GatewayIdentity,
        port: &'a dyn GatewayPort,
        responder: &'a GatewayResponder,
    ) -> HandlerFuture<'a>;
}

/// A fresh, opaque completion id with the given prefix (e.g. `chatcmpl-`). Uses
/// process entropy rather than a UUID dependency; clients treat it as opaque.
pub fn completion_id(prefix: &str) -> String {
    use std::hash::{BuildHasher, Hasher};
    let high = std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish();
    let low = std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish();
    format!("{prefix}{high:016x}{low:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_completion_id_carries_its_prefix_and_is_unique() {
        let first = completion_id("chatcmpl-");
        let second = completion_id("chatcmpl-");
        assert!(first.starts_with("chatcmpl-"));
        assert_eq!(first.len(), "chatcmpl-".len() + 32);
        assert_ne!(first, second);
    }
}
