//! The audit record for a served request and the sinks that store it. A no-op
//! sink and a rotating disk-backed JSONL log both implement the same trait.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::wire::timestamp::{iso8601, millis_from_iso8601};

/// One line of the audit log: who called, what they asked for, and how it went.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayAuditEntry {
    /// When the request completed. On the wire this is an ISO 8601 UTC string;
    /// in memory it is epoch milliseconds.
    #[serde(rename = "ts", with = "wire_ts")]
    pub ts_millis: i64,
    /// The client's id, if authenticated.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub client: Option<String>,
    /// The client's display name, if authenticated.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub client_name: Option<String>,
    /// The HTTP method.
    pub method: String,
    /// The request path.
    pub route: String,
    /// The model served, if any.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub model: Option<String>,
    /// The capability exercised, if any.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub capability: Option<String>,
    /// The audit outcome label.
    pub outcome: String,
    /// The HTTP status returned.
    pub status: u16,
    /// How long the request took, in milliseconds.
    pub duration_ms: i64,
    /// Extra detail (e.g. an internal error description).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub detail: Option<String>,
}

/// Serialize `ts_millis` as an ISO 8601 string and parse it back.
mod wire_ts {
    use serde::{Deserialize, Deserializer, Serializer};

    use super::{iso8601, millis_from_iso8601};

    pub fn serialize<S: Serializer>(millis: &i64, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&iso8601(*millis))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<i64, D::Error> {
        let text = String::deserialize(deserializer)?;
        millis_from_iso8601(&text)
            .ok_or_else(|| serde::de::Error::custom("invalid ISO 8601 timestamp"))
    }
}

/// A sink for audit entries.
pub trait Auditing: Send + Sync {
    /// Record a served request.
    fn append(&self, entry: GatewayAuditEntry);

    /// Record a rejected unauthenticated request. Defaults to [`append`](Self::append);
    /// a real log may rate-limit these.
    fn append_unauthorized(&self, entry: GatewayAuditEntry) {
        self.append(entry);
    }

    /// Flush any buffered state (e.g. a pending coalesced-unauthorized summary)
    /// so nothing is lost at shutdown. Defaults to a no-op.
    fn flush(&self) {}
}

/// An audit sink that discards everything.
pub struct NoopAudit;

impl Auditing for NoopAudit {
    fn append(&self, _entry: GatewayAuditEntry) {}
}

/// The default size at which the active log rotates: 5 MiB.
const DEFAULT_MAX_BYTES: u64 = 5_242_880;
/// The number of log files kept, including the active one. Must be at least 2 —
/// the rotation math assumes a live file plus at least one rotated generation.
const GENERATIONS: u32 = 3;
/// How long a burst of rejected tokens coalesces into one summary line.
const UNAUTHORIZED_WINDOW_MS: i64 = 60_000;

/// A disk-backed JSONL audit log. Entries append one per line; the file rotates
/// through a fixed number of generations once it exceeds a size bound. A burst of
/// rejected unauthenticated requests is coalesced into a single summary line so a
/// scanning client cannot flood the log.
pub struct GatewayAuditLog {
    path: PathBuf,
    max_bytes: u64,
    unauthorized: Mutex<UnauthorizedWindow>,
}

/// The running state of the unauthenticated-request coalescing window.
#[derive(Default)]
struct UnauthorizedWindow {
    start_millis: Option<i64>,
    suppressed: u64,
}

impl GatewayAuditLog {
    /// A log writing `audit.jsonl` inside `directory`, rotating at 5 MiB.
    pub fn new(directory: impl AsRef<Path>) -> Self {
        Self::with_max_bytes(directory, DEFAULT_MAX_BYTES)
    }

    /// A log writing `audit.jsonl` inside `directory`, rotating at `max_bytes`.
    pub fn with_max_bytes(directory: impl AsRef<Path>, max_bytes: u64) -> Self {
        Self {
            path: directory.as_ref().join("audit.jsonl"),
            max_bytes,
            unauthorized: Mutex::new(UnauthorizedWindow::default()),
        }
    }

