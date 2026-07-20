//! Streaming-handler helpers: racing a drain against a run timeout, and writing a
//! failure into a response that has already begun streaming (where the status is
//! already sent, so the error must go in-band).

use std::future::Future;
use std::time::Duration;

use serde_json::json;

use crate::error::{GatewayError, GatewayErrorKind};
use crate::responder::GatewayStreamBody;
use crate::surface::GatewaySurface;
use crate::wire::{anthropic, ollama, openai};

/// The default run timeout, in seconds, for a streamed response. One value for
/// every dialect handler.
pub const DEFAULT_RUN_TIMEOUT_SECONDS: u64 = 600;

/// Run `drain` with a deadline. Returns `Ok(false)` if it finished in time,
/// `Ok(true)` if the deadline elapsed first (the drain is cancelled), or the
/// drain's own error.
pub async fn race_timeout<F, E>(timeout: Duration, drain: F) -> Result<bool, E>
where
    F: Future<Output = Result<(), E>>,
{
    match tokio::time::timeout(timeout, drain).await {
        Ok(Ok(())) => Ok(false),
        Ok(Err(error)) => Err(error),
        Err(_elapsed) => Ok(true),
    }
}

/// The shared streamed-handler epilogue: run `drain` against a deadline of
/// `seconds`, writing the surface's in-band timeout frame if it elapses or its
/// failure frame (and propagating the error) if the drain fails. One definition
/// of the timeout/failure contract for every dialect.
pub async fn drain_bounded<F>(
    surface: GatewaySurface,
    body: &GatewayStreamBody,
    seconds: u64,
    drain: F,
) -> Result<(), GatewayError>
where
    F: Future<Output = Result<(), GatewayError>>,
{
    match race_timeout(Duration::from_secs(seconds), drain).await {
        Ok(false) => Ok(()),
        Ok(true) => {
            write_timeout(surface, body, seconds);
            Ok(())
        }
        Err(error) => {
            write_failure(surface, body, &error);
            Err(error)
        }
    }
}

/// Write a gateway error into an already-streaming response and end it. On the
/// OpenAI surface this is an SSE error frame followed by `[DONE]`; on Ollama it
/// is a single NDJSON `{"error": …}` line.
pub fn write_failure(surface: GatewaySurface, body: &GatewayStreamBody, error: &GatewayError) {
    match surface {
        GatewaySurface::OpenAI => {
            body.write(openai::sse_frame(&error.body(GatewaySurface::OpenAI)));
            body.write(openai::SSE_DONE.to_vec());
            body.end();
        }
        GatewaySurface::Ollama => {
            body.write(ollama::line(&error.body(GatewaySurface::Ollama)));
            body.end();
        }
        // Anthropic has no [DONE] sentinel: the error event is the last frame.
        GatewaySurface::Anthropic => {
            body.write(anthropic::error_frame(error.kind, &error.message));
            body.end();
        }
    }
}

/// Write a timeout notice into an already-streaming response and end it. The
/// OpenAI frame uses the `timeout_error`/`timeout` type/code (distinct from the
/// generic wrapped-error type).
pub fn write_timeout(surface: GatewaySurface, body: &GatewayStreamBody, seconds: u64) {
    let message = format!("the request timed out after {seconds}s");
    match surface {
        GatewaySurface::OpenAI => {
            let frame = json!({
                "error": { "message": message, "type": "timeout_error", "code": "timeout" }
            });
            body.write(openai::sse_frame(&frame));
            body.write(openai::SSE_DONE.to_vec());
            body.end();
        }
        GatewaySurface::Ollama => {
            body.write(ollama::line(&json!({ "error": message })));
            body.end();
        }
        GatewaySurface::Anthropic => {
            body.write(anthropic::error_frame(GatewayErrorKind::Timeout, &message));
            body.end();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::GatewayErrorKind;
    use crate::responder::{GatewayResponder, ResponsePart};

    fn chunks(rx: &mut tokio::sync::mpsc::UnboundedReceiver<ResponsePart>) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        while let Ok(part) = rx.try_recv() {
            if let ResponsePart::Chunk(bytes) = part {
                out.push(bytes);
            }
        }
        out
    }

    #[tokio::test]
    async fn a_drain_that_finishes_in_time_did_not_time_out() {
        let result: Result<bool, ()> = race_timeout(Duration::from_secs(5), async { Ok(()) }).await;
        assert_eq!(result, Ok(false));
    }

    #[tokio::test]
    async fn a_slow_drain_times_out() {
        let result: Result<bool, ()> = race_timeout(Duration::from_millis(10), async {
            tokio::time::sleep(Duration::from_secs(100)).await;
            Ok(())
        })
        .await;
        assert_eq!(result, Ok(true));
    }

    #[tokio::test]
    async fn a_draining_error_propagates() {
        let result: Result<bool, &str> =
            race_timeout(Duration::from_secs(5), async { Err("boom") }).await;
        assert_eq!(result, Err("boom"));
    }

    #[test]
    fn an_openai_failure_writes_an_sse_error_and_done() {
        let (responder, mut rx) = GatewayResponder::new();
        let body = responder.begin_stream(200, "text/event-stream").unwrap();
        let error = GatewayError::new(GatewayErrorKind::ServerError, "boom");
        write_failure(GatewaySurface::OpenAI, &body, &error);
        let chunks = chunks(&mut rx);
        assert!(chunks[0].starts_with(b"data: "));
        assert!(String::from_utf8_lossy(&chunks[0]).contains("boom"));
        assert_eq!(chunks[1], b"data: [DONE]\n\n");
    }

    #[test]
    fn an_ollama_failure_writes_one_error_line() {
        let (responder, mut rx) = GatewayResponder::new();
        let body = responder.begin_stream(200, "application/x-ndjson").unwrap();
        let error = GatewayError::new(GatewayErrorKind::ServerError, "boom");
        write_failure(GatewaySurface::Ollama, &body, &error);
        let chunks = chunks(&mut rx);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], b"{\"error\":\"boom\"}\n");
    }

    #[test]
    fn an_openai_timeout_uses_the_timeout_type() {
        let (responder, mut rx) = GatewayResponder::new();
        let body = responder.begin_stream(200, "text/event-stream").unwrap();
        write_timeout(GatewaySurface::OpenAI, &body, 600);
        let chunks = chunks(&mut rx);
        let frame = String::from_utf8_lossy(&chunks[0]);
        assert!(frame.contains("timeout_error"));
        assert!(frame.contains("600s"));
    }
}
