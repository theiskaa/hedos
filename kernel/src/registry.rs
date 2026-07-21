//! The store of owned models: a `models.json` file holding every [`ModelRecord`]
//! the kernel knows about, with change-detecting writes and corruption recovery.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::persistence::{self, StoreError};
use crate::records::{ModelRecord, ModelState};

const STORE_FILE: &str = "models.json";
const LOCK_FILE: &str = "models.json.lock";
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

    /// The advisory file lock guarding a mutation could not be acquired.
    #[error("locking registry store: {0}")]
    Lock(String),
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
/// method acquires a short-held advisory file lock, reloads the on-disk state
/// under it, applies the change, and persists before returning — so two
/// instances on the same directory serialize their writes instead of one
/// silently clobbering the other. The lock is held only for the span of a
/// single mutation, never across the instance's lifetime.
#[derive(Debug)]
pub struct Registry {
    directory: PathBuf,
    models: BTreeMap<String, ModelRecord>,
    generation: u64,
}

impl Registry {
    /// Open the registry rooted at `directory`, loading `models.json` if present.
    /// A missing store opens empty; a corrupt store is quarantined and reported
    /// as [`RegistryError::CorruptStore`]; a store from a newer schema is left in
    /// place and reported as [`RegistryError::FutureSchema`]. If the file holds
    /// two records with the same id, the last one in file order wins.
    pub fn open(directory: &Path) -> Result<Self, RegistryError> {
        let models = Self::load_models(directory)?;
        Ok(Self {
            directory: directory.to_path_buf(),
            models,
            generation: 0,
        })
    }

    /// Read `<directory>/models.json` into a fresh map, applying the same
    /// missing/corrupt/future-schema handling as [`Self::open`].
    fn load_models(directory: &Path) -> Result<BTreeMap<String, ModelRecord>, RegistryError> {
        let file = directory.join(STORE_FILE);
        match persistence::read_json::<Envelope>(&file) {
            Ok(Some(envelope)) => {
                if envelope.schema_version > SCHEMA_VERSION {
                    return Err(RegistryError::FutureSchema {
                        found: envelope.schema_version,
                        supported: SCHEMA_VERSION,
                    });
                }
                Ok(envelope
                    .models
                    .into_iter()
                    .map(|record| (record.id.clone(), record))
                    .collect())
            }
            Ok(None) => Ok(BTreeMap::new()),
            Err(StoreError::Corrupt { source, .. }) => {
                Err(RegistryError::CorruptStore(source.to_string()))
            }
            Err(other) => Err(RegistryError::Store(other)),
        }
    }

    /// Re-read the on-disk store into memory, discarding the in-memory view. Called
    /// under the advisory lock so a mutation applies to the latest committed state.
    fn reload(&mut self) -> Result<(), RegistryError> {
        self.models = Self::load_models(&self.directory)?;
        Ok(())
    }

    /// Acquire an exclusive OS advisory lock on a `models.json.lock` sibling,
    /// blocking until it is available. The returned file releases the lock when
    /// dropped; callers hold it only for the span of one mutation, never longer.
    fn lock(&self) -> Result<std::fs::File, RegistryError> {
        use fs2::FileExt;
        std::fs::create_dir_all(&self.directory)
            .map_err(|source| RegistryError::Lock(source.to_string()))?;
        let path = self.directory.join(LOCK_FILE);
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|source| RegistryError::Lock(source.to_string()))?;
        file.lock_exclusive()
            .map_err(|source| RegistryError::Lock(source.to_string()))?;
        Ok(file)
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

    /// A counter that advances every time [`Self::save`] persists a change.
    /// Callers can cache work derived from the registry's contents keyed on this
    /// value and rebuild only when it moves.
    pub fn generation(&self) -> u64 {
        self.generation
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
        let _lock = self.lock()?;
        self.reload()?;
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
        let _lock = self.lock()?;
        self.reload()?;
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
        let _lock = self.lock()?;
        self.reload()?;
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
        let _lock = self.lock()?;
        self.reload()?;
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
        let _lock = self.lock()?;
        self.reload()?;
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

    fn save(&mut self) -> Result<(), RegistryError> {
        let envelope = EnvelopeRef {
            schema_version: SCHEMA_VERSION,
            models: self.models.values().collect(),
        };
        persistence::write_json_atomic(&self.directory.join(STORE_FILE), &envelope)?;
        self.generation += 1;
        Ok(())
    }
}
