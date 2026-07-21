//! The download progress display for `hedos pull`: an animated byte spinner that
//! upgrades to a percentage bar the moment a reliable total is known (Hugging
//! Face), and stays a spinner for growing/estimated totals (Ollama, whose total
//! only firms up as layer manifests arrive). Silent under `--json` or off a
//! terminal, where `pull` reports the outcome at the end instead.

use std::io::IsTerminal;
use std::time::Duration;

use indicatif::{ProgressBar, ProgressStyle};
use kernel::install::event::InstallProgress;

use crate::support::output::Out;

/// A live download indicator, or a no-op when progress cannot be shown.
pub struct Download {
    bar: Option<ProgressBar>,
    determinate: bool,
}

impl Download {
    /// Start an indeterminate byte spinner, or a no-op when animation is not
    /// possible (not a terminal, or `--json`).
    pub fn start(out: &Out) -> Self {
        if out.is_json() || !std::io::stderr().is_terminal() {
            return Self {
                bar: None,
                determinate: false,
            };
        }
        let style = ProgressStyle::with_template("{spinner:.cyan} {bytes}  {wide_msg}")
            .unwrap_or_else(|_| ProgressStyle::default_spinner())
            .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ ");
        let bar = ProgressBar::new_spinner().with_style(style);
        bar.enable_steady_tick(Duration::from_millis(90));
        Self {
            bar: Some(bar),
            determinate: false,
        }
    }

    /// Fold in a progress event: upgrade to a percentage bar once a firm total
    /// arrives, then advance the position and show the current file.
    pub fn progress(&mut self, progress: &InstallProgress) {
        let Some(bar) = &self.bar else { return };
        if !self.determinate
            && let Some(total) = progress.total_bytes.filter(|_| !progress.total_is_partial)
        {
            bar.disable_steady_tick();
            bar.set_length(total.max(0) as u64);
            let style = ProgressStyle::with_template(
                "{bar:24.cyan/blue} {percent:>3}%  {bytes}/{total_bytes}  {wide_msg}",
            )
            .unwrap_or_else(|_| ProgressStyle::default_bar())
            .progress_chars("█▓░");
            bar.set_style(style);
            self.determinate = true;
        }
        bar.set_position(progress.bytes_downloaded.max(0) as u64);
        if let Some(file) = &progress.current_file {
            bar.set_message(file.clone());
        }
    }

    /// Show a status line (e.g. "pulling manifest") on the indicator.
    pub fn status(&self, message: &str) {
        if let Some(bar) = &self.bar {
            bar.set_message(message.to_owned());
        }
    }

    /// Stop the indicator and clear its line.
    pub fn finish(&mut self) {
        if let Some(bar) = self.bar.take() {
            bar.finish_and_clear();
        }
    }
}
