//! Interrupt handling: await Ctrl-C so long-running commands (`serve`, `pull`)
//! can stop cleanly.

/// Resolve when the user presses Ctrl-C (SIGINT). If the handler can't be
/// installed, never resolve — a server must not treat a failed install as an
/// immediate shutdown, and a cancellable command should keep running.
pub async fn wait_for_ctrl_c() {
    if tokio::signal::ctrl_c().await.is_err() {
        std::future::pending::<()>().await;
    }
}
