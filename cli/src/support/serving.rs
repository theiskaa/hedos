//! Assembling the loopback gateway, shared by `serve` and `launch`, and probing
//! whether one is already listening.
//!
//! The two differ only in what ends them: `serve` runs until Ctrl-C on a fixed
//! port, `launch` runs on an ephemeral port until the agent it spawned exits.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use gateway::audit::GatewayAuditLog;
use gateway::auth::OpenAuth;
use gateway::kernel_gateway::KernelGateway;
use gateway::router::{GatewayRouter, standard_routes};
use kernel::time::millis_from_iso8601;
use runtime::facade::Kernel;
use serde::Deserialize;
use tokio::net::TcpListener;

use crate::error::CliError;
use crate::support::http::probe_json;

/// Build the router that serves `kernel`, auditing into `audit_dir` and
/// admitting at most `max_inference` concurrent inference requests.
pub fn router(kernel: Arc<Kernel>, audit_dir: &Path, max_inference: usize) -> Arc<GatewayRouter> {
    Arc::new(GatewayRouter::new(
        Arc::new(KernelGateway::new(kernel)),
        Box::new(OpenAuth),
        Box::new(GatewayAuditLog::new(audit_dir)),
        standard_routes(),
        max_inference,
    ))
}

/// Bind loopback on `port`. Port `0` asks the OS for a free one; the caller
/// reads the real port back from the returned listener.
pub async fn bind(port: u16) -> Result<TcpListener, CliError> {
    TcpListener::bind(("127.0.0.1", port))
        .await
        .map_err(|error| {
            CliError::new(format!(
                "could not bind 127.0.0.1:{port} — {error}. Is another gateway already running?"
            ))
        })
}

/// How long a liveness probe waits for the gateway before deciding it is down.
/// Loopback answers in milliseconds; anything slower is a stuck process.
const PROBE_TIMEOUT: Duration = Duration::from_millis(300);

/// A gateway found listening on loopback, and what it holds in memory.
#[derive(Debug, Clone)]
pub(crate) struct GatewayLive {
    /// The port it answered on.
    pub port: u16,
    /// The models it reports resident.
    pub residents: Vec<LiveResident>,
}

/// One resident model as a running gateway's `/api/ps` reports it.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct LiveResident {
    /// The hedos record id.
    pub id: String,
    /// The footprint in bytes.
    pub size: i64,
    /// When the gateway's idle unload fires, if one is armed (ISO 8601).
    expires_at: Option<String>,
}

impl LiveResident {
    /// When the gateway's idle unload fires, in Unix milliseconds.
    pub fn expires_at_millis(&self) -> Option<i64> {
        self.expires_at.as_deref().and_then(millis_from_iso8601)
    }
}

#[derive(Deserialize)]
struct PsBody {
    models: Vec<LiveResident>,
}

/// The gateway on loopback `port`, if one answers `/api/ps` within
/// [`PROBE_TIMEOUT`] and the answer is a hedos gateway's (a stock Ollama
/// daemon's entries carry no `id`).
pub(crate) async fn probe(port: u16) -> Option<GatewayLive> {
    let body: PsBody =
        probe_json(&format!("http://127.0.0.1:{port}/api/ps"), PROBE_TIMEOUT).await?;
    Some(GatewayLive {
        port,
        residents: body.models,
    })
}
