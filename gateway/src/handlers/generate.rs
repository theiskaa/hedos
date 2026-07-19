//! The text-generation handlers: OpenAI `/v1/completions` and Ollama
//! `/api/generate`. Both take a single prompt (not a chat transcript), invoke the
//! `complete` capability, and stream or accumulate the text in their dialect.

use std::collections::BTreeMap;

use kernel::capabilities::CapabilityChunk;
use kernel::records::{Capability, JsonValue};
use serde_json::json;

use super::{
    GatewayHandling, HandlerFuture, bad_request, completion_id, required_model, respond_json,
    runtime_failed,
};
use crate::admission::GatewayWorkKind;
use crate::error::GatewayError;
use crate::identity::{GatewayIdentity, GatewayOutcome};
use crate::param_guard;
use crate::port::GatewayPort;
use crate::request::GatewayRequest;
use crate::resolver::resolve_authorized;
use crate::responder::GatewayResponder;
use crate::wire::timestamp::{now_iso8601, now_unix_seconds};
use crate::wire::{ollama, openai, param_decoding};

const COMPLETIONS_HONORED_KEYS: &[&str] = &[
    "model",
    "prompt",
    "stream",
    "stream_options",
    "temperature",
    "top_p",
    "max_tokens",
    "max_completion_tokens",
    "stop",
    "seed",
    "n",
    "best_of",
    "frequency_penalty",
    "presence_penalty",
    "user",
];

const GENERATE_HONORED_KEYS: &[&str] = &["model", "prompt", "stream", "think", "format", "options"];

/// Merge the honored `params` into a `{"prompt": …}` invoke payload.
fn prompt_payload(prompt: String, params: &BTreeMap<String, JsonValue>) -> JsonValue {
    let mut payload = BTreeMap::new();
    payload.insert("prompt".to_owned(), JsonValue::String(prompt));
    for (key, value) in params {
        payload.insert(key.clone(), value.clone());
    }
    JsonValue::Object(payload)
}

/// `POST /v1/completions`.
pub struct OpenAICompletionsHandler;

impl OpenAICompletionsHandler {
    fn prompt(body: &BTreeMap<String, JsonValue>) -> Result<String, GatewayError> {
        match body.get("prompt") {
            Some(JsonValue::String(prompt)) => Ok(prompt.clone()),
            Some(JsonValue::Array(items)) => match items.as_slice() {
                [JsonValue::String(single)] => Ok(single.clone()),
                _ => Err(bad_request("prompt array must hold exactly one string")
                    .with_code("unsupported_parameter")),
            },
            _ => Err(bad_request("prompt is required")),
        }
    }
}

impl GatewayHandling for OpenAICompletionsHandler {
    fn handle<'a>(
        &'a self,
        request: &'a GatewayRequest,
        identity: &'a GatewayIdentity,
        port: &'a dyn GatewayPort,
        responder: &'a GatewayResponder,
    ) -> HandlerFuture<'a> {
        Box::pin(async move {
            let body = request.decoded_json()?;
            let model = required_model(&body)?;
            let prompt = Self::prompt(&body)?;
            param_decoding::reject_unknown_keys(&body, COMPLETIONS_HONORED_KEYS, "parameter")?;
            if matches!(body.get("best_of"), Some(JsonValue::Int(best)) if *best != 1) {
                return Err(bad_request("best_of greater than 1 is not supported")
                    .with_code("unsupported_parameter"));
            }
            let sampling = openai::decode_sampling(&body)?;
            let stream = body
                .get("stream")
                .and_then(JsonValue::as_bool)
                .unwrap_or(false);
            let include_usage = body
                .get("stream_options")
                .and_then(JsonValue::as_object)
                .and_then(|options| options.get("include_usage"))
                .and_then(JsonValue::as_bool)
                .unwrap_or(false);

            let record = resolve_authorized(
                port,
                model,
                Capability::complete(),
                GatewayWorkKind::Stream,
                identity,
            )
            .await?;
            let honored = port
                .honored_params(&record.id, Capability::complete())
                .await?;
            param_guard::require(&sampling, &honored, record.runtime.id.as_ref())?;
            let mut out = port
                .invoke(
                    &record.id,
                    Capability::complete(),
                    prompt_payload(prompt, &sampling),
                )
                .await?;

            let id = completion_id("cmpl-");
            let created = now_unix_seconds();
            if stream {
                let body = responder.begin_stream(200, "text/event-stream")?;
                let mut final_stats = None;
                while let Some(result) = out.recv().await {
                    match result.map_err(runtime_failed)? {
                        CapabilityChunk::Text(text) => body.write(openai::sse_frame(
                            &openai::text_completion_chunk(&id, created, model, &text, None),
                        )),
                        CapabilityChunk::Done(stats) => final_stats = stats,
                        _ => {}
                    }
                }
                let finish = final_stats
                    .as_ref()
                    .and_then(|stats| stats.finish_reason.clone())
                    .unwrap_or_else(|| "stop".to_owned());
                body.write(openai::sse_frame(&openai::text_completion_chunk(
                    &id,
                    created,
                    model,
                    "",
                    Some(&finish),
                )));
                if include_usage {
                    body.write(openai::sse_frame(&json!({
                        "id": id,
                        "object": "text_completion",
                        "created": created,
                        "model": model,
                        "choices": [],
                        "usage": openai::usage(final_stats.as_ref()),
                    })));
                }
                body.write(openai::SSE_DONE.to_vec());
                body.end();
            } else {
                let mut text = String::new();
                let mut final_stats = None;
                while let Some(result) = out.recv().await {
                    match result.map_err(runtime_failed)? {
                        CapabilityChunk::Text(chunk) => text.push_str(&chunk),
                        CapabilityChunk::Done(stats) => final_stats = stats,
                        _ => {}
                    }
                }
                respond_json(
                    responder,
                    &openai::text_completion(&id, created, model, &text, final_stats.as_ref()),
                );
            }
            Ok(GatewayOutcome::ok_for(
                Some(&record.id),
                Some(&Capability::complete()),
            ))
        })
    }
}

