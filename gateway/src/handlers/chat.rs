//! The chat handlers: OpenAI `/v1/chat/completions` and Ollama `/api/chat`. Both
//! decode a request, resolve and admit a model, guard its parameters, invoke the
//! kernel, and stream the chunks back in their dialect (or accumulate them into a
//! single response).

use std::collections::BTreeMap;
use std::time::Duration;

use kernel::capabilities::{CapabilityChunk, GenerationStats};
use kernel::records::{Capability, JsonValue, ModelRecord};
use runtime::adapters::ChunkStream;
use serde_json::json;

use super::stream::{race_timeout, write_failure, write_timeout};
use super::{GatewayHandling, HandlerFuture, completion_id};
use super::{bad_request, runtime_failed};
use crate::admission::GatewayWorkKind;
use crate::error::GatewayError;
use crate::identity::{GatewayIdentity, GatewayOutcome};
use crate::param_guard;
use crate::port::GatewayPort;
use crate::request::GatewayRequest;
use crate::resolver::resolve_authorized;
use crate::responder::GatewayResponder;
use crate::surface::GatewaySurface;
use crate::wire::timestamp::{now_iso8601, now_unix_seconds};
use crate::wire::{ollama, openai};

/// The default run timeout, in seconds, for a streamed chat.
const DEFAULT_RUN_TIMEOUT_SECONDS: u64 = 600;

/// The shared chat prefix: resolve and admit the model, reject tools it can't
/// serve, guard the parameters, and invoke it — yielding the record and its
/// chunk stream.
async fn dispatch_chat(
    port: &dyn GatewayPort,
    identity: &GatewayIdentity,
    model: &str,
    tools_present: bool,
    params: &BTreeMap<String, JsonValue>,
    payload: JsonValue,
) -> Result<(ModelRecord, ChunkStream), GatewayError> {
    let record = resolve_authorized(
        port,
        model,
        Capability::chat(),
        GatewayWorkKind::Stream,
        identity,
    )
    .await?;
    if tools_present && !port.supports_tools(&record.id).await {
        return Err(bad_request(format!(
            "{model} does not support tool calling"
        )));
    }
    let honored = port.honored_params(&record.id, Capability::chat()).await?;
    param_guard::require(params, &honored, record.runtime.id.as_ref())?;
    let stream = port.invoke(&record.id, Capability::chat(), payload).await?;
    Ok((record, stream))
}

/// The OpenAI chat-completions handler.
pub struct OpenAIChatHandler {
    /// How long a streamed run may take before it times out.
    pub run_timeout_seconds: u64,
}

impl Default for OpenAIChatHandler {
    fn default() -> Self {
        Self {
            run_timeout_seconds: DEFAULT_RUN_TIMEOUT_SECONDS,
        }
    }
}

impl GatewayHandling for OpenAIChatHandler {
    fn handle<'a>(
        &'a self,
        request: &'a GatewayRequest,
        identity: &'a GatewayIdentity,
        port: &'a dyn GatewayPort,
        responder: &'a GatewayResponder,
    ) -> HandlerFuture<'a> {
        Box::pin(async move {
            let mut chat = openai::decode_chat_request(request.decoded_json()?)?;
            // `tool_choice: "none"` disables tools entirely.
            if matches!(&chat.tool_choice, Some(JsonValue::String(choice)) if choice == "none") {
                chat.tools.clear();
            }
            let payload = openai::chat_payload(&chat);
            let (record, mut stream) = dispatch_chat(
                port,
                identity,
                &chat.model,
                !chat.tools.is_empty(),
                &chat.sampling,
                payload,
            )
            .await?;

            let id = completion_id("chatcmpl-");
            let created = now_unix_seconds();
            let served = chat.model.clone();
            let include_usage = chat.include_usage;

            if chat.stream {
                let body = responder.begin_stream(200, "text/event-stream")?;
                let drain = async {
                    let mut first = true;
                    let mut final_stats: Option<GenerationStats> = None;
                    let mut tool_calls = 0usize;
                    while let Some(result) = stream.recv().await {
                        match result.map_err(runtime_failed)? {
                            CapabilityChunk::Text(text) => {
                                let mut chunk = openai::StreamChunk::new(&id, created, &served);
                                chunk.content = Some(&text);
                                chunk.role = first;
                                body.write(openai::sse_frame(&chunk.to_value()));
                                first = false;
                            }
                            CapabilityChunk::Thinking(thought) => {
                                let mut chunk = openai::StreamChunk::new(&id, created, &served);
                                chunk.reasoning = Some(&thought);
                                chunk.role = first;
                                body.write(openai::sse_frame(&chunk.to_value()));
                                first = false;
                            }
                            CapabilityChunk::ToolCall(call) => {
                                let mut chunk = openai::StreamChunk::new(&id, created, &served);
                                chunk.tool_call = Some(&call);
                                chunk.tool_call_index = tool_calls;
                                chunk.role = first;
                                body.write(openai::sse_frame(&chunk.to_value()));
                                first = false;
                                tool_calls += 1;
                            }
                            CapabilityChunk::Done(stats) => final_stats = stats,
                            _ => {}
                        }
                    }
                    let finish = if tool_calls > 0 {
                        "tool_calls".to_owned()
                    } else {
                        final_stats
                            .as_ref()
                            .and_then(|stats| stats.finish_reason.clone())
                            .unwrap_or_else(|| "stop".to_owned())
                    };
                    let mut final_chunk = openai::StreamChunk::new(&id, created, &served);
                    final_chunk.finish_reason = Some(&finish);
                    body.write(openai::sse_frame(&final_chunk.to_value()));
                    if include_usage {
                        let usage = json!({
                            "id": id,
                            "object": "chat.completion.chunk",
                            "created": created,
                            "model": served,
                            "choices": [],
                            "usage": openai::usage(final_stats.as_ref()),
                        });
                        body.write(openai::sse_frame(&usage));
                    }
                    body.write(openai::SSE_DONE.to_vec());
                    body.end();
                    Ok::<(), GatewayError>(())
                };
                match race_timeout(Duration::from_secs(self.run_timeout_seconds), drain).await {
                    Ok(false) => {}
                    Ok(true) => {
                        write_timeout(GatewaySurface::OpenAI, &body, self.run_timeout_seconds)
                    }
                    Err(error) => {
                        write_failure(GatewaySurface::OpenAI, &body, &error);
                        return Err(error);
                    }
                }
            } else {
                let mut content = String::new();
                let mut final_stats: Option<GenerationStats> = None;
                let mut tool_calls = Vec::new();
                while let Some(result) = stream.recv().await {
                    match result.map_err(runtime_failed)? {
                        CapabilityChunk::Text(text) => content.push_str(&text),
                        CapabilityChunk::ToolCall(call) => tool_calls.push(call),
                        CapabilityChunk::Done(stats) => final_stats = stats,
                        _ => {}
                    }
                }
                let body = openai::completion(
                    &id,
                    created,
                    &served,
                    &content,
                    final_stats.as_ref(),
                    &tool_calls,
                );
                responder.respond(
                    200,
                    "application/json",
                    serde_json::to_vec(&body).unwrap_or_default(),
                    Vec::new(),
                );
            }
            Ok(GatewayOutcome::ok_for(
                Some(&record.id),
                Some(&Capability::chat()),
            ))
        })
    }
}

