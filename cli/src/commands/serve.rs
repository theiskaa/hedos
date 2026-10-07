//! `hedos serve` — run the OpenAI/Ollama-compatible gateway on loopback until
//! interrupted.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use clap::Args;
use gateway::server;
use tokio::sync::oneshot;

use crate::error::CliError;
use crate::support::output::Out;
use crate::support::serving;
use crate::support::session::Session;
use crate::support::signals::{self, Interrupts};

/// How long requests still in flight when a termination stops the gateway
/// may run on. A cold start can take its whole readiness timeout and a reply
/// can stream for minutes, and a supervisor that sent SIGTERM kills outright
/// before long, which would leave every model server running; past this they
/// are cut, which stops the work behind them.
const DRAIN_GRACE: Duration = Duration::from_secs(5);

/// How long a cut waits for the requests it cut to finish their short answers
/// before the gateway stops waiting on them at all.
const CUT_GRACE: Duration = Duration::from_secs(2);

/// Arguments for `serve`.
#[derive(Args)]
pub struct ServeArgs {
    /// The port to bind (default from settings, else 43367).
    #[arg(short, long)]
    port: Option<u16>,
}

/// Run the `serve` command; blocks until Ctrl-C, SIGTERM, or SIGHUP.
pub async fn run(args: ServeArgs, out: &Out) -> Result<(), CliError> {
    let session = Session::open()?;
    serve(&session, args.port, out).await
}

/// What told the gateway to stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stopped {
    /// Ctrl-C.
    Interrupted,
    /// SIGTERM, or SIGHUP from a closed terminal.
    Terminated,
}

/// Serve the gateway on `port` (the configured one when `None`) until Ctrl-C
/// or a termination. Shared by the command and `hedos shelf`.
///
/// Ctrl-C stops taking requests and waits for the ones in flight; a second
/// Ctrl-C cuts them. A termination waits for them only [`DRAIN_GRACE`].
pub(crate) async fn serve(session: &Session, port: Option<u16>, out: &Out) -> Result<(), CliError> {
    let port = port.unwrap_or(session.settings.gateway.port);
    let max_inference = session.settings.gateway.max_concurrent_inference.max(1) as usize;
    let audit_dir = session.dirs.sub("gateway");

    let router = serving::router(Arc::clone(&session.kernel), &audit_dir, max_inference);
    let listener = serving::bind(port).await?;
    let address = listener.local_addr()?;
    let base_url = format!("http://{address}/v1");
    let mut interrupts = Interrupts::new();
    out.line(&format!("gateway listening on {base_url}"));
    out.err("auth: open (loopback) — any local client is allowed. Ctrl-C to stop.");
    out.json(&serde_json::json!({
        "running": true,
        "port": address.port(),
        "baseUrl": base_url,
    }));

    let (stop, stop_requested) = oneshot::channel();
    let (cut, cut_requested) = oneshot::channel();
    let serving = server::serve_with_cut(
        listener,
        Arc::clone(&router),
        fired(stop_requested),
        fired(cut_requested),
    );
    tokio::pin!(serving);
    let stopped = tokio::select! {
        result = &mut serving => {
            result?;
            out.err("gateway stopped");
            return Ok(());
        }
        () = interrupts.next() => Stopped::Interrupted,
        _ = signals::terminated() => Stopped::Terminated,
    };
    let _ = stop.send(());
    let terminated = async {
        signals::terminated().await;
    };
    let ended = tokio::select! {
        result = &mut serving => Some(result),
        () = cut_due(stopped, interrupts.next(), terminated, DRAIN_GRACE) => {
            out.err("ending the requests still in flight");
            let _ = cut.send(());
            tokio::time::timeout(CUT_GRACE, &mut serving).await.ok()
        }
    };
    match ended {
        Some(result) => result?,
        None => router.flush_audit(),
    }
    out.err("gateway stopped");
    Ok(())
}

/// Resolve once `signal` is sent; never, when its sender is dropped unsent.
async fn fired(signal: oneshot::Receiver<()>) {
    if signal.await.is_err() {
        std::future::pending::<()>().await;
    }
}

/// When the requests still in flight at a stop are cut: after Ctrl-C, on a
/// second Ctrl-C, or `grace` after a termination that follows it; after a
/// termination, `grace` after it.
async fn cut_due(
    stopped: Stopped,
    interrupted: impl Future<Output = ()>,
    terminated: impl Future<Output = ()>,
    grace: Duration,
) {
    match stopped {
        Stopped::Terminated => tokio::time::sleep(grace).await,
        Stopped::Interrupted => tokio::select! {
            () = interrupted => {}
            () = terminated => tokio::time::sleep(grace).await,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use tokio::time::{Instant, sleep, timeout};

    const LONG: Duration = Duration::from_secs(3600);

    #[tokio::test(start_paused = true)]
    async fn a_termination_cuts_after_the_grace() {
        let started = Instant::now();
        let never = std::future::pending::<()>;
        cut_due(Stopped::Terminated, never(), never(), DRAIN_GRACE).await;
        assert_eq!(started.elapsed(), DRAIN_GRACE);
    }

    #[tokio::test(start_paused = true)]
    async fn ctrl_c_waits_for_the_requests_in_flight() {
        let never = std::future::pending::<()>;
        let waited = timeout(
            LONG,
            cut_due(Stopped::Interrupted, never(), never(), DRAIN_GRACE),
        )
        .await;
        assert!(waited.is_err(), "a first Ctrl-C cut the requests in flight");
    }

    #[tokio::test(start_paused = true)]
    async fn a_second_ctrl_c_cuts_at_once() {
        let started = Instant::now();
        let second = sleep(Duration::from_secs(3));
        cut_due(
            Stopped::Interrupted,
            second,
            std::future::pending(),
            DRAIN_GRACE,
        )
        .await;
        assert_eq!(started.elapsed(), Duration::from_secs(3));
    }

    #[tokio::test(start_paused = true)]
    async fn a_termination_after_ctrl_c_cuts_after_the_grace() {
        let started = Instant::now();
        let terminated = sleep(Duration::from_secs(2));
        cut_due(
            Stopped::Interrupted,
            std::future::pending(),
            terminated,
            DRAIN_GRACE,
        )
        .await;
        assert_eq!(started.elapsed(), Duration::from_secs(2) + DRAIN_GRACE);
    }

    #[tokio::test]
    async fn a_dropped_signal_never_fires() {
        let (sender, signal) = oneshot::channel::<()>();
        drop(sender);
        assert!(
            timeout(Duration::from_millis(50), fired(signal))
                .await
                .is_err()
        );
    }
}
