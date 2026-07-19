//! The per-route request handlers, plus the streaming helpers they share.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;

use kernel::records::JsonValue;

use crate::error::GatewayError;
use crate::identity::{GatewayIdentity, GatewayOutcome};
use crate::port::GatewayPort;
use crate::request::GatewayRequest;
use crate::responder::GatewayResponder;

pub mod chat;
pub mod embeddings;
pub mod generate;
pub mod images;
pub mod models;
pub mod speech;
pub mod stream;

/// The generic server error shown when a runtime stream fails mid-flight; the
/// runtime's own message may carry internals, so it is not surfaced.
pub(crate) fn runtime_failed(_: runtime::adapters::RuntimeError) -> GatewayError {
    GatewayError::new(
        crate::error::GatewayErrorKind::ServerError,
        "the runtime failed to complete the request",
    )
}

/// A `400 Bad Request` carrying `message`.
pub(crate) fn bad_request(message: impl Into<String>) -> GatewayError {
    GatewayError::new(crate::error::GatewayErrorKind::BadRequest, message)
}

/// A `500` server error carrying `message`; unlike [`runtime_failed`] the caller
/// supplies the wording, so use it only for messages safe to surface.
pub(crate) fn server_error(message: impl Into<String>) -> GatewayError {
    GatewayError::new(crate::error::GatewayErrorKind::ServerError, message)
}

/// The non-empty `model` field of a request body, or a `400`.
pub(crate) fn required_model(body: &BTreeMap<String, JsonValue>) -> Result<&str, GatewayError> {
    body.get("model")
        .and_then(JsonValue::as_str)
        .filter(|model| !model.is_empty())
        .ok_or_else(|| bad_request("model is required"))
}

/// Send `value` as a `200` JSON response.
pub(crate) fn respond_json(responder: &GatewayResponder, value: &serde_json::Value) {
    responder.respond(
        200,
        "application/json",
        serde_json::to_vec(value).unwrap_or_default(),
        Vec::new(),
    );
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
