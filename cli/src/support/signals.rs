//! Interrupt handling: await Ctrl-C so long-running commands (`serve`, `pull`)
//! can stop cleanly.

/// Resolve when the user presses Ctrl-C (SIGINT). Resolves immediately if the
/// handler can't be installed, so a command never hangs waiting on it.
pub async fn wait_for_ctrl_c() {
    let _ = tokio::signal::ctrl_c().await;
}
