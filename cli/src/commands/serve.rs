//! `hedos serve` — run the OpenAI/Ollama-compatible gateway on loopback until
//! interrupted.

use std::sync::Arc;

use clap::Args;
use gateway::audit::GatewayAuditLog;
use gateway::auth::OpenAuth;
use gateway::kernel_gateway::KernelGateway;
use gateway::router::{GatewayRouter, standard_routes};
use gateway::server;
use tokio::net::TcpListener;

use crate::error::CliError;
use crate::support::output::Out;
use crate::support::session::Session;
use crate::support::signals;

/// Arguments for `serve`.
#[derive(Args)]
pub struct ServeArgs {
    /// The port to bind (default from settings, else 43367).
    #[arg(short, long)]
    port: Option<u16>,
}

/// Run the `serve` command; blocks until Ctrl-C.
pub async fn run(args: ServeArgs, out: &Out) -> Result<(), CliError> {
    let session = Session::open()?;
    let port = args.port.unwrap_or(session.settings.gateway.port);
    let max_inference = session.settings.gateway.max_concurrent_inference.max(1) as usize;
    let audit_dir = session.dirs.sub("gateway");

    let gateway = Arc::new(KernelGateway::new(Arc::new(session.kernel)));
    let router = Arc::new(GatewayRouter::new(
        gateway,
        Box::new(OpenAuth),
        Box::new(GatewayAuditLog::new(&audit_dir)),
        standard_routes(),
        max_inference,
    ));

    let listener = TcpListener::bind(("127.0.0.1", port)).await?;
    let address = listener.local_addr()?;
    let base_url = format!("http://{address}/v1");
    out.line(&format!("gateway listening on {base_url}"));
    out.err("auth: open (loopback) — any local client is allowed. Ctrl-C to stop.");
    out.json(&serde_json::json!({
        "running": true,
        "port": address.port(),
        "baseUrl": base_url,
    }));

    server::serve_with_shutdown(listener, router, signals::wait_for_ctrl_c()).await?;
    out.err("gateway stopped");
    Ok(())
}
