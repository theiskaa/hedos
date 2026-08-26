//! Asking the local Ollama daemon what it holds, and telling it to let go.
//! Models it serves are loaded in its process, invisible to this one's
//! governor, so residency for them is whatever the daemon says. The daemon is
//! addressed where the runtime adapter addresses it; the two move together.

use std::time::Duration;

use kernel::records::{ModelRecord, SourceKind};
use kernel::time::millis_from_iso8601;
use runtime::adapters::OLLAMA_BASE_URL;
use serde::Deserialize;
use serde_json::json;

use crate::support::http::probe_json;

/// How long a probe waits for the daemon before deciding it is not running.
const PROBE_TIMEOUT: Duration = Duration::from_millis(300);
/// How long an unload request may take; the daemon answers before it evicts.
const UNLOAD_TIMEOUT: Duration = Duration::from_secs(10);

/// One model the daemon reports loaded.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct DaemonResident {
    /// The tag, as the daemon names it.
    pub name: String,
    /// Bytes in memory.
    pub size: i64,
    /// When the daemon lets it go, in its own RFC 3339 form, if it says.
    expires_at: Option<String>,
}

impl DaemonResident {
    /// When the daemon lets the model go, in Unix milliseconds.
    pub fn expires_at_millis(&self) -> Option<i64> {
        self.expires_at.as_deref().and_then(millis_from_iso8601)
    }
}

#[derive(Deserialize)]
struct PsBody {
    models: Vec<DaemonResident>,
}

/// The models the daemon holds, or `None` when nothing answers `/api/ps`.
pub(crate) async fn residents() -> Option<Vec<DaemonResident>> {
    let body: PsBody = probe_json(&format!("{OLLAMA_BASE_URL}/api/ps"), PROBE_TIMEOUT).await?;
    Some(body.models)
}

/// The daemon's entry for `record`, if the daemon holds it.
pub(crate) fn held<'a>(
    residents: &'a [DaemonResident],
    record: &ModelRecord,
) -> Option<&'a DaemonResident> {
    let tag = tag_of(record)?;
    residents.iter().find(|resident| resident.name == tag)
}

/// Whether the daemon holds `record` right now: one probe, one lookup.
pub(crate) async fn holds_now(record: &ModelRecord) -> bool {
    residents()
        .await
        .is_some_and(|residents| held(&residents, record).is_some())
}

/// The daemon's tag for `record`, when the daemon serves it.
pub(crate) fn tag_of(record: &ModelRecord) -> Option<&str> {
    (record.source.kind == SourceKind::ollama()).then_some(record.name.as_str())
}

/// Ask the daemon to unload `tag` now (a generate with `keep_alive: 0`). Only
/// call it for a tag the daemon holds: on a cold one it would load first.
pub(crate) async fn unload(tag: &str) -> Result<(), String> {
    reqwest::Client::builder()
        .timeout(UNLOAD_TIMEOUT)
        .build()
        .map_err(|error| error.to_string())?
        .post(format!("{OLLAMA_BASE_URL}/api/generate"))
        .json(&json!({ "model": tag, "keep_alive": 0 }))
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map(|_| ())
        .map_err(|error| error.to_string())
}