/// `POST /api/generate`.
pub struct OllamaGenerateHandler;

impl GatewayHandling for OllamaGenerateHandler {
    fn handle<'a>(
        &'a self,
        request: &'a GatewayRequest,
        identity: &'a GatewayIdentity,
        port: &'a dyn GatewayPort,
        responder: &'a GatewayResponder,
    ) -> HandlerFuture<'a> {
        Box::pin(async move {
            let body = request.decoded_json()?;
            let model = required_model(&body)?;
            let prompt = body
                .get("prompt")
                .and_then(JsonValue::as_str)
                .ok_or_else(|| bad_request("prompt is required"))?
                .to_owned();
            param_decoding::reject_unknown_keys(&body, GENERATE_HONORED_KEYS, "parameter")?;
            let options = ollama::decode_options(&body)?;
            // Ollama defaults `stream` to true.
            let stream = body
                .get("stream")
                .and_then(JsonValue::as_bool)
                .unwrap_or(true);

            let record = resolve_authorized(
                port,
                model,
                Capability::complete(),
                GatewayWorkKind::Stream,
                identity,
            )
            .await?;
            let honored = port
                .honored_params(&record.id, Capability::complete())
                .await?;
            param_guard::require(&options, &honored, record.runtime.id.as_ref())?;
            let mut out = port
                .invoke(
                    &record.id,
                    Capability::complete(),
                    prompt_payload(prompt, &options),
                )
                .await?;

            if stream {
                let body = responder.begin_stream(200, "application/x-ndjson")?;
                let mut final_stats = None;
                while let Some(result) = out.recv().await {
                    match result.map_err(runtime_failed)? {
                        CapabilityChunk::Text(text) => body.write(ollama::line(
                            &ollama::generate_delta(model, &now_iso8601(), &text),
                        )),
                        CapabilityChunk::Done(stats) => final_stats = stats,
                        _ => {}
                    }
                }
                body.write(ollama::line(&ollama::generate_final(
                    model,
                    &now_iso8601(),
                    final_stats.as_ref(),
                )));
                body.end();
            } else {
                let mut response = String::new();
                let mut final_stats = None;
                while let Some(result) = out.recv().await {
                    match result.map_err(runtime_failed)? {
                        CapabilityChunk::Text(chunk) => response.push_str(&chunk),
                        CapabilityChunk::Done(stats) => final_stats = stats,
                        _ => {}
                    }
                }
                let mut object =
                    ollama::generate_final(model, &now_iso8601(), final_stats.as_ref());
                object["response"] = json!(response);
                respond_json(responder, &object);
            }
            Ok(GatewayOutcome::ok_for(
                Some(&record.id),
                Some(&Capability::complete()),
            ))
        })
    }
}
