//! The HTTP transport the install providers fetch through. Behind a trait so the
//! Hugging Face / Ollama providers can be driven against a mock in tests.

use std::future::Future;
use std::pin::Pin;

use kernel::install::InstallError;

/// A GET request: a URL and any headers to send.
#[derive(Debug, Clone)]
pub struct InstallRequest {
    /// The full request URL.
    pub url: String,
    /// Header `(name, value)` pairs.
    pub headers: Vec<(String, String)>,
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

/// Fetches install requests over HTTP. The streaming download path lands with the
/// download provider; this covers the metadata/listing requests.
pub trait InstallTransport: Send + Sync {
    /// Fetch `request`, returning the whole response.
    fn fetch(&self, request: InstallRequest) -> TransportFuture;
}

/// The production transport, over `reqwest`.
pub struct ReqwestTransport {
    client: reqwest::Client,
}

impl ReqwestTransport {
    /// A transport with a fresh client.
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
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
}
