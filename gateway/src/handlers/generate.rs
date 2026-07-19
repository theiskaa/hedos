//! The text-generation handlers: OpenAI `/v1/completions` and Ollama
//! `/api/generate`. Both take a single prompt (not a chat transcript), invoke the
//! `complete` capability, and stream or accumulate the text in their dialect.

use std::collections::BTreeMap;

use kernel::capabilities::CapabilityChunk;
use kernel::records::{Capability, JsonValue};
use serde_json::json;

use super::{
    GatewayHandling, HandlerFuture, bad_request, collect_completion, completion_id, dispatch,
    required_model, respond_json, runtime_failed,
};
use crate::error::GatewayError;
use crate::identity::{GatewayIdentity, GatewayOutcome};
use crate::port::GatewayPort;
use crate::request::GatewayRequest;
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

            let payload = prompt_payload(prompt, &sampling);
            let (record, mut out) = dispatch(
                port,
                identity,
                model,
                Capability::complete(),
                false,
                &sampling,
                payload,
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
                    body.write(openai::sse_frame(&openai::usage_frame(
                        &id,
                        created,
                        model,
                        "text_completion",
                        final_stats.as_ref(),
                    )));
                }
                body.write(openai::SSE_DONE.to_vec());
                body.end();
            } else {
                let (text, _tool_calls, final_stats) = collect_completion(&mut out).await?;
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

            let payload = prompt_payload(prompt, &options);
            let (record, mut out) = dispatch(
                port,
                identity,
                model,
                Capability::complete(),
                false,
                &options,
                payload,
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
                let (response, _tool_calls, final_stats) = collect_completion(&mut out).await?;
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