    /// The most recent `limit` entries, oldest first. Malformed lines are skipped.
    pub fn tail(&self, limit: usize) -> Vec<GatewayAuditEntry> {
        let Ok(text) = fs::read_to_string(&self.path) else {
            return Vec::new();
        };
        let lines: Vec<&str> = text.lines().filter(|line| !line.is_empty()).collect();
        let start = lines.len().saturating_sub(limit);
        lines[start..]
            .iter()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }

    /// Flush any pending coalesced-unauthorized summary, then persist `entry`.
    fn write_entry(&self, window: &mut UnauthorizedWindow, entry: &GatewayAuditEntry) {
        self.flush_unauthorized(window);
        self.persist(entry);
    }

    /// Emit the coalesced summary for a closed unauthorized window, if any.
    fn flush_unauthorized(&self, window: &mut UnauthorizedWindow) {
        let Some(start) = window.start_millis.take() else {
            window.suppressed = 0;
            return;
        };
        let suppressed = std::mem::take(&mut window.suppressed);
        if suppressed == 0 {
            return;
        }
        self.persist(&GatewayAuditEntry {
            ts_millis: start,
            client: None,
            client_name: None,
            method: "-".to_owned(),
            route: "-".to_owned(),
            model: None,
            capability: None,
            outcome: "unauthorized".to_owned(),
            status: 401,
            duration_ms: 0,
            detail: Some(format!(
                "{suppressed} more unauthenticated requests rejected"
            )),
        });
    }

    /// Serialize `entry` as one JSONL line, rotating first if the file is full.
    fn persist(&self, entry: &GatewayAuditEntry) {
        let Ok(mut line) = serde_json::to_vec(entry) else {
            return;
        };
        line.push(b'\n');
        if let Some(parent) = self.path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        self.rotate_if_needed();
        // `append(true).create(true)` creates the file when missing without ever
        // truncating an existing log.
        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            let _ = file.write_all(&line);
        }
    }

    /// Rotate `audit.jsonl` → `audit.1.jsonl` → … when it exceeds `max_bytes`,
    /// dropping the oldest generation.
    fn rotate_if_needed(&self) {
        let Ok(metadata) = fs::metadata(&self.path) else {
            return;
        };
        if metadata.len() <= self.max_bytes {
            return;
        }
        let _ = fs::remove_file(self.rotated_path(GENERATIONS - 1));
        for generation in (1..GENERATIONS - 1).rev() {
            let source = self.rotated_path(generation);
            if source.exists() {
                let _ = fs::rename(&source, self.rotated_path(generation + 1));
            }
        }
        let _ = fs::rename(&self.path, self.rotated_path(1));
    }

    /// The path of the `generation`th rotated file (`audit.<n>.jsonl`).
    fn rotated_path(&self, generation: u32) -> PathBuf {
        self.path
            .with_file_name(format!("audit.{generation}.jsonl"))
    }
}

impl Auditing for GatewayAuditLog {
    fn append(&self, entry: GatewayAuditEntry) {
        // Best-effort: the lock serializes appends with the coalescing state, and
        // a poisoned lock simply drops this entry rather than propagate a panic.
        let Ok(mut window) = self.unauthorized.lock() else {
            return;
        };
        self.write_entry(&mut window, &entry);
    }

    fn append_unauthorized(&self, entry: GatewayAuditEntry) {
        let Ok(mut window) = self.unauthorized.lock() else {
            return;
        };
        if let Some(start) = window.start_millis
            && entry.ts_millis - start < UNAUTHORIZED_WINDOW_MS
        {
            window.suppressed += 1;
            return;
        }
        self.flush_unauthorized(&mut window);
        window.start_millis = Some(entry.ts_millis);
        window.suppressed = 0;
        self.persist(&entry);
    }

    fn flush(&self) {
        if let Ok(mut window) = self.unauthorized.lock() {
            self.flush_unauthorized(&mut window);
        }
    }
}
