//! Scans a local Ollama store (`~/.ollama/models`): walks `manifests/<registry>/
//! <namespace>/<model>/<tag>`, reads each manifest's layer list, and resolves the
//! weight/template/projector/params blobs into a [`DiscoveredModel`].

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::discovery::scanner::{DiscoveredModel, ScanResult, StoreScanner};
use crate::records::{JsonValue, ModelSource, SourceKind};
use crate::resolution::{gguf_general_architecture, ollama_profile};

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
    #[serde(default)]
    config: Option<Layer>,
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

            let bytes = match crate::fs::read_regular(&file) {
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
                .and_then(|layer| crate::fs::read_regular(&self.blob_path(&layer.digest)).ok())
                .and_then(|bytes| String::from_utf8(bytes).ok())
                .map(|template| template.contains(".Tools"));
            let has_projector = manifest
                .layers
                .iter()
                .any(|layer| layer.media_type.ends_with(".projector"));
            let architecture = weight_blob
                .as_deref()
                .and_then(|path| gguf_general_architecture(Path::new(path)));
            let profile = ollama_profile(has_projector, architecture.as_deref());

            let mut context_length_hint = None;
            let mut stop_tokens_hint = None;
            if let Some(params) = manifest
                .layers
                .iter()
                .find(|layer| layer.media_type.ends_with(".params"))
            {
                match crate::fs::read_regular(&self.blob_path(&params.digest))
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
            discovered.files = manifest
                .layers
                .iter()
                .map(|layer| display(&self.blob_path(&layer.digest)))
                .collect();
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

/// The layer blobs the manifest at `manifest` names, under the models root it
/// sits in (`<root>/manifests/<registry>/<namespace>/<model>/<tag>`). `None`
/// when the path is not laid out that way or the manifest cannot be read.
pub(crate) fn manifest_blobs(manifest: &Path) -> Option<Vec<PathBuf>> {
    let (store, parsed) = read_manifest(manifest)?;
    Some(
        parsed
            .layers
            .iter()
            .map(|layer| store.blob_path(&layer.digest))
            .collect(),
    )
}

/// The files besides its layers that the daemon deletes with the model whose
/// manifest is at `manifest`: the manifest itself, and its config blob when
/// it names one that is on disk. Neither holds weights when it is what the
/// daemon writes ([`manifest_as_ollama_writes`], [`config_as_ollama_writes`]).
/// `None` as [`manifest_blobs`] is.
pub(crate) fn manifest_bookkeeping(manifest: &Path) -> Option<Vec<PathBuf>> {
    let (store, parsed) = read_manifest(manifest)?;
    let mut files = vec![manifest.to_path_buf()];
    files.extend(
        parsed
            .config
            .map(|config| store.blob_path(&config.digest))
            .filter(|blob| blob.is_file()),
    );
    Some(files)
}

/// The most an Ollama manifest or config blob read as the daemon's own holds.
/// The daemon writes a few hundred bytes for each.
const WRITTEN_BYTES: u64 = 1 << 20;

/// The most a name in a manifest or config the daemon writes holds (a media
/// type, a family, a quantization, a renderer).
const NAME_BYTES: usize = 256;

/// The most names a list in a config the daemon writes holds (its families,
/// its capabilities).
const NAMES: usize = 64;

/// The most a model name in a manifest holds (a layer's `from` or `name`).
const ADDRESS_BYTES: usize = 4096;

/// Whether the file at `path` is an Ollama manifest as the daemon writes it:
/// JSON with a schema version of 2, a media type, a config layer and the
/// layers, each a media type, a `sha256:` digest, a size, and the model it
/// came `from`, and no other field, in at most a MiB. Nothing else can hide
/// there.
pub(crate) fn manifest_as_ollama_writes(path: &Path) -> bool {
    written_json::<WrittenManifest>(path).is_some_and(|manifest| manifest.plausible())
}

/// Whether the file at `path` is an Ollama config blob as the daemon writes
/// it for a local model: JSON with the model's format, family, type and
/// quantization, its renderer and parser names, the platform, and the
/// digests of its layers, and no other field, in at most a MiB. A cloud
/// model's config, naming the host it runs on, is that model, so it is not.
pub(crate) fn config_as_ollama_writes(path: &Path) -> bool {
    written_json::<WrittenConfig>(path).is_some_and(|config| config.plausible())
}

fn written_json<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > WRITTEN_BYTES {
        return None;
    }
    let bytes = crate::fs::read_regular(path).ok()?;
    if bytes.len() as u64 > WRITTEN_BYTES {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WrittenManifest {
    #[serde(rename = "schemaVersion")]
    schema_version: u32,
    #[serde(rename = "mediaType", default)]
    media_type: String,
    #[serde(default)]
    config: Option<WrittenLayer>,
    #[serde(default)]
    layers: Vec<WrittenLayer>,
}

impl WrittenManifest {
    fn plausible(&self) -> bool {
        self.schema_version == 2
            && self.media_type.len() <= NAME_BYTES
            && self
                .config
                .iter()
                .chain(&self.layers)
                .all(WrittenLayer::plausible)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WrittenLayer {
    #[serde(rename = "mediaType")]
    media_type: String,
    digest: String,
    size: u64,
    #[serde(default)]
    from: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

impl WrittenLayer {
    fn plausible(&self) -> bool {
        self.media_type.len() <= NAME_BYTES
            && is_digest(&self.digest)
            && self.size <= i64::MAX as u64
            && self
                .from
                .as_ref()
                .is_none_or(|from| from.len() <= ADDRESS_BYTES)
            && self
                .name
                .as_ref()
                .is_none_or(|name| name.len() <= ADDRESS_BYTES)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WrittenConfig {
    #[serde(default)]
    model_format: String,
    #[serde(default)]
    model_family: String,
    #[serde(default)]
    model_families: Option<Vec<String>>,
    #[serde(default)]
    model_type: String,
    #[serde(default)]
    file_type: String,
    #[serde(default)]
    renderer: String,
    #[serde(default)]
    parser: String,
    #[serde(default)]
    requires: String,
    #[serde(default)]
    remote_host: String,
    #[serde(default)]
    remote_model: String,
    #[serde(default)]
    capabilities: Option<Vec<String>>,
    #[serde(default)]
    context_length: u64,
    #[serde(default)]
    embedding_length: u64,
    #[serde(default)]
    base_name: String,
    #[serde(default)]
    architecture: String,
    #[serde(default)]
    os: String,
    #[serde(default)]
    rootfs: Option<WrittenRootFs>,
}

impl WrittenConfig {
    fn plausible(&self) -> bool {
        let names = [
            &self.model_format,
            &self.model_family,
            &self.model_type,
            &self.file_type,
            &self.renderer,
            &self.parser,
            &self.requires,
            &self.base_name,
            &self.architecture,
            &self.os,
        ];
        let lists = || self.model_families.iter().chain(&self.capabilities);
        lists().all(|list| list.len() <= NAMES)
            && names
                .into_iter()
                .chain(lists().flatten())
                .all(|name| name.len() <= NAME_BYTES)
            && self.remote_host.is_empty()
            && self.remote_model.is_empty()
            && self.context_length <= u64::from(u32::MAX)
            && self.embedding_length <= u64::from(u32::MAX)
            && self.rootfs.as_ref().is_none_or(WrittenRootFs::plausible)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WrittenRootFs {
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    diff_ids: Option<Vec<String>>,
}

impl WrittenRootFs {
    fn plausible(&self) -> bool {
        self.kind.len() <= NAME_BYTES && self.diff_ids.iter().flatten().all(|id| is_digest(id))
    }
}

/// Whether `text` is a digest as Ollama spells one: `sha256:` and 64 hex
/// characters.
fn is_digest(text: &str) -> bool {
    text.strip_prefix("sha256:")
        .is_some_and(|hex| hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

fn read_manifest(manifest: &Path) -> Option<(OllamaStoreScanner, Manifest)> {
    let manifests = manifest.ancestors().nth(4)?;
    if manifests.file_name()? != "manifests" {
        return None;
    }
    let store = OllamaStoreScanner::new(manifests.parent()?);
    let bytes = crate::fs::read_regular(manifest).ok()?;
    let parsed = serde_json::from_slice::<Manifest>(&bytes).ok()?;
    Some((store, parsed))
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

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = r#"{"schemaVersion":2,"mediaType":"application/vnd.docker.distribution.manifest.v2+json","config":{"mediaType":"application/vnd.docker.container.image.v1+json","digest":"sha256:f0988ff50a2458c598ff6b1b87b94d0f5c44d73061c2795391878b00b2285e11","size":473},"layers":[{"mediaType":"application/vnd.ollama.image.model","digest":"sha256:4c27e0f5b5adf02ac956c7322bd2ee7636fe3f45a8512c9aba5385242cb6e09a","size":9608338848},{"mediaType":"application/vnd.ollama.image.license","digest":"sha256:7339fa418c9ad3e8e12e74ad0fd26a9cc4be8703f9c110728a992b193be85cb2","size":11355}]}"#;

    const CONFIG: &str = r#"{"model_format":"gguf","model_family":"gemma4","model_families":["gemma4"],"model_type":"8.0B","file_type":"Q4_K_M","renderer":"gemma4","parser":"gemma4","requires":"0.20.0","architecture":"amd64","os":"linux","rootfs":{"type":"layers","diff_ids":["sha256:4c27e0f5b5adf02ac956c7322bd2ee7636fe3f45a8512c9aba5385242cb6e09a"]}}"#;

    /// Whether `text`, written to a file, passes `check`.
    fn passes(check: fn(&Path) -> bool, text: &str) -> bool {
        let path = std::env::temp_dir().join(format!(
            "hedos-ollama-written-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::write(&path, text).unwrap();
        let passed = check(&path);
        std::fs::remove_file(&path).ok();
        passed
    }

    #[test]
    fn a_manifest_the_daemon_wrote_is_its_own_and_one_with_more_is_not() {
        assert!(passes(manifest_as_ollama_writes, MANIFEST));
        let noted = MANIFEST.replacen('{', r#"{"notes":"mine","#, 1);
        assert!(!passes(manifest_as_ollama_writes, &noted));
        let short = MANIFEST.replace("sha256:7339fa41", "sha256:zz");
        assert!(!passes(manifest_as_ollama_writes, &short));
        let old = MANIFEST.replace(r#""schemaVersion":2"#, r#""schemaVersion":1"#);
        assert!(!passes(manifest_as_ollama_writes, &old));
        assert!(!passes(manifest_as_ollama_writes, "not json"));
    }

    #[test]
    fn a_config_the_daemon_wrote_is_its_own_and_one_with_more_is_not() {
        assert!(passes(config_as_ollama_writes, CONFIG));
        assert!(passes(config_as_ollama_writes, "{}"));
        let diary = CONFIG.replacen('{', r#"{"diary":"mine","#, 1);
        assert!(!passes(config_as_ollama_writes, &diary));
        let long = CONFIG.replace(
            "gemma4\",\"parser",
            &format!("{}\",\"parser", "x".repeat(300)),
        );
        assert!(!passes(config_as_ollama_writes, &long));
        let cloud = CONFIG.replacen('{', r#"{"remote_host":"https://ollama.com:443","#, 1);
        assert!(
            !passes(config_as_ollama_writes, &cloud),
            "a cloud model's config is the model"
        );
    }
}
