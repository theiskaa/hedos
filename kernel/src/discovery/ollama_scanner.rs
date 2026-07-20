//! Scans a local Ollama store (`~/.ollama/models`): walks `manifests/<registry>/
//! <namespace>/<model>/<tag>`, reads each manifest's layer list, and resolves the
//! weight/template/projector/params blobs into a [`DiscoveredModel`].

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::discovery::scanner::{DiscoveredModel, ScanResult, StoreScanner};
use crate::records::{JsonValue, ModelSource, SourceKind};
use crate::resolution::ollama_profile;

/// A scanner over one Ollama models root.
pub struct OllamaStoreScanner {
    root: PathBuf,
}

impl OllamaStoreScanner {
    /// A scanner rooted at an Ollama models directory (the one holding
    /// `manifests/` and `blobs/`).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn blob_path(&self, digest: &str) -> PathBuf {
        self.root.join("blobs").join(digest.replace(':', "-"))
    }
}

#[derive(Debug, Deserialize)]
struct Manifest {
    #[serde(default)]
    layers: Vec<Layer>,
}

#[derive(Debug, Deserialize)]
struct Layer {
    #[serde(rename = "mediaType", default)]
    media_type: String,
    #[serde(default)]
    size: i64,
    #[serde(default)]
    digest: String,
}

impl StoreScanner for OllamaStoreScanner {
    fn kinds(&self) -> Vec<SourceKind> {
        vec![SourceKind::ollama()]
    }

    fn scan(&self) -> ScanResult {
        let mut result = ScanResult::default();
        if !self.root.exists() {
            return result;
        }
        // Root exists but can't be listed (no search permission, or it isn't a
        // directory) — that's a scan failure, not an empty store.
        if std::fs::read_dir(&self.root).is_err() {
            result.failed_kinds.push(SourceKind::ollama());
            return result;
        }
        let manifests = self.root.join("manifests");
        if !manifests.exists() {
            return result;
        }

        let mut files = Vec::new();
        if collect_files(&manifests, &mut files).is_err() {
            result.failed_kinds.push(SourceKind::ollama());
            return result;
        }

        for file in files {
            let Ok(relative) = file.strip_prefix(&manifests) else {
                continue;
            };
            // Map (don't drop) each component so a non-UTF-8 segment can't shift
            // the count — the four-component check must see the true depth.
            let parts: Vec<std::borrow::Cow<str>> = relative
                .components()
                .map(|component| component.as_os_str().to_string_lossy())
                .collect();
            // `<registry>/<namespace>/<model>/<tag>` — exactly four components.
            let [_, namespace, model, tag] = parts.as_slice() else {
                continue;
            };

            let bytes = match std::fs::read(&file) {
                Ok(bytes) => bytes,
                Err(_) => {
                    result
                        .issues
                        .push(format!("ollama: unreadable manifest {}", display(&file)));
                    continue;
                }
            };
            let manifest = match serde_json::from_slice::<Manifest>(&bytes) {
                Ok(manifest) => manifest,
                Err(error) => {
                    result.issues.push(format!(
                        "ollama: unreadable manifest {}: {error}",
                        display(&file)
                    ));
                    continue;
                }
            };

            let name = if namespace.as_ref() == "library" {
                format!("{model}:{tag}")
            } else {
                format!("{namespace}/{model}:{tag}")
            };
            let footprint: i64 = manifest.layers.iter().map(|layer| layer.size).sum();
            let weight_blob = manifest
                .layers
                .iter()
                .find(|layer| layer.media_type.ends_with(".model"))
                .map(|layer| display(&self.blob_path(&layer.digest)));
            let template_layer = manifest
                .layers
                .iter()
                .find(|layer| layer.media_type.ends_with(".template"));
            let has_template = template_layer.is_some();
            // Ollama decides tool support from this Go template: a tool-capable
            // model gates its output on `.Tools`. Reading it is authoritative —
            // the same signal `/api/show` reports — and needs no daemon. A model
            // whose template we can't read stays undetermined (`None`).
            let tool_capable_hint = template_layer
                .and_then(|layer| std::fs::read_to_string(self.blob_path(&layer.digest)).ok())
                .map(|template| template.contains(".Tools"));
            let has_projector = manifest
                .layers
                .iter()
                .any(|layer| layer.media_type.ends_with(".projector"));
            let profile = ollama_profile(has_projector, weight_blob.as_deref());

            let mut context_length_hint = None;
            let mut stop_tokens_hint = None;
            if let Some(params) = manifest
                .layers
                .iter()
                .find(|layer| layer.media_type.ends_with(".params"))
            {
                match std::fs::read(self.blob_path(&params.digest))
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<JsonValue>(&bytes).ok())
                {
                    Some(JsonValue::Object(fields)) => {
                        context_length_hint = fields
                            .get("num_ctx")
                            .and_then(JsonValue::as_i64)
                            .filter(|value| *value > 0);
                        stop_tokens_hint = fields.get("stop").and_then(string_array);
                    }
                    _ => result
                        .issues
                        .push(format!("ollama: unreadable params blob for {name}")),
                }
            }

            let mut source = ModelSource::new(SourceKind::ollama(), &display(&file));
            source.repo = Some(name.clone());
            let mut discovered = DiscoveredModel::new(name, source);
            discovered.modality_hint = Some(profile.modality);
            discovered.capabilities_hint = profile.capabilities;
            discovered.execution_hint = profile.execution;
            discovered.footprint_bytes = footprint;
            discovered.primary_weight_path = weight_blob;
            discovered.context_length_hint = context_length_hint;
            discovered.has_chat_template_hint = has_template.then_some(true);
            discovered.tool_capable_hint = tool_capable_hint;
            discovered.stop_tokens_hint = stop_tokens_hint;
            result.discovered.push(discovered);
        }

        result
    }
}

/// Recursively collect the regular files under `dir` (skipping hidden entries).
/// An error reading `dir` itself propagates; a subdirectory that can't be read is
/// skipped so one bad directory doesn't abort the whole scan.
fn collect_files(dir: &Path, into: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with('.'))
        {
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            let _ = collect_files(&path, into);
        } else if file_type.is_file() {
            into.push(path);
        }
    }
    Ok(())
}

fn string_array(value: &JsonValue) -> Option<Vec<String>> {
    let JsonValue::Array(items) = value else {
        return None;
    };
    // All-or-nothing: a single non-string element voids the whole array rather
    // than being silently dropped.
    items
        .iter()
        .map(|item| item.as_str().map(str::to_owned))
        .collect()
}

fn display(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
