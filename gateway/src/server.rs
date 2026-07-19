//! The HTTP server: an axum front end that turns each request into a
//! [`GatewayRequest`], drives the [`GatewayRouter`] against a
//! [`GatewayResponder`], and streams the responder's parts back as the response.

use std::convert::Infallible;
use std::sync::Arc;

use axum::Router;
use axum::body::{Body, Bytes, to_bytes};
use axum::extract::{Request, State};
use axum::response::Response;
use tokio::net::TcpListener;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::UnboundedReceiverStream;

use crate::defaults::MAX_BODY_BYTES;
use crate::request::GatewayRequest;
use crate::responder::{GatewayResponder, ResponsePart};
use crate::router::GatewayRouter;

/// The axum application backed by `router`.
pub fn app(router: Arc<GatewayRouter>) -> Router {
    Router::new().fallback(handle).with_state(router)
}

/// Serve the gateway on an already-bound `listener` until it stops.
pub async fn serve(listener: TcpListener, router: Arc<GatewayRouter>) -> std::io::Result<()> {
    axum::serve(listener, app(router)).await
}

/// Bind loopback on `port` (0 picks a free port) and serve until stopped.
pub async fn run(port: u16, router: Arc<GatewayRouter>) -> std::io::Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", port)).await?;
    serve(listener, router).await
}

async fn handle(State(router): State<Arc<GatewayRouter>>, request: Request) -> Response {
    let method = request.method().as_str().to_owned();
    let uri = request
        .uri()
        .path_and_query()
        .map(|target| target.as_str().to_owned())
        .unwrap_or_else(|| request.uri().path().to_owned());
    let headers: Vec<(String, String)> = request
        .headers()
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.as_str().to_owned(), value.to_owned()))
        })
        .collect();

    let limit = router.body_limit(&uri, MAX_BODY_BYTES);
    let body = match to_bytes(request.into_body(), limit).await {
        Ok(bytes) => bytes.to_vec(),
        Err(_) => return status_json(413, br#"{"error":"request body too large"}"#),
    };

    let gateway_request = GatewayRequest::new(&method, &uri, headers, body);
    let (responder, mut parts) = GatewayResponder::new();

    // Dispatch runs concurrently, writing response parts; the head arrives first,
    // then the body streams as chunks land.
    let dispatch_router = Arc::clone(&router);
    tokio::spawn(async move {
        dispatch_router.dispatch(gateway_request, &responder).await;
    });

    match parts.recv().await {
        Some(ResponsePart::Head { status, headers }) => {
            let mut builder = Response::builder().status(status);
            for (name, value) in headers {
                builder = builder.header(name, value);
            }
            let stream = UnboundedReceiverStream::new(parts).filter_map(|part| match part {
                ResponsePart::Chunk(bytes) => Some(Ok::<Bytes, Infallible>(Bytes::from(bytes))),
                ResponsePart::Head { .. } => None,
            });
            builder
                .body(Body::from_stream(stream))
                .unwrap_or_else(|_| Response::new(Body::empty()))
        }
        // The dispatcher produced no response at all.
        _ => status_json(500, br#"{"error":"internal error"}"#),
    }
}

fn status_json(status: u16, body: &'static [u8]) -> Response {
    Response::builder()
        .status(status)
        .header("Content-Type", "application/json")
        .body(Body::from(body))
        .unwrap_or_else(|_| Response::new(Body::empty()))
}
