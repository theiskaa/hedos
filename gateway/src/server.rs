//! The HTTP server: an axum front end that turns each request into a
//! [`GatewayRequest`], drives the [`GatewayRouter`] against a
//! [`GatewayResponder`], and streams the responder's parts back as the response.

use std::convert::Infallible;
use std::future::Future;
use std::sync::Arc;

use axum::Router;
use axum::body::{Body, Bytes, to_bytes};
use axum::extract::{Request, State};
use axum::response::Response;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::UnboundedReceiverStream;

use crate::defaults::MAX_BODY_BYTES;
use crate::request::GatewayRequest;
use crate::responder::{GatewayResponder, ResponsePart};
use crate::router::GatewayRouter;
use crate::wire::timestamp::now_unix_millis;

/// What every request is served with: the router, and whether the requests
/// still in flight have been cut.
#[derive(Clone)]
struct Served {
    router: Arc<GatewayRouter>,
    cut: watch::Receiver<bool>,
}

/// The axum application backed by `router`.
pub fn app(router: Arc<GatewayRouter>) -> Router {
    let (_, never_cut) = watch::channel(false);
    app_with_cut(router, never_cut)
}

fn app_with_cut(router: Arc<GatewayRouter>, cut: watch::Receiver<bool>) -> Router {
    Router::new()
        .fallback(handle)
        .with_state(Served { router, cut })
}

/// Serve the gateway on an already-bound `listener` until it stops.
pub async fn serve(listener: TcpListener, router: Arc<GatewayRouter>) -> std::io::Result<()> {
    serve_with_shutdown(listener, router, std::future::pending()).await
}

/// Serve on `listener` until `shutdown` resolves, then drain in-flight requests
/// and flush the audit log so a pending coalesced summary is not lost at exit.
pub async fn serve_with_shutdown<S>(
    listener: TcpListener,
    router: Arc<GatewayRouter>,
    shutdown: S,
) -> std::io::Result<()>
where
    S: Future<Output = ()> + Send + 'static,
{
    serve_with_cut(listener, router, shutdown, std::future::pending()).await
}

/// [`serve_with_shutdown`], except that the requests still in flight once
/// `cut` resolves are ended rather than waited for. One not yet answered is
/// answered `503` "the gateway is stopping"; a streamed answer ends where it
/// stands, its body closed as a complete one, so a client sees a short answer
/// rather than a broken connection. Either way the work behind it stops.
pub async fn serve_with_cut<S, C>(
    listener: TcpListener,
    router: Arc<GatewayRouter>,
    shutdown: S,
    cut: C,
) -> std::io::Result<()>
where
    S: Future<Output = ()> + Send + 'static,
    C: Future<Output = ()> + Send + 'static,
{
    let (cutting, cut_off) = watch::channel(false);
    let cutter = tokio::spawn(async move {
        cut.await;
        let _ = cutting.send(true);
    });
    let result = axum::serve(listener, app_with_cut(Arc::clone(&router), cut_off))
        .with_graceful_shutdown(shutdown)
        .await;
    cutter.abort();
    router.flush_audit();
    result
}

/// Bind loopback on `port` (0 picks a free port) and serve until stopped.
pub async fn run(port: u16, router: Arc<GatewayRouter>) -> std::io::Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", port)).await?;
    serve(listener, router).await
}

async fn handle(State(served): State<Served>, request: Request) -> Response {
    let Served { router, cut } = served;
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
    let started = now_unix_millis();
    let (responder, mut parts) = GatewayResponder::new();

    // Dispatch runs concurrently, writing response parts; the head arrives first,
    // then the body streams as chunks land. A cut drops it, and the work it was
    // waiting on with it; dropping the responder then ends the response.
    let dispatch_router = Arc::clone(&router);
    tokio::spawn(async move {
        let stopped = tokio::select! {
            biased;
            () = cut_requested(cut) => true,
            () = dispatch_router.dispatch(&gateway_request, &responder) => false,
        };
        if stopped {
            dispatch_router
                .stopped(&gateway_request, started, &responder)
                .await;
        }
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

/// Resolve once the requests in flight are cut; never, when nothing will cut
/// them.
async fn cut_requested(mut cut: watch::Receiver<bool>) {
    if cut.wait_for(|cut| *cut).await.is_err() {
        std::future::pending::<()>().await;
    }
}

fn status_json(status: u16, body: &'static [u8]) -> Response {
    Response::builder()
        .status(status)
        .header("Content-Type", "application/json")
        .body(Body::from(body))
        .unwrap_or_else(|_| Response::new(Body::empty()))
}
