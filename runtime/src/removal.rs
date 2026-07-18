//! Running a model deletion: trashing the files a non-daemon model owns, or
//! asking the Ollama daemon to delete a tag. The pure preview/path logic lives
//! in [`kernel::removal`]; this drives it.

use std::collections::HashMap;
use std::path::Path;

use kernel::records::ModelRecord;
use kernel::removal::{ModelDeletionReport, RemovalError, preview};

use crate::install::ollama::{daemon_binary, daemon_reachable, error_message, start_daemon};

const DEFAULT_BASE_URL: &str = "http://127.0.0.1:11434";

/// Disposes of a file at `path`, returning a human-readable reason on failure.
/// The caller chooses the policy — move to the OS trash, or delete permanently.
pub type Trasher = Box<dyn Fn(&Path) -> Result<(), String> + Send + Sync>;

/// A [`Trasher`] that permanently removes the path (files and directories).
/// Callers wanting recoverable deletion supply a trash-backed one instead.
pub fn permanent_delete_trasher() -> Trasher {
    Box::new(|path| {
        let result = if path.is_dir() {
            std::fs::remove_dir_all(path)
        } else {
            std::fs::remove_file(path)
        };
        result.map_err(|error| error.to_string())
    })
}

/// Removes installed models: trashes files, or deletes Ollama tags via the daemon.
pub struct ModelRemover {
    trasher: Trasher,
    ollama: OllamaModelRemover,
}

impl ModelRemover {
    /// A remover that disposes of files with `trasher` and deletes Ollama models
    /// through `ollama`.
    pub fn new(trasher: Trasher, ollama: OllamaModelRemover) -> Self {
        Self { trasher, ollama }
    }

    /// Delete `record`, returning what was removed. Ollama models go through the
    /// daemon; everything else is trashed file by file.
    pub async fn remove(&self, record: &ModelRecord) -> Result<ModelDeletionReport, RemovalError> {
        let details = preview(record);
        if details.via_daemon {
            let tag = record
                .source
                .repo
                .clone()
                .unwrap_or_else(|| record.name.clone());
            self.ollama.delete(&tag).await?;
            return Ok(ModelDeletionReport {
                model_id: details.model_id,
                name: details.name,
                kind: details.kind,
                trashed_paths: Vec::new(),
                freed_bytes_estimate: details.bytes_estimate,
                daemon_deleted: true,
            });
        }

        let mut trashed = Vec::new();
        for path in &details.paths {
            (self.trasher)(Path::new(path)).map_err(|reason| RemovalError::TrashFailed {
                path: path.clone(),
                reason,
            })?;
            trashed.push(path.clone());
        }
        Ok(ModelDeletionReport {
            model_id: details.model_id,
            name: details.name,
            kind: details.kind,
            trashed_paths: trashed,
            freed_bytes_estimate: details.bytes_estimate,
            daemon_deleted: false,
        })
    }
}

/// Reports whether the Ollama binary is present (so the daemon can be started).
type BinaryProbe = Box<dyn Fn() -> bool + Send + Sync>;

/// Deletes Ollama models by asking the daemon (`DELETE /api/delete`), starting it
/// first if a binary is present but nothing is listening.
pub struct OllamaModelRemover {
    base_url: String,
    client: reqwest::Client,
    environment: HashMap<String, String>,
    binary_present: Option<BinaryProbe>,
}

impl OllamaModelRemover {
    /// A remover pointed at the default local Ollama, reading the process env.
    pub fn new() -> Self {
        Self::with_config(DEFAULT_BASE_URL, std::env::vars().collect())
    }

    /// A remover pointed at `base_url` with an explicit environment (for tests).
    pub fn with_config(base_url: impl Into<String>, environment: HashMap<String, String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            client: reqwest::Client::new(),
            environment,
            binary_present: None,
        }
    }

    /// Override the binary-presence probe (tests, so a real `ollama serve` isn't
    /// spawned). By default the well-known locations + `PATH` are checked.
    pub fn with_binary_present(mut self, probe: impl Fn() -> bool + Send + Sync + 'static) -> Self {
        self.binary_present = Some(Box::new(probe));
        self
    }

    fn has_binary(&self) -> bool {
        match &self.binary_present {
            Some(probe) => probe(),
            None => daemon_binary(&self.environment).is_some(),
        }
    }

    /// Delete the model `tag`. Treats a 404 as success (already gone).
    pub async fn delete(&self, tag: &str) -> Result<(), RemovalError> {
        if !daemon_reachable(&self.client, &self.base_url).await {
            if !self.has_binary() {
                return Err(RemovalError::DaemonUnavailable(
                    "Ollama isn't running, and hedos can't delete its models without the daemon. \
                     Install Ollama or start it, then retry."
                        .to_owned(),
                ));
            }
            start_daemon(&self.client, &self.base_url, &self.environment)
                .await
                .map_err(|error| RemovalError::DaemonUnavailable(error.to_string()))?;
        }

        let url = format!("{}/api/delete", self.base_url);
        let body = serde_json::to_vec(&serde_json::json!({ "model": tag }))
            .map_err(|error| RemovalError::DaemonDeleteFailed(error.to_string()))?;
        let response = self
            .client
            .delete(&url)
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await
            .map_err(|error| RemovalError::DaemonDeleteFailed(format!("ollama: {error}")))?;

        let status = response.status().as_u16();
        match status {
            200 | 404 => Ok(()),
            _ => {
                let body = response
                    .bytes()
                    .await
                    .map(|bytes| bytes.to_vec())
                    .unwrap_or_default();
                Err(RemovalError::DaemonDeleteFailed(error_message(
                    &body,
                    i64::from(status),
                )))
            }
        }
    }
}

impl Default for OllamaModelRemover {
    fn default() -> Self {
        Self::new()
    }
}
