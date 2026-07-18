//! The progress and event types an install emits.

/// Download progress for an install: bytes so far, the total (when known), and
/// which file is in flight.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct InstallProgress {
    /// Bytes downloaded so far.
    pub bytes_downloaded: i64,
    /// The total to download, if known.
    pub total_bytes: Option<i64>,
    /// Whether `total_bytes` is a partial/growing estimate (so no fraction).
    pub total_is_partial: bool,
    /// The file currently downloading, if any.
    pub current_file: Option<String>,
}

impl InstallProgress {
    /// The completed fraction in `[0, 1]`, or `None` when the total is unknown,
    /// zero, or only a partial estimate.
    pub fn fraction(&self) -> Option<f64> {
        let total = self.total_bytes?;
        if total <= 0 || self.total_is_partial {
            return None;
        }
        Some((self.bytes_downloaded as f64 / total as f64).clamp(0.0, 1.0))
    }
}

/// A lifecycle event for an install job.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum InstallEvent {
    /// Waiting to start.
    Queued,
    /// Resolving the plan before transfer.
    Preparing,
    /// A human-readable status line.
    Status(String),
    /// Download progress.
    Progress(InstallProgress),
    /// Completed successfully.
    Done,
    /// Failed, with a message.
    Failed {
        /// Why it failed.
        message: String,
    },
    /// Cancelled by the user.
    Cancelled,
}

impl InstallEvent {
    /// Whether this event ends the job (done/failed/cancelled).
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            InstallEvent::Done | InstallEvent::Failed { .. } | InstallEvent::Cancelled
        )
    }
}

/// What a provider's transfer emits while running (before the terminal event is
/// synthesized by the install service).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum InstallStreamEvent {
    /// A status line.
    Status(String),
    /// Download progress.
    Progress(InstallProgress),
}
