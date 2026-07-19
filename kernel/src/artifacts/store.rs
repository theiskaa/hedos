//! The content-addressed artifact store: `<year>/<slug>_<hash12>.<ext>` outputs
//! (deduplicated by content), `blobs/<hash>` previews, and `<year>/<id>.json`
//! provenance sidecars. The in-memory index is rebuilt by scanning the sidecars.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::persistence;
use crate::util::{hex_encode, now_millis};

use super::artifact::{Artifact, ArtifactDraft};

/// Errors from the artifact store.
#[derive(Debug, thiserror::Error)]
pub enum ArtifactStoreError {
    /// No artifact with the given id is stored.
    #[error("no artifact with id {0} is stored")]
    NotFound(String),
    /// A filesystem operation failed.
    #[error("artifact store io error: {0}")]
    Io(#[from] std::io::Error),
    /// Writing a provenance sidecar failed.
    #[error("artifact sidecar error: {0}")]
    Sidecar(#[from] persistence::StoreError),
}

/// A content-addressed store of generated outputs, rooted at a directory. The
/// in-memory index is populated lazily on first access, so read methods take
/// `&mut self`.
pub struct ArtifactStore {
    root: PathBuf,
    artifacts: HashMap<String, Artifact>,
    loaded: bool,
}

impl ArtifactStore {
    /// A store rooted at `root` (created lazily on first write).
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            artifacts: HashMap::new(),
            loaded: false,
        }
    }

    /// Store `draft`, writing its bytes (deduplicated), optional preview, and a
    /// provenance sidecar, and return the resulting record.
    pub fn store(&mut self, draft: ArtifactDraft) -> Result<Artifact, ArtifactStoreError> {
        self.load_if_needed()?;
        let created_at = now_millis();
        let hash = hex_encode(&Sha256::digest(&draft.data));
        let slug = slug(&draft.model);
        let year = year_of(created_at);
        let path = format!("{year}/{slug}_{}.{}", &hash[..12], draft.file_extension);
        self.write_if_absent(&draft.data, &path)?;
        let preview_path = match &draft.preview {
            Some(preview) => Some(self.spill(preview)?),
            None => None,
        };
        let artifact = Artifact {
            id: self.unique_id(&slug, &hash, &draft.job_id),
            path,
            content_hash: hash,
            preview_path,
            model: draft.model,
            model_id: draft.model_id,
            runtime: draft.runtime,
            capability: draft.capability,
            params: draft.params,
            created_at,
            duration_ms: draft.duration_ms,
            job_id: draft.job_id,
            session_id: draft.session_id,
        };
        self.write_sidecar(&artifact)?;
        self.artifacts.insert(artifact.id.clone(), artifact.clone());
        Ok(artifact)
    }

    /// Every artifact, newest first by `(created_at, id)`.
    pub fn list(&mut self) -> Result<Vec<Artifact>, ArtifactStoreError> {
        self.load_if_needed()?;
        let mut artifacts: Vec<Artifact> = self.artifacts.values().cloned().collect();
        artifacts.sort_by(super::gallery::newest);
        Ok(artifacts)
    }

    /// The artifact with `id`, if stored.
    pub fn get(&mut self, id: &str) -> Result<Option<Artifact>, ArtifactStoreError> {
        self.load_if_needed()?;
        Ok(self.artifacts.get(id).cloned())
    }

    /// The absolute path to `id`'s output file, if stored.
    pub fn url(&mut self, id: &str) -> Result<Option<PathBuf>, ArtifactStoreError> {
        self.load_if_needed()?;
        Ok(self
            .artifacts
            .get(id)
            .map(|artifact| self.root.join(&artifact.path)))
    }

    /// The bytes of `id`'s preview, if it has one.
    pub fn preview_data(&mut self, id: &str) -> Result<Option<Vec<u8>>, ArtifactStoreError> {
        self.load_if_needed()?;
        let Some(preview_path) = self.artifacts.get(id).and_then(|a| a.preview_path.clone()) else {
            return Ok(None);
        };
        Ok(Some(std::fs::read(self.root.join(preview_path))?))
    }

