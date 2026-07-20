//! Assembling the loopback gateway, shared by `serve` and `launch`.
//!
//! The two differ only in what ends them: `serve` runs until Ctrl-C on a fixed
//! port, `launch` runs on an ephemeral port until the agent it spawned exits.

use std::path::Path;
use std::sync::Arc;

use gateway::audit::GatewayAuditLog;
use gateway::auth::OpenAuth;
use gateway::kernel_gateway::KernelGateway;
use gateway::router::{GatewayRouter, standard_routes};
use runtime::facade::Kernel;
use tokio::net::TcpListener;

use crate::error::CliError;

/// Build the router that serves `kernel`, auditing into `audit_dir` and
/// admitting at most `max_inference` concurrent inference requests.
pub fn router(kernel: Kernel, audit_dir: &Path, max_inference: usize) -> Arc<GatewayRouter> {
    Arc::new(GatewayRouter::new(
        Arc::new(KernelGateway::new(Arc::new(kernel))),
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
