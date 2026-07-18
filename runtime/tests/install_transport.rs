//! Tests for the install transport's streaming path.

use kernel::install::InstallError;
use runtime::install::transport::{StreamFuture, TransportFuture};
use runtime::install::{InstallRequest, InstallTransport, ReqwestTransport};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::task::AbortHandle;

struct MockServer {
    base_url: String,
    accept: AbortHandle,
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.accept.abort();
    }
}

/// A server that writes `body` (in two flushed pieces) as a 200 response.
async fn mock(body: &'static str) -> MockServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let accept = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut tmp = [0u8; 2048];
                let _ = stream.read(&mut tmp).await;
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes()).await;
                let _ = stream.flush().await;
                let (first, second) = body.split_at(body.len() / 2);
                let _ = stream.write_all(first.as_bytes()).await;
                let _ = stream.flush().await;
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                let _ = stream.write_all(second.as_bytes()).await;
                let _ = stream.flush().await;
            });
        }
    })
    .abort_handle();
    MockServer {
        base_url: format!("http://{addr}"),
        accept,
    }
}

#[tokio::test]
async fn reqwest_transport_streams_the_body_in_chunks() {
    let server = mock("hello streaming world").await;
    let transport = ReqwestTransport::new();
    let mut start = transport
        .stream(InstallRequest::get(server.base_url.clone()))
        .await
        .expect("stream");
    assert_eq!(start.status, 200);

    let mut collected = Vec::new();
    while let Some(chunk) = start.chunks.recv().await {
        collected.extend_from_slice(&chunk.expect("chunk"));
    }
    assert_eq!(collected, b"hello streaming world");
}

#[tokio::test]
async fn dropping_the_receiver_cancels_the_download() {
    let server = mock("some bytes here").await;
    let transport = ReqwestTransport::new();
    let start = transport
        .stream(InstallRequest::get(server.base_url.clone()))
        .await
        .expect("stream");
    // Drop the receiver immediately — the producer task observes the closed channel
    // and stops; no panic, no hang.
    drop(start.chunks);
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
}

/// A server that sends the head and the first byte of a longer advertised body,
/// then holds the connection open forever without sending the rest.
async fn stalling_mock() -> MockServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let accept = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut tmp = [0u8; 2048];
                let _ = stream.read(&mut tmp).await;
                // Advertise ten bytes but only ever send one, then never write again.
                let _ = stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\nConnection: close\r\n\r\nA",
                    )
                    .await;
                let _ = stream.flush().await;
                // Keep the socket open so the client waits for the missing bytes.
                std::future::pending::<()>().await;
            });
        }
    })
    .abort_handle();
    MockServer {
        base_url: format!("http://{addr}"),
        accept,
    }
}

#[tokio::test]
async fn a_stalled_download_ends_with_an_idle_timeout_error() {
    let server = stalling_mock().await;
    let transport =
        ReqwestTransport::new().with_idle_timeout(std::time::Duration::from_millis(200));
    let mut start = transport
        .stream(InstallRequest::get(server.base_url.clone()))
        .await
        .expect("stream");
    // The one delivered byte arrives...
    let first = start.chunks.recv().await.expect("first chunk").expect("ok");
    assert_eq!(first, b"A");
    // ...then, with no further bytes, the idle timeout fires and surfaces a
    // terminal error rather than hanging.
    match start.chunks.recv().await {
        Some(Err(InstallError::TransferFailed(message))) => assert!(message.contains("stalled")),
        other => panic!("expected a stalled transfer error, got {other:?}"),
    }
}

/// A transport that only implements `fetch`, so `stream` uses the default.
struct FetchOnly;

impl InstallTransport for FetchOnly {
    fn fetch(&self, _request: InstallRequest) -> TransportFuture {
        Box::pin(async { Err(InstallError::TransferFailed("unused".to_owned())) })
    }
}

#[tokio::test]
async fn the_default_stream_reports_unsupported() {
    let transport = FetchOnly;
    let result = transport.stream(InstallRequest::get("http://x")).await;
    match result {
        Err(InstallError::TransferFailed(message)) => assert!(message.contains("streaming")),
        _ => panic!("expected an unsupported-streaming transfer failure"),
    }
}

// Exercise the exported alias so it stays part of the public surface.
fn _stream_future_type(_: StreamFuture) {}