    /// Delete `id`: remove its sidecar always, and its output/preview files only
    /// when no surviving artifact still references them (deduplicated files stay
    /// until their last owner is gone). Files are unlinked permanently.
    pub fn delete(&mut self, id: &str) -> Result<(), ArtifactStoreError> {
        self.load_if_needed()?;
        let Some(artifact) = self.artifacts.remove(id) else {
            return Err(ArtifactStoreError::NotFound(id.to_owned()));
        };
        remove_if_present(&self.sidecar_path(&artifact))?;
        if !self
            .artifacts
            .values()
            .any(|other| other.path == artifact.path)
        {
            remove_if_present(&self.root.join(&artifact.path))?;
        }
        if let Some(preview_path) = &artifact.preview_path
            && !self
                .artifacts
                .values()
                .any(|other| other.preview_path.as_deref() == Some(preview_path))
        {
            remove_if_present(&self.root.join(preview_path))?;
        }
        Ok(())
    }

    fn write_if_absent(&self, data: &[u8], path: &str) -> Result<(), ArtifactStoreError> {
        let url = self.root.join(path);
        if url.exists() {
            return Ok(());
        }
        persistence::write_atomic(&url, data)?;
        Ok(())
    }

    fn spill(&self, preview: &[u8]) -> Result<String, ArtifactStoreError> {
        let hash = hex_encode(&Sha256::digest(preview));
        let path = format!("blobs/{hash}");
        self.write_if_absent(preview, &path)?;
        Ok(path)
    }

    fn unique_id(&self, slug: &str, hash: &str, job_id: &str) -> String {
        let job_prefix: String = job_id.chars().take(8).collect::<String>().to_lowercase();
        let base = format!("{slug}_{}_{}", &hash[..12], job_prefix);
        if !self.artifacts.contains_key(&base) {
            return base;
        }
        let mut counter = 2;
        while self.artifacts.contains_key(&format!("{base}-{counter}")) {
            counter += 1;
        }
        format!("{base}-{counter}")
    }

    fn sidecar_path(&self, artifact: &Artifact) -> PathBuf {
        self.root
            .join(year_of(artifact.created_at).to_string())
            .join(format!("{}.json", artifact.id))
    }

    fn write_sidecar(&self, artifact: &Artifact) -> Result<(), ArtifactStoreError> {
        persistence::write_json_atomic(&self.sidecar_path(artifact), artifact)?;
        Ok(())
    }

    fn load_if_needed(&mut self) -> Result<(), ArtifactStoreError> {
        if self.loaded {
            return Ok(());
        }
        self.loaded = true;
        let Ok(entries) = std::fs::read_dir(&self.root) else {
            return Ok(());
        };
        let mut scanned = HashMap::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() || !is_year_dir(&path) {
                continue;
            }
            let Ok(sidecars) = std::fs::read_dir(&path) else {
                continue;
            };
            for sidecar in sidecars.flatten() {
                let sidecar_path = sidecar.path();
                if sidecar_path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                    continue;
                }
                match persistence::read_json::<Artifact>(&sidecar_path) {
                    Ok(Some(artifact)) => {
                        scanned.insert(artifact.id.clone(), artifact);
                    }
                    // Missing/corrupt sidecar (quarantined inside read_json) is skipped.
                    _ => continue,
                }
            }
        }
        self.artifacts = scanned;
        Ok(())
    }
}

fn remove_if_present(path: &Path) -> Result<(), ArtifactStoreError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.into()),
    }
}

fn is_year_dir(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.parse::<i64>().is_ok())
}

fn slug(name: &str) -> String {
    let kept: String = name
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric())
        .take(24)
        .collect();
    if kept.is_empty() {
        "artifact".to_owned()
    } else {
        kept
    }
}

/// The Gregorian year for an epoch-millisecond timestamp, computed with the
/// civil-from-days algorithm (no calendar crate). The year is in UTC, so an
/// artifact created near midnight in an offset zone can land in a different year
/// directory than the machine's local calendar would pick. This is cosmetic — the
/// store stays self-consistent and `load_if_needed` scans every year dir.
fn year_of(millis: i64) -> i64 {
    let days = millis.div_euclid(86_400_000);
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    // The algorithm's era starts in March, so January and February belong to the
    // following civil year.
    year + i64::from(month <= 2)
}
