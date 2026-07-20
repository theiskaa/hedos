//! The model-listing handlers: OpenAI `/v1/models`, Ollama `/api/tags`, and the
//! Ollama `/api/version` and `/api/show` handshakes stock clients probe.

use kernel::records::{Capability, ModelRecord, ModelState};
use serde_json::json;

use super::{GatewayHandling, HandlerFuture, respond_json};
use crate::error::{GatewayError, GatewayErrorKind};
use crate::identity::{GatewayIdentity, GatewayOutcome};
use crate::port::GatewayPort;
use crate::request::GatewayRequest;
use crate::resolver::resolve;
use crate::responder::GatewayResponder;
use crate::wire::{ollama, openai};

/// The Ollama version this gateway reports to stock clients.
const OLLAMA_VERSION: &str = "0.5.0";

fn ready(shelf: Vec<ModelRecord>) -> Vec<ModelRecord> {
    shelf
        .into_iter()
        .filter(|record| record.state == ModelState::Ready)
        .collect()
}

/// `GET /v1/models`: every ready model the token can reach.
pub struct OpenAIModelsHandler;

impl GatewayHandling for OpenAIModelsHandler {
    fn handle<'a>(
        &'a self,
        _request: &'a GatewayRequest,
        identity: &'a GatewayIdentity,
        port: &'a dyn GatewayPort,
        responder: &'a GatewayResponder,
    ) -> HandlerFuture<'a> {
        Box::pin(async move {
            let visible = identity.scopes.filter(&ready(port.shelf().await));
            respond_json(responder, &openai::models_list(&visible));
            Ok(GatewayOutcome::ok())
        })
    }
}

/// `GET /api/tags`: every ready chat model the token can reach, Ollama-style.
pub struct OllamaTagsHandler;

impl GatewayHandling for OllamaTagsHandler {
    fn handle<'a>(
        &'a self,
        _request: &'a GatewayRequest,
        identity: &'a GatewayIdentity,
        port: &'a dyn GatewayPort,
        responder: &'a GatewayResponder,
    ) -> HandlerFuture<'a> {
        Box::pin(async move {
            let chat_models: Vec<ModelRecord> = ready(port.shelf().await)
                .into_iter()
                .filter(|record| record.capabilities.contains(&Capability::chat()))
                .collect();
            let visible = identity.scopes.filter(&chat_models);
            respond_json(responder, &ollama::tags(&visible));
            Ok(GatewayOutcome::ok())
        })
    }
}

/// `GET /api/version`: the version handshake for stock Ollama clients.
pub struct OllamaVersionHandler;

impl GatewayHandling for OllamaVersionHandler {
    fn handle<'a>(
        &'a self,
        _request: &'a GatewayRequest,
        _identity: &'a GatewayIdentity,
        _port: &'a dyn GatewayPort,
        responder: &'a GatewayResponder,
    ) -> HandlerFuture<'a> {
        Box::pin(async move {
            respond_json(responder, &json!({ "version": OLLAMA_VERSION }));
            Ok(GatewayOutcome::ok())
        })
    }
}

/// `POST /api/show`: the model-details handshake Ollama clients make.
pub struct OllamaShowHandler;

impl GatewayHandling for OllamaShowHandler {
    fn handle<'a>(
        &'a self,
        request: &'a GatewayRequest,
        identity: &'a GatewayIdentity,
        port: &'a dyn GatewayPort,
        responder: &'a GatewayResponder,
    ) -> HandlerFuture<'a> {
        Box::pin(async move {
            let body = request.decoded_json()?;
            let requested = body
                .get("model")
                .or_else(|| body.get("name"))
                .and_then(kernel::records::JsonValue::as_str)
                .ok_or_else(|| {
                    GatewayError::new(GatewayErrorKind::BadRequest, "model is required")
                })?;
            let shelf = port.shelf().await;
            let record = resolve(requested, &shelf, &identity.scopes)?;

            let mut capabilities = Vec::new();
            if record.capabilities.contains(&Capability::chat())
                || record.capabilities.contains(&Capability::complete())
            {
                capabilities.push("completion");
            }
            if record.capabilities.contains(&Capability::embed()) {
                capabilities.push("embedding");
            }
            if record.capabilities.contains(&Capability::see()) {
                capabilities.push("vision");
            }
            if record.capabilities.contains(&Capability::tools()) {
                capabilities.push("tools");
            }

            respond_json(
                responder,
                &json!({
                    "details": ollama::details(&record),
                    "capabilities": capabilities,
                    "model_info": {},
                }),
            );
            Ok(GatewayOutcome::ok_for(Some(&record.id), None))
        })
    }
}
