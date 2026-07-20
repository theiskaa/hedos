//! The Anthropic messages handler: `POST /v1/messages`.
//!
//! Streaming is not optional on this surface. Claude Code consumes server-sent
//! events as they arrive, and a gateway that buffers a whole response before
//! relaying it stalls the client, so the streamed path is the one that matters
//! even though the unary path is implemented for completeness.

use kernel::capabilities::{CapabilityChunk, GenerationStats};
use kernel::records::Capability;

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
use crate::wire::anthropic;

/// The Anthropic messages handler.
pub struct AnthropicMessagesHandler {
    /// How long a streamed run may take before it times out.
    pub run_timeout_seconds: u64,
}

impl Default for AnthropicMessagesHandler {
    fn default() -> Self {
        Self {
            run_timeout_seconds: DEFAULT_RUN_TIMEOUT_SECONDS,
        }
    }
}

impl GatewayHandling for AnthropicMessagesHandler {
    fn handle<'a>(
        &'a self,
        request: &'a GatewayRequest,
        identity: &'a GatewayIdentity,
        port: &'a dyn GatewayPort,
        responder: &'a GatewayResponder,
    ) -> HandlerFuture<'a> {
        Box::pin(async move {
            let message = anthropic::decode_messages_request(request.decoded_json()?)?;
            let payload = anthropic::messages_payload(&message);
            let (record, mut stream) = dispatch(
                port,
                identity,
                &message.model,
                Capability::chat(),
                !message.tools.is_empty(),
                &message.sampling,
                payload,
            )
            .await?;

            let id = completion_id("msg_");
            let served = message.model.clone();

            if message.stream {
                let body = responder.begin_stream(200, "text/event-stream")?;
                let drain = async {
                    body.write(anthropic::message_start(&id, &served, None));

                    // Anthropic indexes content blocks within the message, and a
                    // block must be opened and closed around its deltas. Text
                    // arrives incrementally so its block stays open across
                    // chunks; each tool call is a complete block of its own.
                    let mut index = 0usize;
                    let mut text_open = false;
                    let mut tool_calls = 0usize;
                    let mut final_stats: Option<GenerationStats> = None;

                    while let Some(result) = stream.recv().await {
                        match result.map_err(runtime_failed)? {
                            CapabilityChunk::Text(text) => {
                                if !text_open {
                                    body.write(anthropic::text_block_start(index));
                                    text_open = true;
                                }
                                body.write(anthropic::text_delta(index, &text));
                            }
                            CapabilityChunk::ToolCall(call) => {
                                if text_open {
                                    body.write(anthropic::block_stop(index));
                                    text_open = false;
                                    index += 1;
                                }
                                body.write(anthropic::tool_block_start(index, &call));
                                body.write(anthropic::tool_input_delta(index, &call));
                                body.write(anthropic::block_stop(index));
                                index += 1;
                                tool_calls += 1;
                            }
                            CapabilityChunk::Done(stats) => final_stats = stats,
                            // Thinking is dropped rather than sent as a thinking
                            // block: those carry a signature this gateway cannot
                            // issue, and Claude Code replays blocks it receives.
                            _ => {}
                        }
                    }
                    if text_open {
                        body.write(anthropic::block_stop(index));
                    }
                    body.write(anthropic::message_delta(tool_calls, final_stats.as_ref()));
                    body.write(anthropic::message_stop());
                    body.end();
                    Ok::<(), GatewayError>(())
                };
                drain_bounded(
                    GatewaySurface::Anthropic,
                    &body,
                    self.run_timeout_seconds,
                    drain,
                )
                .await?;
            } else {
                let (content, tool_calls, final_stats) = collect_completion(&mut stream).await?;
                let value =
                    anthropic::message(&id, &served, &content, &tool_calls, final_stats.as_ref());
                respond_json(responder, &value);
            }
            Ok(GatewayOutcome::ok_for(
                Some(&record.id),
                Some(&Capability::chat()),
            ))
        })
    }
}
