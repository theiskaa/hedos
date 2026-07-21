//! The HTTP transport the install providers fetch through. Behind a trait so the
//! Hugging Face / Ollama providers can be driven against a mock in tests.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use kernel::install::InstallError;
use tokio::sync::mpsc;

/// Chunks buffered ahead of a slow consumer. Small, so a fast network can't
/// balloon memory to the whole file size while the consumer (disk write / hash)
/// lags — the producer blocks on a full channel instead (backpressure).
const STREAM_CHANNEL_CAPACITY: usize = 16;

/// If no body bytes arrive within this window the download is treated as stalled,
/// so a server that completes the head then goes silent can't wedge the task
/// forever. Generous enough not to trip legitimate slow links.
const DEFAULT_STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(300);

/// Cap on how long connecting may take before a fetch fails, so a dead host
/// can't hang `send()` indefinitely.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// A GET request: a URL and any headers to send.
#[derive(Clone)]
pub struct InstallRequest {
    /// The full request URL.
    pub url: String,
    /// Header `(name, value)` pairs.
    pub headers: Vec<(String, String)>,
}

impl std::fmt::Debug for InstallRequest {
    /// Header values are redacted so an `authorization: Bearer <token>` never
    /// reaches a log or an error string that happens to format the request.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let headers: Vec<(&str, &str)> = self
            .headers
            .iter()
            .map(|(name, _)| (name.as_str(), "<redacted>"))
            .collect();
        f.debug_struct("InstallRequest")
            .field("url", &self.url)
            .field("headers", &headers)
            .finish()
    }
}

impl InstallRequest {
    /// A GET request for `url` with no headers.
    pub fn get(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            headers: Vec::new(),
        }
    }

    /// This request with `(name, value)` added as a header.
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }
}

/// A response: the HTTP status and the body bytes.
#[derive(Debug, Clone)]
pub struct InstallResponse {
    /// The HTTP status code.
    pub status: u16,
    /// The response body.
    pub body: Vec<u8>,
}

/// A future returning a fetched response (or a transfer error).
pub type TransportFuture =
    Pin<Box<dyn Future<Output = Result<InstallResponse, InstallError>> + Send>>;

/// A started streaming response: the HTTP status and a bounded receiver of body
/// chunks (`Ok(bytes)` until a terminal `Err` or the producer finishes). Dropping
/// the receiver signals the producer to stop; a slow consumer applies backpressure.
pub struct StreamStart {
    /// The HTTP status code.
    pub status: u16,
    /// The body, delivered in chunks.
    pub chunks: mpsc::Receiver<Result<Vec<u8>, InstallError>>,
}

/// A future returning a started stream (or a transfer error before the body).
pub type StreamFuture = Pin<Box<dyn Future<Output = Result<StreamStart, InstallError>> + Send>>;

/// Fetches install requests over HTTP: whole responses (metadata/listing) and
/// streamed responses (file downloads / pull progress).
pub trait InstallTransport: Send + Sync {
    /// Fetch `request`, returning the whole response.
    fn fetch(&self, request: InstallRequest) -> TransportFuture;

    /// Start a streaming fetch of `request`. The default doesn't stream;
    /// [`ReqwestTransport`] and test doubles override it.
    fn stream(&self, _request: InstallRequest) -> StreamFuture {
        Box::pin(async {
            Err(InstallError::TransferFailed(
                "this transport does not support streaming".to_owned(),
            ))
        })
    }
}

/// The production transport, over `reqwest`.
pub struct ReqwestTransport {
    client: reqwest::Client,
    idle_timeout: Duration,
}

impl ReqwestTransport {
    /// A transport with a fresh client and the default streaming idle timeout.
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            client,
            idle_timeout: DEFAULT_STREAM_IDLE_TIMEOUT,
        }
    }

    /// This transport with a different streaming idle timeout — the maximum a
    /// download may go without delivering bytes before it's treated as stalled.
    /// Long-running server-side work (e.g. an Ollama pull assembling layers) wants
    /// a larger window than a plain file download.
    pub fn with_idle_timeout(mut self, idle_timeout: Duration) -> Self {
        self.idle_timeout = idle_timeout;
        self
    }
}

impl Default for ReqwestTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl InstallTransport for ReqwestTransport {
    fn fetch(&self, request: InstallRequest) -> TransportFuture {
        let client = self.client.clone();
        Box::pin(async move {
            let mut builder = client.get(&request.url);
            for (name, value) in &request.headers {
                builder = builder.header(name, value);
            }
            let response = builder.send().await.map_err(|error| {
                InstallError::TransferFailed(format!("fetching {}: {error}", request.url))
            })?;
            let status = response.status().as_u16();
            let body = response
                .bytes()
                .await
                .map_err(|error| {
                    InstallError::TransferFailed(format!("reading response: {error}"))
                })?
                .to_vec();
            Ok(InstallResponse { status, body })
        })
    }

    fn stream(&self, request: InstallRequest) -> StreamFuture {
        let client = self.client.clone();
        let idle_timeout = self.idle_timeout;
        Box::pin(async move {
            let mut builder = client.get(&request.url);
            for (name, value) in &request.headers {
                builder = builder.header(name, value);
            }
            let mut response = builder.send().await.map_err(|error| {
                InstallError::TransferFailed(format!("fetching {}: {error}", request.url))
            })?;
            let status = response.status().as_u16();
            let (tx, chunks) = mpsc::channel(STREAM_CHANNEL_CAPACITY);
            // Pump the body into the bounded channel. Termination is guaranteed on
            // every path: a dropped receiver wins the `tx.closed()` arm (or fails
            // the `send`), and a server that goes silent trips the idle timeout —
            // so the task can't leak on a stalled download.
            tokio::spawn(async move {
                loop {
                    let chunk = tokio::select! {
                        biased;
                        _ = tx.closed() => return,
                        chunk = tokio::time::timeout(idle_timeout, response.chunk()) => chunk,
                    };
                    let chunk = match chunk {
                        Ok(chunk) => chunk,
                        Err(_elapsed) => {
                            let _ = tx
                                .send(Err(InstallError::TransferFailed(
                                    "download stalled: no data within the idle timeout".to_owned(),
                                )))
                                .await;
                            return;
                        }
                    };
                    match chunk {
                        Ok(Some(bytes)) => {
                            // A full channel parks here (backpressure); an `Err`
                            // means the receiver dropped, so stop.
                            if tx.send(Ok(bytes.to_vec())).await.is_err() {
                                return;
                            }
                        }
                        Ok(None) => return,
                        Err(error) => {
                            let _ = tx
                                .send(Err(InstallError::TransferFailed(format!(
                                    "reading stream: {error}"
                                ))))
                                .await;
                            return;
                        }
                    }
                }
            });
            Ok(StreamStart { status, chunks })
        })
    }
}
