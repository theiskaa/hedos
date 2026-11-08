//! The per-route request handlers, plus the streaming helpers they share.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;

use kernel::capabilities::{CapabilityChunk, GenerationStats, ToolCall};
use kernel::records::{Capability, JsonValue};
use runtime::adapters::ChunkStream;
use runtime::settings::{DEFAULT_ROUTE_NAME, ROUTING_OFF};

use crate::admission::GatewayWorkKind;
use crate::auto_route::RouteRequest;
use crate::error::{GatewayError, GatewayErrorKind};
use crate::identity::{GatewayIdentity, GatewayOutcome};
use crate::param_guard;
use crate::port::GatewayPort;
use crate::request::GatewayRequest;
use crate::resolver::{authorize, resolve_authorized};
use crate::responder::GatewayResponder;

pub mod audio_generation;
pub mod chat;
pub mod embeddings;
pub mod extract;
pub mod generate;
pub mod images;
pub mod messages;
pub mod models;
mod named;
pub mod speech;
pub mod stream;
pub mod systemone;
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
///
/// A rejection is the exception to the `500`: the runtime is saying the request
/// itself was wrong, and a client that retries server errors should not retry it.
pub(crate) fn runtime_failed(error: runtime::adapters::RuntimeError) -> GatewayError {
    let kind = match error {
        runtime::adapters::RuntimeError::Rejected(_) => GatewayErrorKind::BadRequest,
        _ => GatewayErrorKind::ServerError,
    };
    GatewayError::new(kind, error.to_string())
}

/// A `400 Bad Request` carrying `message`.
pub(crate) fn bad_request(message: impl Into<String>) -> GatewayError {
    GatewayError::new(GatewayErrorKind::BadRequest, message)
}

/// A `500` server error carrying `message`; unlike [`runtime_failed`] the caller
/// supplies the wording, so use it only for messages safe to surface.
pub(crate) fn server_error(message: impl Into<String>) -> GatewayError {
    GatewayError::new(GatewayErrorKind::ServerError, message)
}

/// The non-empty `model` field of a request body, or a `400`.
pub(crate) fn required_model(body: &BTreeMap<String, JsonValue>) -> Result<&str, GatewayError> {
    body.get("model")
        .and_then(JsonValue::as_str)
        .filter(|model| !model.is_empty())
        .ok_or_else(|| bad_request("model is required"))
}

/// A dispatched request: its chunk stream, the headers its response carries
/// (naming the route when auto routing chose the model), and the outcome to
/// audit once it is served.
pub(crate) struct Dispatched {
    pub stream: ChunkStream,
    pub headers: Vec<(String, String)>,
    pub outcome: GatewayOutcome,
}

/// The header naming the model auto routing chose.
pub const ROUTED_MODEL_HEADER: &str = "x-hedos-model";
/// The header saying why auto routing chose it.
pub const ROUTE_REASON_HEADER: &str = "x-hedos-route";

/// `value` with anything a header value cannot hold replaced by `?`. A model
/// name or a configured hard word can be any text, and one bad byte makes the
/// whole response unbuildable.
fn header_safe(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c == ' ' || c.is_ascii_graphic() {
                c
            } else {
                '?'
            }
        })
        .collect()
}

/// The shared streaming prefix: resolve and admit `model` for the request's
/// capability (routing it first when it names the `auto` model), reject tools
/// it can't serve, guard the parameters, and invoke it. Chat requests
/// exercise [`Capability::chat`]; the prompt endpoints exercise
/// [`Capability::complete`] and never carry tools.
pub(crate) async fn dispatch(
    port: &dyn GatewayPort,
    identity: &GatewayIdentity,
    model: &str,
    params: &BTreeMap<String, JsonValue>,
    payload: JsonValue,
    request: RouteRequest,
) -> Result<Dispatched, GatewayError> {
    let capability = request.capability().clone();
    let auto = port.auto_route();
    let routing = auto.filter(|auto| auto.answers(model));
    let (record, routed) = match routing {
        Some(auto) => {
            let routed = auto
                .route(port, &identity.scopes, &request, params, &payload)
                .await?;
            authorize(
                port,
                &routed.record,
                &capability,
                GatewayWorkKind::Stream,
                identity,
            )
            .await?;
            (routed.record, Some(routed.decision))
        }
        None => {
            let record = resolve_authorized(
                port,
                model,
                capability.clone(),
                GatewayWorkKind::Stream,
                identity,
            )
            .await
            .map_err(|error| routing_off(error, model, auto.is_some()))?;
            (record, None)
        }
    };
    if request.has_tools() && !record.capabilities.contains(&Capability::tools()) {
        return Err(bad_request(format!(
            "{model} does not support tool calling"
        )));
    }
    let honored = port.honored_params(&record.id, capability.clone()).await?;
    param_guard::require(params, &honored, record.runtime.id.as_ref())?;
    let stream = port.invoke(&record.id, capability.clone(), payload).await?;

    let mut outcome = GatewayOutcome::ok_for(Some(&record.id), Some(&capability));
    let mut headers = Vec::new();
    if let (Some(auto), Some(decision)) = (routing, routed) {
        auto.remember(&request, decision.tier);
        let reason = decision.reason.to_string();
        headers.push((
            ROUTED_MODEL_HEADER.to_owned(),
            header_safe(record.wire_id()),
        ));
        headers.push((ROUTE_REASON_HEADER.to_owned(), header_safe(&reason)));
        outcome = outcome.routed(auto.name(), &reason);
    }
    Ok(Dispatched {
        stream,
        headers,
        outcome,
    })
}

/// A request for `auto` that matched nothing while no routing is configured,
/// told how to turn it on.
fn routing_off(error: GatewayError, model: &str, routing_on: bool) -> GatewayError {
    if routing_on || error.kind != GatewayErrorKind::NotFound || model != DEFAULT_ROUTE_NAME {
        return error;
    }
    GatewayError::new(error.kind, format!("{}; {ROUTING_OFF}", error.message))
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
    respond_json_with(responder, value, Vec::new());
}

/// Send `value` as a `200` JSON response carrying `extra_headers`.
pub(crate) fn respond_json_with(
    responder: &GatewayResponder,
    value: &serde_json::Value,
    extra_headers: Vec<(String, String)>,
) {
    responder.respond(
        200,
        "application/json",
        serde_json::to_vec(value).unwrap_or_default(),
        extra_headers,
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
    fn a_runtime_rejection_is_the_callers_fault_and_any_other_failure_is_ours() {
        use runtime::adapters::RuntimeError;

        let rejected = runtime_failed(RuntimeError::Rejected("options do not fit".to_owned()));
        assert_eq!(rejected.kind, GatewayErrorKind::BadRequest);
        assert_eq!(rejected.message, "options do not fit");
        let failed = runtime_failed(RuntimeError::Failed("out of memory".to_owned()));
        assert_eq!(failed.kind, GatewayErrorKind::ServerError);
    }

    #[test]
    fn a_completion_id_carries_its_prefix_and_is_unique() {
        let first = completion_id("chatcmpl-");
        let second = completion_id("chatcmpl-");
        assert!(first.starts_with("chatcmpl-"));
        assert_eq!(first.len(), "chatcmpl-".len() + 32);
        assert_ne!(first, second);
    }
}
