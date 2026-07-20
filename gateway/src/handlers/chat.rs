//! The chat handlers: OpenAI `/v1/chat/completions` and Ollama `/api/chat`. Both
//! decode a request, resolve and admit a model, guard its parameters, invoke the
//! kernel, and stream the chunks back in their dialect (or accumulate them into a
//! single response).

use kernel::capabilities::{CapabilityChunk, GenerationStats};
use kernel::records::{Capability, JsonValue};

use super::stream::{DEFAULT_RUN_TIMEOUT_SECONDS, drain_bounded};
use super::{
    GatewayHandling, HandlerFuture, collect_completion, completion_id, dispatch, respond_json,
    runtime_failed,
};
use crate::error::GatewayError;
use crate::identity::{GatewayIdentity, GatewayOutcome};
use crate::port::GatewayPort;
use crate::request::GatewayRequest;
use crate::responder::GatewayResponder;
use crate::surface::GatewaySurface;
use crate::wire::timestamp::{now_iso8601, now_unix_seconds};
use crate::wire::{ollama, openai};

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
            let (record, mut stream) = dispatch(
                port,
                identity,
                &chat.model,
                Capability::chat(),
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
                        body.write(openai::sse_frame(&openai::usage_frame(
                            &id,
                            created,
                            &served,
                            "chat.completion.chunk",
                            final_stats.as_ref(),
                        )));
                    }
                    body.write(openai::SSE_DONE.to_vec());
                    body.end();
                    Ok::<(), GatewayError>(())
                };
                drain_bounded(
                    GatewaySurface::OpenAI,
                    &body,
                    self.run_timeout_seconds,
                    drain,
                )
                .await?;
            } else {
                let (content, tool_calls, final_stats) = collect_completion(&mut stream).await?;
                let body = openai::completion(
                    &id,
                    created,
                    &served,
                    &content,
                    final_stats.as_ref(),
                    &tool_calls,
                );
                respond_json(responder, &body);
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
            let (record, mut stream) = dispatch(
                port,
                identity,
                &chat.model,
                Capability::chat(),
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
                drain_bounded(
                    GatewaySurface::Ollama,
                    &body,
                    self.run_timeout_seconds,
                    drain,
                )
                .await?;
            } else {
                let (content, tool_calls, final_stats) = collect_completion(&mut stream).await?;
                let body = ollama::final_frame(
                    &served,
                    &now_iso8601(),
                    &content,
                    final_stats.as_ref(),
                    &tool_calls,
                );
                respond_json(responder, &body);
            }
            Ok(GatewayOutcome::ok_for(
                Some(&record.id),
                Some(&Capability::chat()),
            ))
        })
    }
}
