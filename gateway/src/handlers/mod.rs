//! The per-route request handlers, plus the streaming helpers they share.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;

use kernel::capabilities::{CapabilityChunk, GenerationStats, ToolCall};
use kernel::records::{Capability, JsonValue, ModelRecord};
use runtime::adapters::ChunkStream;

use crate::admission::GatewayWorkKind;
use crate::error::GatewayError;
use crate::identity::{GatewayIdentity, GatewayOutcome};
use crate::param_guard;
use crate::port::GatewayPort;
use crate::request::GatewayRequest;
use crate::resolver::resolve_authorized;
use crate::responder::GatewayResponder;

pub mod chat;
pub mod embeddings;
pub mod generate;
pub mod images;
pub mod messages;
pub mod models;
pub mod speech;
pub mod stream;
pub mod transcriptions;

/// The error shown when a runtime stream fails mid-flight.
///
/// The runtime's own message is surfaced rather than hidden. The gateway binds
/// loopback and treats every local caller as trusted with every model on the
/// shelf, so a caller that can read this error can already do far more than read
/// it; withholding the text buys no confidentiality and costs the only
/// explanation of why a request failed. "the runtime failed to complete the
/// request" is unactionable when the real cause is a stopped daemon, a model
/// that cannot do tool calling, or an out-of-memory GPU.
pub(crate) fn runtime_failed(error: runtime::adapters::RuntimeError) -> GatewayError {
    GatewayError::new(
        crate::error::GatewayErrorKind::ServerError,
        error.to_string(),
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

/// The shared streaming prefix: resolve and admit `model` for `capability`,
/// reject tools it can't serve (when `tools_present`), guard the parameters, and
/// invoke it — yielding the record and its chunk stream. Chat passes
/// [`Capability::chat`] with its tool flag; the prompt endpoints pass
/// [`Capability::complete`] with `tools_present = false`.
pub(crate) async fn dispatch(
    port: &dyn GatewayPort,
    identity: &GatewayIdentity,
    model: &str,
    capability: Capability,
    tools_present: bool,
    params: &BTreeMap<String, JsonValue>,
    payload: JsonValue,
) -> Result<(ModelRecord, ChunkStream), GatewayError> {
    let record = resolve_authorized(
        port,
        model,
        capability.clone(),
        GatewayWorkKind::Stream,
        identity,
    )
    .await?;
    if tools_present && !record.capabilities.contains(&Capability::tools()) {
        return Err(bad_request(format!(
            "{model} does not support tool calling"
        )));
    }
    let honored = port.honored_params(&record.id, capability.clone()).await?;
    param_guard::require(params, &honored, record.runtime.id.as_ref())?;
    let stream = port.invoke(&record.id, capability, payload).await?;
    Ok((record, stream))
}

/// Drain a non-streamed run to its accumulated text, tool calls, and final
/// stats. The prompt endpoints ignore the (always empty) tool-call vector.
pub(crate) async fn collect_completion(
    stream: &mut ChunkStream,
) -> Result<(String, Vec<ToolCall>, Option<GenerationStats>), GatewayError> {
    let mut content = String::new();
    let mut tool_calls = Vec::new();
    let mut final_stats = None;
    while let Some(result) = stream.recv().await {
        match result.map_err(runtime_failed)? {
            CapabilityChunk::Text(text) => content.push_str(&text),
            CapabilityChunk::ToolCall(call) => tool_calls.push(call),
            CapabilityChunk::Done(stats) => final_stats = stats,
            _ => {}
        }
    }
    Ok((content, tool_calls, final_stats))
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
