//! The response sink handlers write to. Rather than binding to an HTTP library,
//! a responder pushes [`ResponsePart`]s onto a channel; the server layer consumes
//! them to build the actual response (a full body, or a streamed one). The
//! response ends when the responder and any stream body are dropped and the
//! channel closes.

use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::mpsc;

use crate::error::{GatewayError, GatewayErrorKind};

/// One piece of an outgoing response: the head, then zero or more body chunks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResponsePart {
    /// The status line and headers, sent exactly once, first.
    Head {
        /// The HTTP status code.
        status: u16,
        /// The response headers, in order.
        headers: Vec<(String, String)>,
    },
    /// A chunk of the response body.
    Chunk(Vec<u8>),
}

/// A response sink. A handler calls [`respond`](Self::respond) once for a full
/// response, or [`begin_stream`](Self::begin_stream) to send a head and then
/// write body chunks through the returned [`GatewayStreamBody`].
pub struct GatewayResponder {
    tx: mpsc::UnboundedSender<ResponsePart>,
    started: AtomicBool,
}

impl GatewayResponder {
    /// A responder and the receiver the server drains to build the response.
    pub fn new() -> (Self, mpsc::UnboundedReceiver<ResponsePart>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (
            Self {
                tx,
                started: AtomicBool::new(false),
            },
            rx,
        )
    }

    /// Whether the head has already been sent.
    pub fn has_started(&self) -> bool {
        self.started.load(Ordering::Acquire)
    }

    /// Claim the right to send the head; `true` for the first caller only.
    fn start(&self) -> bool {
        !self.started.swap(true, Ordering::AcqRel)
    }

    /// Send a complete response: a head with `status`/`content_type` (plus any
    /// `extra_headers`) and the whole `body`. A no-op if a response already
    /// started, matching the Swift guard.
    pub fn respond(
        &self,
        status: u16,
        content_type: &str,
        body: Vec<u8>,
        extra_headers: Vec<(String, String)>,
    ) {
        if !self.start() {
            return;
        }
        let mut headers = vec![("Content-Type".to_owned(), content_type.to_owned())];
        headers.extend(extra_headers);
        let _ = self.tx.send(ResponsePart::Head { status, headers });
        if !body.is_empty() {
            let _ = self.tx.send(ResponsePart::Chunk(body));
        }
    }

    /// Begin a streamed response, sending the head and returning a body to write
    /// chunks through. Fails if a response already started.
    pub fn begin_stream(
        &self,
        status: u16,
        content_type: &str,
    ) -> Result<GatewayStreamBody, GatewayError> {
        if !self.start() {
            return Err(GatewayError::new(
                GatewayErrorKind::ServerError,
                "response already started",
            ));
        }
        let headers = vec![
            ("Content-Type".to_owned(), content_type.to_owned()),
            ("Cache-Control".to_owned(), "no-cache".to_owned()),
        ];
        let _ = self.tx.send(ResponsePart::Head { status, headers });
        Ok(GatewayStreamBody {
            tx: self.tx.clone(),
            ended: AtomicBool::new(false),
        })
    }
}

/// The body of a streamed response: write chunks, then let it drop (or call
/// [`end`](Self::end)) to stop. Dropping every sender closes the response.
pub struct GatewayStreamBody {
    tx: mpsc::UnboundedSender<ResponsePart>,
    ended: AtomicBool,
}

impl GatewayStreamBody {
    /// Write a body chunk. Empty chunks and writes after [`end`](Self::end) are
    /// dropped.
    pub fn write(&self, data: Vec<u8>) {
        if data.is_empty() || self.ended.load(Ordering::Acquire) {
            return;
        }
        let _ = self.tx.send(ResponsePart::Chunk(data));
    }

    /// Stop the stream: further writes are ignored. The response closes when this
    /// body and its responder are dropped.
    pub fn end(&self) {
        self.ended.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drain(mut rx: mpsc::UnboundedReceiver<ResponsePart>) -> Vec<ResponsePart> {
        let mut parts = Vec::new();
        while let Ok(part) = rx.try_recv() {
            parts.push(part);
        }
        parts
    }

    #[test]
    fn respond_sends_a_head_and_a_body_chunk() {
        let (responder, rx) = GatewayResponder::new();
        responder.respond(200, "application/json", b"{}".to_vec(), Vec::new());
        assert!(responder.has_started());
        let parts = drain(rx);
        assert_eq!(
            parts[0],
            ResponsePart::Head {
                status: 200,
                headers: vec![("Content-Type".to_owned(), "application/json".to_owned())],
            }
        );
        assert_eq!(parts[1], ResponsePart::Chunk(b"{}".to_vec()));
    }

    #[test]
    fn respond_with_an_empty_body_sends_only_a_head() {
        let (responder, rx) = GatewayResponder::new();
        responder.respond(204, "application/json", Vec::new(), Vec::new());
        assert_eq!(drain(rx).len(), 1);
    }

    #[test]
    fn extra_headers_follow_the_content_type() {
        let (responder, rx) = GatewayResponder::new();
        responder.respond(
            503,
            "application/json",
            b"x".to_vec(),
            vec![("Retry-After".to_owned(), "1".to_owned())],
        );
        let parts = drain(rx);
        let ResponsePart::Head { headers, .. } = &parts[0] else {
            panic!("expected head");
        };
        assert_eq!(headers[0].0, "Content-Type");
        assert_eq!(headers[1], ("Retry-After".to_owned(), "1".to_owned()));
    }

    #[test]
    fn a_second_respond_is_a_no_op() {
        let (responder, rx) = GatewayResponder::new();
        responder.respond(200, "application/json", b"first".to_vec(), Vec::new());
        responder.respond(500, "application/json", b"second".to_vec(), Vec::new());
        let parts = drain(rx);
        // Only the first response was sent.
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[1], ResponsePart::Chunk(b"first".to_vec()));
    }

    #[test]
    fn a_stream_sends_a_head_then_chunks() {
        let (responder, rx) = GatewayResponder::new();
        let body = responder.begin_stream(200, "text/event-stream").unwrap();
        body.write(b"data: 1\n\n".to_vec());
        body.write(Vec::new()); // dropped
        body.write(b"data: 2\n\n".to_vec());
        let parts = drain(rx);
        assert!(matches!(parts[0], ResponsePart::Head { status: 200, .. }));
        assert_eq!(parts[1], ResponsePart::Chunk(b"data: 1\n\n".to_vec()));
        assert_eq!(parts[2], ResponsePart::Chunk(b"data: 2\n\n".to_vec()));
    }

    #[test]
    fn writes_after_end_are_ignored() {
        let (responder, rx) = GatewayResponder::new();
        let body = responder.begin_stream(200, "text/event-stream").unwrap();
        body.write(b"kept".to_vec());
        body.end();
        body.write(b"dropped".to_vec());
        let parts = drain(rx);
        // Head + the one kept chunk.
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[1], ResponsePart::Chunk(b"kept".to_vec()));
    }

    #[test]
    fn begin_stream_after_respond_fails() {
        let (responder, _rx) = GatewayResponder::new();
        responder.respond(200, "application/json", b"x".to_vec(), Vec::new());
        assert!(responder.begin_stream(200, "text/event-stream").is_err());
    }
}
