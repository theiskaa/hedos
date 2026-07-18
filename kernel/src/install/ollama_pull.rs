//! Folding an Ollama `/api/pull` ndjson stream into install progress. Each line is
//! a status object; digest lines carry per-layer byte totals that aggregate into
//! overall progress, and a `success` line ends the pull.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::install::bytes::saturating_sum;
use crate::install::error::InstallError;
use crate::install::event::InstallProgress;

/// What folding one pull line produced.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Outcome {
    /// The line carried nothing new (unparseable, blank, or a repeated status).
    Ignored,
    /// A new status line.
    Status(String),
    /// Updated aggregate download progress.
    Progress(InstallProgress),
    /// The pull completed.
    Success,
}

/// Accumulates per-layer byte counts across pull lines into overall progress.
#[derive(Debug, Default)]
pub struct Aggregator {
    totals: BTreeMap<String, i64>,
    completed: BTreeMap<String, i64>,
    last_status: Option<String>,
}

impl Aggregator {
    /// A fresh aggregator.
    pub fn new() -> Self {
        Self::default()
    }

    /// Fold one ndjson line, updating internal totals. Returns what to emit, or a
    /// [`InstallError::TransferFailed`] if the line carried an error.
    pub fn fold(&mut self, line: &str) -> Result<Outcome, InstallError> {
        let Ok(decoded) = serde_json::from_str::<Line>(line) else {
            return Ok(Outcome::Ignored);
        };
        if let Some(error) = decoded.error.as_deref().filter(|error| !error.is_empty()) {
            return Err(InstallError::TransferFailed(format!("ollama: {error}")));
        }
        let Some(status) = decoded
            .status
            .as_deref()
            .filter(|status| !status.is_empty())
        else {
            return Ok(Outcome::Ignored);
        };
        if status == "success" {
            return Ok(Outcome::Success);
        }
        if let (Some(digest), Some(total)) = (decoded.digest.as_ref(), decoded.total) {
            self.totals.insert(digest.clone(), total);
            // Keep the last known completed count for this layer when a line omits it.
            let completed = decoded
                .completed
                .or_else(|| self.completed.get(digest).copied())
                .unwrap_or(0);
            self.completed.insert(digest.clone(), completed);
            return Ok(Outcome::Progress(self.aggregate()));
        }
        if self.last_status.as_deref() == Some(status) {
            return Ok(Outcome::Ignored);
        }
        self.last_status = Some(status.to_owned());
        Ok(Outcome::Status(status.to_owned()))
    }

    fn aggregate(&self) -> InstallProgress {
        InstallProgress {
            bytes_downloaded: saturating_sum(self.completed.values().copied()),
            total_bytes: Some(saturating_sum(self.totals.values().copied())),
            total_is_partial: true,
            current_file: None,
        }
    }
}

#[derive(Deserialize)]
struct Line {
    status: Option<String>,
    digest: Option<String>,
    total: Option<i64>,
    completed: Option<i64>,
    error: Option<String>,
}
