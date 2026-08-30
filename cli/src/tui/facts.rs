//! The machine facts the screen shows, gathered once per refresh: memory,
//! what is loaded and by whom, disk by store, and what the gateway served.

use std::collections::{BTreeMap, HashMap};

use gateway::audit::GatewayAuditEntry;
use gateway::stats::{LatencyPercentiles, percentiles};
use kernel::records::{ModelRecord, ModelState};
use kernel::time::now_millis;

use crate::support::machine;
use crate::support::residency::{self, Resident};
use crate::support::session::Session;

const HOUR_MILLIS: i64 = 3_600_000;
const DAY_MILLIS: i64 = 24 * HOUR_MILLIS;
const MINUTE_MILLIS: i64 = 60_000;
/// Hourly buckets in the activity sparkline.
pub const HOURS: usize = 24;

/// One model's slice of the gateway's recent history. Only served requests
/// count: the gateway records no model on the ones it rejects.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ModelActivity {
    /// Requests served in the last 24 hours.
    pub requests: u64,
    /// Latency percentiles over those.
    pub latency: Option<LatencyPercentiles>,
    /// Requests per hour, oldest first, the last bucket being this hour.
    pub hourly: [u32; HOURS],
    /// When the model was last requested, ever, in Unix milliseconds.
    pub last_seen_millis: i64,
}

/// What the gateway has been doing, from its audit log.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Activity {
    /// Per model, keyed by the id the gateway resolved the request to.
    pub models: HashMap<String, ModelActivity>,
    /// Requests in the last minute, all models.
    pub requests_last_minute: u64,
    /// Every request the log holds.
    pub total_requests: u64,
    /// When the newest request of any kind came in, in Unix milliseconds.
    pub last_request_millis: i64,
}

impl Activity {
    /// Fold `entries` at `now` into per-model activity, in one pass.
    pub fn from_entries(entries: &[GatewayAuditEntry], now: i64) -> Self {
        let day_ago = now - DAY_MILLIS;
        let mut models: HashMap<String, ModelActivity> = HashMap::new();
        let mut durations: HashMap<String, Vec<i64>> = HashMap::new();
        for entry in entries {
            let Some(model) = &entry.model else {
                continue;
            };
            let activity = models.entry(model.clone()).or_default();
            activity.last_seen_millis = activity.last_seen_millis.max(entry.ts_millis);
            if entry.ts_millis >= day_ago && entry.is_ok() {
                activity.requests += 1;
                let age = (now - entry.ts_millis).max(0) / HOUR_MILLIS;
                activity.hourly[(HOURS - 1).saturating_sub(age as usize)] += 1;
                durations
                    .entry(model.clone())
                    .or_default()
                    .push(entry.duration_ms);
            }
        }
        for (model, samples) in durations {
            if let Some(activity) = models.get_mut(&model) {
                activity.latency = percentiles(samples);
            }
        }
        Self {
            models,
            requests_last_minute: entries
                .iter()
                .filter(|entry| entry.ts_millis >= now - MINUTE_MILLIS)
                .count() as u64,
            total_requests: entries.len() as u64,
            last_request_millis: entries
                .iter()
                .map(|entry| entry.ts_millis)
                .max()
                .unwrap_or(0),
        }
    }

    /// The activity for `record`. Handlers log the resolved id, so the id is
    /// the key; the names are tried for logs older than that.
    pub fn for_record(&self, record: &ModelRecord) -> Option<&ModelActivity> {
        [record.id.as_str(), record.name.as_str()]
            .into_iter()
            .chain(record.alias.as_deref())
            .find_map(|key| self.models.get(key))
    }
}

/// Everything about the machine the screen shows.
#[derive(Debug, Clone, Default)]
pub struct Facts {
    /// The machine's total memory in bytes.
    pub memory_bytes: u64,
    /// The models in memory: local, then the gateway's, then the Ollama daemon's.
    pub residents: Vec<Resident>,
    /// The port a running gateway answered on, if any.
    pub gateway_port: Option<u16>,
    /// Bytes on disk per store kind, largest first.
    pub disk_by_store: Vec<(String, i64)>,
    /// What the gateway has served, from its audit log.
    pub activity: Activity,
    /// When these facts were read, in Unix milliseconds; ages are measured
    /// from here so the screen never needs a clock.
    pub collected_at_millis: i64,
}

impl Facts {
    /// Gather the facts for `records` from `session`, the gateway probe, and
    /// the audit `entries`.
    pub async fn collect(
        session: &Session,
        records: &[ModelRecord],
        entries: &[GatewayAuditEntry],
    ) -> Self {
        let now = now_millis();
        let loaded = residency::loaded(session, records).await;
        Self {
            memory_bytes: machine::memory_budget_bytes(),
            residents: loaded.residents,
            gateway_port: loaded.gateway_port,
            disk_by_store: disk_by_store(records),
            activity: Activity::from_entries(entries, now),
            collected_at_millis: now,
        }
    }

    /// Bytes held in memory, all holders together.
    pub fn resident_bytes(&self) -> i64 {
        self.residents.iter().map(|resident| resident.bytes).sum()
    }

    /// The resident entry for `id`, if it is loaded.
    pub fn resident(&self, id: &str) -> Option<&Resident> {
        self.residents.iter().find(|resident| resident.id == id)
    }

    /// Whether `id` is loaded, by any holder.
    pub fn is_warm(&self, id: &str) -> bool {
        self.resident(id).is_some()
    }

