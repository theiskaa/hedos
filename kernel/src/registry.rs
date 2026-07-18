//! The store of owned models: a `models.json` file holding every [`ModelRecord`]
//! the kernel knows about, with change-detecting writes and corruption recovery.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::persistence::{self, StoreError};
use crate::records::{ModelRecord, ModelState};

const STORE_FILE: &str = "models.json";
const SCHEMA_VERSION: u32 = 1;

/// Errors raised by the registry.
#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    /// The store file could not be decoded. It has been quarantined; the payload
    /// describes the decode failure.
    #[error("corrupt registry store: {0}")]
    CorruptStore(String),

    /// The store was written by a newer schema version than this build supports.
    /// It is left untouched rather than downgraded, so its data is not lost.
    #[error("registry store schema {found} is newer than supported {supported}")]
    FutureSchema {
        /// The version found on disk.
        found: u32,
        /// The newest version this build understands.
        supported: u32,
    },

    /// A filesystem or encoding error while reading or writing the store.
    #[error(transparent)]
    Store(#[from] StoreError),
}

#[derive(Debug, Deserialize)]
struct Envelope {
    schema_version: u32,
    models: Vec<ModelRecord>,
}

#[derive(Serialize)]
struct EnvelopeRef<'a> {
    schema_version: u32,
    models: Vec<&'a ModelRecord>,
}

/// The in-memory model store, backed by `<directory>/models.json`. Every mutating
/// method persists the change before returning. Not safe for concurrent writers:
/// two instances on the same directory each hold their own view and the last to
/// save wins, so a single process should own one instance.
#[derive(Debug)]
pub struct Registry {
    directory: PathBuf,
    models: BTreeMap<String, ModelRecord>,
}

impl Registry {
    /// Open the registry rooted at `directory`, loading `models.json` if present.
    /// A missing store opens empty; a corrupt store is quarantined and reported
    /// as [`RegistryError::CorruptStore`]; a store from a newer schema is left in
    /// place and reported as [`RegistryError::FutureSchema`]. If the file holds
    /// two records with the same id, the last one in file order wins.
    pub fn open(directory: &Path) -> Result<Self, RegistryError> {
        let file = directory.join(STORE_FILE);
        let models = match persistence::read_json::<Envelope>(&file) {
            Ok(Some(envelope)) => {
                if envelope.schema_version > SCHEMA_VERSION {
                    return Err(RegistryError::FutureSchema {
                        found: envelope.schema_version,
                        supported: SCHEMA_VERSION,
                    });
                }
                envelope
                    .models
                    .into_iter()
                    .map(|record| (record.id.clone(), record))
                    .collect()
            }
            Ok(None) => BTreeMap::new(),
            Err(StoreError::Corrupt { source, .. }) => {
                return Err(RegistryError::CorruptStore(source.to_string()));
            }
            Err(other) => return Err(RegistryError::Store(other)),
        };
        Ok(Self {
            directory: directory.to_path_buf(),
            models,
        })
    }

    /// The record with `id`, if present.
    pub fn get(&self, id: &str) -> Option<&ModelRecord> {
        self.models.get(id)
    }

    /// Whether a record with `id` is present.
    pub fn contains(&self, id: &str) -> bool {
        self.models.contains_key(id)
    }

    /// The number of records held.
    pub fn len(&self) -> usize {
        self.models.len()
    }

    /// Whether the registry holds no records.
    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }

    /// Every record, sorted by display name (case-insensitive) then id.
    pub fn list(&self) -> Vec<&ModelRecord> {
        let mut records: Vec<&ModelRecord> = self.models.values().collect();
        records.sort_by_cached_key(|record| (record.name.to_lowercase(), record.id.clone()));
        records
    }

    /// Insert or replace `record`. Returns whether anything changed; an identical
    /// record is a no-op that touches no disk.
    pub fn register(&mut self, record: ModelRecord) -> Result<bool, RegistryError> {
        if self.models.get(&record.id) == Some(&record) {
            return Ok(false);
        }
        self.models.insert(record.id.clone(), record);
        self.save()?;
        Ok(true)
    }

    /// Insert or replace many records, writing once. Returns how many input
    /// records differed from the store (counted per input record, so two inputs
    /// with the same id both count even though only the last survives).
    pub fn register_all(&mut self, records: Vec<ModelRecord>) -> Result<usize, RegistryError> {
        let mut changed = 0;
        for record in records {
            if self.models.get(&record.id) != Some(&record) {
                self.models.insert(record.id.clone(), record);
                changed += 1;
            }
        }
        if changed > 0 {
            self.save()?;
        }
        Ok(changed)
    }

    /// Remove the record with `id`, returning it if it was present.
    pub fn unregister(&mut self, id: &str) -> Result<Option<ModelRecord>, RegistryError> {
        let removed = self.models.remove(id);
        if removed.is_some() {
            self.save()?;
        }
        Ok(removed)
    }

    /// Set the lifecycle state of `id` if it is present. Returns whether the
    /// record was present; only a real state change touches disk.
    pub fn set_state_if_present(
        &mut self,
        id: &str,
        state: ModelState,
    ) -> Result<bool, RegistryError> {
        let Some(record) = self.models.get_mut(id) else {
            return Ok(false);
        };
        if record.state == state {
            return Ok(true);
        }
        record.state = state;
        self.save()?;
        Ok(true)
    }

    /// Apply `transform` to each present id. When it returns a record that differs
    /// from the current one, the record is stored under its own id — if the
    /// transform changed the id, the old key is removed so the record migrates
    /// cleanly rather than leaving the map keyed by a stale id. Returns the changed
    /// records; writes once if anything changed.
    pub fn update(
        &mut self,
        ids: &[String],
        transform: impl Fn(&ModelRecord) -> Option<ModelRecord>,
    ) -> Result<Vec<ModelRecord>, RegistryError> {
        let mut changed = Vec::new();
        for id in ids {
            let Some(next) = self.models.get(id).and_then(&transform) else {
                continue;
            };
            if self.models.get(id) == Some(&next) {
                continue;
            }
            if next.id != *id {
                self.models.remove(id);
            }
            self.models.insert(next.id.clone(), next.clone());
            changed.push(next);
        }
        if !changed.is_empty() {
            self.save()?;
        }
        Ok(changed)
    }

    fn save(&self) -> Result<(), RegistryError> {
        let envelope = EnvelopeRef {
            schema_version: SCHEMA_VERSION,
            models: self.models.values().collect(),
        };
        persistence::write_json_atomic(&self.directory.join(STORE_FILE), &envelope)?;
        Ok(())
    }
}