/// The Ollama chat handler.
pub struct OllamaChatHandler {
    /// How long a streamed run may take before it times out.
    pub run_timeout_seconds: u64,
}

impl Default for OllamaChatHandler {
    fn default() -> Self {
        Self {
            run_timeout_seconds: DEFAULT_RUN_TIMEOUT_SECONDS,
        }
    }
}

impl GatewayHandling for OllamaChatHandler {
    fn handle<'a>(
        &'a self,
        request: &'a GatewayRequest,
        identity: &'a GatewayIdentity,
        port: &'a dyn GatewayPort,
        responder: &'a GatewayResponder,
    ) -> HandlerFuture<'a> {
        Box::pin(async move {
            let chat = ollama::decode_chat_request(&request.decoded_json()?)?;
            let payload = ollama::chat_payload(&chat);
            let (record, mut stream) = dispatch_chat(
                port,
                identity,
                &chat.model,
                !chat.tools.is_empty(),
                &chat.options,
                payload,
            )
            .await?;

            // Each frame carries a fresh timestamp, as Ollama does.
            let served = chat.model.clone();

            if chat.stream {
                let body = responder.begin_stream(200, "application/x-ndjson")?;
                let drain = async {
                    let mut final_stats: Option<GenerationStats> = None;
                    let mut saw_tool_call = false;
                    while let Some(result) = stream.recv().await {
                        match result.map_err(runtime_failed)? {
                            CapabilityChunk::Text(text) => {
                                let frame =
                                    ollama::delta(&served, &now_iso8601(), Some(&text), None, None);
                                body.write(ollama::line(&frame));
                            }
                            CapabilityChunk::Thinking(thought) => {
                                let frame = ollama::delta(
                                    &served,
                                    &now_iso8601(),
                                    None,
                                    Some(&thought),
                                    None,
                                );
                                body.write(ollama::line(&frame));
                            }
                            CapabilityChunk::ToolCall(call) => {
                                saw_tool_call = true;
                                let frame =
                                    ollama::delta(&served, &now_iso8601(), None, None, Some(&call));
                                body.write(ollama::line(&frame));
                            }
                            CapabilityChunk::Done(stats) => final_stats = stats,
                            _ => {}
                        }
                    }
                    let mut stats = final_stats.unwrap_or_default();
                    if saw_tool_call {
                        stats.finish_reason = Some("stop".to_owned());
                    }
                    let frame = ollama::final_frame(&served, &now_iso8601(), "", Some(&stats), &[]);
                    body.write(ollama::line(&frame));
                    body.end();
                    Ok::<(), GatewayError>(())
                };
                match race_timeout(Duration::from_secs(self.run_timeout_seconds), drain).await {
                    Ok(false) => {}
                    Ok(true) => {
                        write_timeout(GatewaySurface::Ollama, &body, self.run_timeout_seconds)
                    }
                    Err(error) => {
                        write_failure(GatewaySurface::Ollama, &body, &error);
                        return Err(error);
                    }
                }
            } else {
                let mut content = String::new();
                let mut final_stats: Option<GenerationStats> = None;
                let mut tool_calls = Vec::new();
                while let Some(result) = stream.recv().await {
                    match result.map_err(runtime_failed)? {
                        CapabilityChunk::Text(text) => content.push_str(&text),
                        CapabilityChunk::ToolCall(call) => tool_calls.push(call),
                        CapabilityChunk::Done(stats) => final_stats = stats,
                        _ => {}
                    }
                }
                let body = ollama::final_frame(
                    &served,
                    &now_iso8601(),
                    &content,
                    final_stats.as_ref(),
                    &tool_calls,
                );
                responder.respond(
                    200,
                    "application/json",
                    serde_json::to_vec(&body).unwrap_or_default(),
                    Vec::new(),
                );
            }
            Ok(GatewayOutcome::ok_for(
                Some(&record.id),
                Some(&Capability::chat()),
            ))
        })
    }
}