    /// Bytes not held by any resident.
    pub fn free_bytes(&self) -> i64 {
        self.memory_bytes as i64 - self.resident_bytes()
    }

    /// Bytes on disk across every store.
    pub fn disk_bytes(&self) -> i64 {
        self.disk_by_store.iter().map(|(_, bytes)| bytes).sum()
    }

    /// The gateway in a phrase: `on :11434 · 3 req/min`, or `off`.
    pub fn gateway_state(&self) -> String {
        match self.gateway_port {
            Some(port) => format!(
                "on :{port} · {} req/min",
                self.activity.requests_last_minute
            ),
            None => "off".to_owned(),
        }
    }
}

/// Footprints summed per store kind, largest first, ties by name. A record
/// whose weights are gone holds no disk.
fn disk_by_store(records: &[ModelRecord]) -> Vec<(String, i64)> {
    let mut totals: BTreeMap<&str, i64> = BTreeMap::new();
    for record in records {
        if record.state == ModelState::Missing {
            continue;
        }
        *totals.entry(record.source.kind.as_str()).or_default() +=
            record.footprint_bytes().unwrap_or(0);
    }
    let mut stores: Vec<(String, i64)> = totals
        .into_iter()
        .map(|(kind, bytes)| (kind.to_owned(), bytes))
        .collect();
    stores.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    stores
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::support::residency::Holder;
    use kernel::records::byte_format::BYTES_PER_MIB;
    use kernel::records::{Capability, Modality, ModelSource, SourceKind};

    fn record(name: &str, kind: SourceKind, footprint_mb: Option<i64>) -> ModelRecord {
        let mut record = ModelRecord::new(
            name,
            Modality::text(),
            vec![Capability::chat()],
            ModelSource::new(kind, name),
        );
        record.footprint_mb = footprint_mb;
        record
    }

    #[test]
    fn disk_is_summed_per_store_largest_first() {
        let records = [
            record("a", SourceKind::ollama(), Some(1)),
            record("b", SourceKind::huggingface_cache(), Some(5)),
            record("c", SourceKind::ollama(), Some(2)),
            record("d", SourceKind::file(), None),
        ];
        let mut gone = record("e", SourceKind::ollama(), Some(100));
        gone.state = ModelState::Missing;
        let records = [
            records[0].clone(),
            records[1].clone(),
            records[2].clone(),
            records[3].clone(),
            gone,
        ];
        let stores = disk_by_store(&records);
        assert_eq!(stores[0].0, "huggingface-cache");
        assert_eq!(stores[0].1, 5 * BYTES_PER_MIB);
        assert_eq!(stores[1], ("ollama".to_owned(), 3 * BYTES_PER_MIB));
        assert_eq!(stores[2], ("file".to_owned(), 0));
    }

    #[test]
    fn totals_add_up() {
        let facts = Facts {
            memory_bytes: 0,
            residents: vec![
                Resident {
                    id: "a".into(),
                    name: "a".into(),
                    bytes: 10,
                    holder: Holder::Local,
                    expires_at_millis: None,
                },
                Resident {
                    id: "b".into(),
                    name: "b".into(),
                    bytes: 5,
                    holder: Holder::Gateway,
                    expires_at_millis: None,
                },
            ],
            gateway_port: None,
            disk_by_store: vec![("ollama".into(), 7), ("file".into(), 3)],
            activity: Activity::default(),
            collected_at_millis: 0,
        };
        assert_eq!(facts.resident_bytes(), 15);
        assert_eq!(facts.disk_bytes(), 10);
        assert_eq!(facts.resident("b").map(|r| r.holder), Some(Holder::Gateway));
    }

    fn entry(model: &str, outcome: &str, ts_millis: i64) -> GatewayAuditEntry {
        GatewayAuditEntry {
            ts_millis,
            client: None,
            client_name: None,
            method: "POST".to_owned(),
            route: "/api/chat".to_owned(),
            model: Some(model.to_owned()),
            capability: Some("chat".to_owned()),
            outcome: outcome.to_owned(),
            status: if outcome == "ok" { 200 } else { 500 },
            duration_ms: 10,
            detail: None,
        }
    }

    #[test]
    fn a_day_of_requests_lands_in_hourly_buckets() {
        let now = 100 * DAY_MILLIS;
        let entries = [
            entry("m", "ok", now - 10),
            entry("m", "ok", now - 2 * HOUR_MILLIS - 1),
            entry("m", "error", now - 3 * HOUR_MILLIS),
            entry("m", "ok", now - 2 * DAY_MILLIS),
        ];
        let activity = Activity::from_entries(&entries, now);
        let model = &activity.models["m"];
        assert_eq!(model.requests, 2);
        assert_eq!(model.hourly[HOURS - 1], 1);
        assert_eq!(model.hourly[HOURS - 3], 1);
        assert_eq!(model.hourly.iter().sum::<u32>(), 2);
        assert_eq!(model.last_seen_millis, now - 10);
        assert_eq!(activity.requests_last_minute, 1);
    }

    #[test]
    fn an_old_model_keeps_its_last_seen_but_no_recent_counts() {
        let now = 100 * DAY_MILLIS;
        let activity = Activity::from_entries(&[entry("m", "ok", now - 3 * DAY_MILLIS)], now);
        let model = &activity.models["m"];
        assert_eq!(model.requests, 0);
        assert_eq!(model.last_seen_millis, now - 3 * DAY_MILLIS);
    }
}
