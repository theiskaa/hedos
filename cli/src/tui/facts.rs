//! The machine facts the header and detail pane show, gathered once per
//! refresh: memory, what is loaded and by whom, disk by store, the gateway.

use std::collections::BTreeMap;

use kernel::records::ModelRecord;
use kernel::records::byte_format::BYTES_PER_MIB;
use kernel::time::now_millis;

use crate::support::machine;
use crate::support::session::Session;

/// Who holds a resident model in memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Holder {
    /// This process's own kernel.
    Local,
    /// A `hedos serve` on the configured port.
    Gateway,
}

/// One model held in memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resident {
    /// The record id.
    pub id: String,
    /// The display name.
    pub name: String,
    /// The footprint in bytes.
    pub bytes: i64,
    /// Who holds it.
    pub holder: Holder,
    /// When its idle unload fires, in Unix milliseconds, if a timer is armed.
    pub expires_at_millis: Option<i64>,
}

impl Resident {
    /// Seconds until the idle unload fires, if one is armed and still ahead.
    /// A deadline the clock has passed is a snapshot gone stale, not a countdown.
    pub fn expires_in_seconds(&self) -> Option<i64> {
        self.expires_at_millis
            .map(|deadline| (deadline - now_millis()) / 1000)
            .filter(|seconds| *seconds > 0)
    }
}

/// Everything about the machine the screen shows.
#[derive(Debug, Clone, Default)]
pub struct Facts {
    /// The machine's total memory in bytes.
    pub memory_bytes: u64,
    /// The models in memory, local first, then the gateway's.
    pub residents: Vec<Resident>,
    /// The port a running gateway answered on, if any.
    pub gateway_port: Option<u16>,
    /// Bytes on disk per store kind, largest first.
    pub disk_by_store: Vec<(String, i64)>,
}

impl Facts {
    /// Gather the facts for `records` from `session` and the gateway probe.
    pub async fn collect(session: &Session, records: &[ModelRecord]) -> Self {
        let live = session.live_gateway().await;
        let name_of = |id: &str| {
            records
                .iter()
                .find(|record| record.id == id)
                .map(|record| record.display_name().to_owned())
        };

        let mut residents: Vec<Resident> = session
            .kernel
            .resident_models()
            .into_iter()
            .filter_map(|entry| {
                let id = entry.model_id?;
                Some(Resident {
                    name: name_of(&id).unwrap_or(entry.name),
                    id,
                    bytes: entry.footprint_mb * BYTES_PER_MIB,
                    holder: Holder::Local,
                    expires_at_millis: entry.expires_at_millis,
                })
            })
            .collect();
        if let Some(live) = &live {
            for resident in &live.residents {
                if residents.iter().any(|known| known.id == resident.id) {
                    continue;
                }
                let Some(name) = name_of(&resident.id) else {
                    continue;
                };
                residents.push(Resident {
                    id: resident.id.clone(),
                    name,
                    bytes: resident.size,
                    holder: Holder::Gateway,
                    expires_at_millis: resident.expires_at_millis(),
                });
            }
        }

        Self {
            memory_bytes: machine::memory_budget_bytes(),
            residents,
            gateway_port: live.map(|live| live.port),
            disk_by_store: disk_by_store(records),
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
}

/// Footprints summed per store kind, largest first, ties by name.
fn disk_by_store(records: &[ModelRecord]) -> Vec<(String, i64)> {
    let mut totals: BTreeMap<&str, i64> = BTreeMap::new();
    for record in records {
        *totals.entry(record.source.kind.as_str()).or_default() +=
            record.footprint_mb.unwrap_or(0) * BYTES_PER_MIB;
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
        };
        assert_eq!(facts.resident_bytes(), 15);
        assert_eq!(facts.disk_bytes(), 10);
        assert_eq!(facts.resident("b").map(|r| r.holder), Some(Holder::Gateway));
    }
}
