//! Support logic for manifest-declared runtimes: resolving a model's on-disk
//! paths, substituting the `{model}`/`{prompt}`/`{workdir}`/`{outputs}`/`{python}`
//! placeholders in a manifest command, generating a starter manifest for a model,
//! and the small string/JSON helpers the manifest adapters share.
//!
//! The command runtimes launch the interpreter directly rather than through a
//! macOS sandbox wrapper; this module is the cross-platform core the manifest
//! adapters build on.

mod community;
mod consent;
mod install;

/// The data directory's folder for the commands manifests install.
pub const COMMANDS_DIR: &str = "commands";
mod store;

pub use community::{CommunityLibrary, ManifestInstaller, Recipe, RuntimeInstallPreview};
pub use consent::{HostConsent, host_consent, servable_models};
pub use install::{InstallError, host_build, host_target, install, installed, unpack};
pub(crate) use store::Hashing;
pub use store::{RuntimeCatalog, StoreLoad, UserRuntimeStore};

use std::collections::BTreeMap;
use std::path::Path;

use kernel::manifests::ManifestDetect;
use kernel::records::{Capability, ExecutionMode, JsonValue, Modality, ModelRecord, SourceKind};
use kernel::resolution::{IdentifiedModel, ModelFormat, identify};

/// The largest output file a manifest command may produce before it is refused
/// (256 MiB), guarding against a runaway runtime filling memory.
pub const MAX_OUTPUT_FILE_BYTES: u64 = 256 * 1024 * 1024;

/// Why manifest support could not produce a result.
#[derive(Debug, Clone, thiserror::Error)]
pub enum ManifestError {
    /// The command or its output was rejected.
    #[error("{0}")]
    Failed(String),
}

/// A model's resolved paths for a sandboxed runtime: the `sandbox_root` to mount
/// and the `snapshot` directory the weights actually live in (the Hugging Face
/// `snapshots/<ref>` when the record is a cache entry, else the root itself).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidecarModelPaths {
    /// The directory to grant the sandbox.
    pub sandbox_root: String,
    /// The directory holding the weights.
    pub snapshot: String,
}

impl SidecarModelPaths {
    /// Resolve `record` to its sandbox root and snapshot directory. Symlinks in
    /// the path are resolved when possible; a leading `~` is not expanded
    /// (discovery passes absolute paths).
    pub fn resolve(record: &ModelRecord) -> Self {
        let base = Path::new(&record.source.path);
        // canonicalize resolves symlinks but fails if the path is absent; fall
        // back to the path as given so a not-yet-present model still resolves.
        let root = std::fs::canonicalize(base).unwrap_or_else(|_| base.to_path_buf());
        if record.source.kind == SourceKind::huggingface_cache()
            && let Some(reference) = &record.source.reference
        {
            let snapshot = root.join("snapshots").join(reference);
            if snapshot.exists() {
                return Self {
                    sandbox_root: root.to_string_lossy().into_owned(),
                    snapshot: snapshot.to_string_lossy().into_owned(),
                };
            }
        }
        let root = root.to_string_lossy().into_owned();
        Self {
            sandbox_root: root.clone(),
            snapshot: root,
        }
    }
}

/// Whether `ch` is a line separator — any Unicode newline, not just `\n`, since
/// sidecars emit `\r`-based progress bars (tqdm, download bars) whose final
/// segment is the real error.
fn is_newline(ch: char) -> bool {
    matches!(
        ch,
        '\n' | '\r' | '\u{0B}' | '\u{0C}' | '\u{0085}' | '\u{2028}' | '\u{2029}'
    )
}

/// A one-line summary of a runtime's failure output: the last non-blank line,
/// trimmed and capped at 300 characters, or a fixed message when there was none.
pub fn error_summary(raw: &str) -> String {
    let last = raw
        .split(is_newline)
        .map(str::trim)
        .rfind(|line| !line.is_empty());
    match last {
        None => "the runtime stopped without output".to_owned(),
        Some(line) => {
            let chars: Vec<char> = line.chars().collect();
            let start = chars.len().saturating_sub(300);
            chars[start..].iter().collect()
        }
    }
}

/// Whether `detect` recognizes `record`: the `model_type` its `config.json`
/// names, the pipeline class its `model_index.json` names, a weight-file
/// extension match, or a marker file in the model's snapshot (optionally
/// required to contain a string).
pub fn detect_matches(detect: &ManifestDetect, record: &ModelRecord) -> bool {
    let snapshot = SidecarModelPaths::resolve(record).snapshot;
    let snapshot = Path::new(&snapshot);
    match detect {
        ManifestDetect::ModelType(model_type) => {
            json_string(&snapshot.join("config.json"), "model_type").as_ref() == Some(model_type)
        }
        ManifestDetect::Pipeline(pipeline) => {
            json_string(&snapshot.join("model_index.json"), "_class_name").as_ref()
                == Some(pipeline)
        }
        ManifestDetect::Extension(extension) => {
            let candidate = record
                .primary_weight_path
                .as_deref()
                .unwrap_or(&record.source.path);
            candidate
                .to_lowercase()
                .ends_with(&format!(".{}", extension.to_lowercase()))
        }
        ManifestDetect::File { path, contains } => {
            let target = snapshot.join(path);
            match contains {
                None => target.exists(),
                Some(contains) => kernel::fs::read_regular(&target).is_ok_and(|content| {
                    String::from_utf8_lossy(&content).contains(contains.as_str())
                }),
            }
        }
    }
}

/// The string at top-level `key` of the JSON object in `path`, when the file
/// reads, parses as an object, and holds a string there.
fn json_string(path: &Path, key: &str) -> Option<String> {
    let data = kernel::fs::read_regular(path).ok()?;
    let json: serde_json::Value = serde_json::from_slice(&data).ok()?;
    json.get(key)?.as_str().map(str::to_owned)
}

/// A filesystem-safe slug of `id`: every non-alphanumeric character becomes `-`.
pub fn slug(id: &str) -> String {
    id.chars()
        .map(|ch| if ch.is_alphanumeric() { ch } else { '-' })
        .collect()
}

/// The prompt text for a payload: its `prompt` string, else the flattened
/// conversation.
pub fn prompt_text(payload: &JsonValue) -> String {
    if let JsonValue::Object(object) = payload
        && let Some(JsonValue::String(prompt)) = object.get("prompt")
    {
        return prompt.clone();
    }
    conversation_text(payload)
}

/// The `messages` array flattened to `role: content` lines.
pub fn conversation_text(payload: &JsonValue) -> String {
    let JsonValue::Object(object) = payload else {
        return String::new();
    };
    let Some(JsonValue::Array(entries)) = object.get("messages") else {
        return String::new();
    };
    let mut lines = Vec::new();
    for entry in entries {
        if let JsonValue::Object(fields) = entry
            && let Some(JsonValue::String(role)) = fields.get("role")
            && let Some(JsonValue::String(content)) = fields.get("content")
        {
            lines.push(format!("{role}: {content}"));
        }
    }
    lines.join("\n")
}

/// Read `path`, refusing a file larger than `limit` bytes.
pub fn bounded_output_data(path: &Path, limit: u64) -> Result<Vec<u8>, ManifestError> {
    let size = std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);
    if size > limit {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("output");
        return Err(ManifestError::Failed(format!(
            "output file {name} is {size} bytes, larger than the {limit} cap"
        )));
    }
    std::fs::read(path).map_err(|error| ManifestError::Failed(format!("reading output: {error}")))
}

/// Replace every placeholder key in `token` with its value, scanning left to
/// right and preferring the longest matching key at each position (so a longer
/// key is never shadowed by a shorter prefix of it).
pub fn expand_placeholders(token: &str, replacements: &BTreeMap<String, String>) -> String {
    let mut keys: Vec<&String> = replacements.keys().collect();
    keys.sort_by_key(|key| std::cmp::Reverse(key.len()));
    let mut result = String::new();
    let mut rest = token;
    while !rest.is_empty() {
        let mut matched = false;
        for key in &keys {
            if let Some(tail) = rest.strip_prefix(key.as_str()) {
                if let Some(value) = replacements.get(*key) {
                    result.push_str(value);
                }
                rest = tail;
                matched = true;
                break;
            }
        }
        if !matched {
            match rest.chars().next() {
                Some(ch) => {
                    result.push(ch);
                    rest = &rest[ch.len_utf8()..];
                }
                None => break,
            }
        }
    }
    result
}

/// Substitute a manifest command into its argument vector, expanding the model,
/// prompt, workdir, outputs, (when an environment is prepared) python, and
/// (when the manifest installs its command) bin placeholders. Errors if the
/// command uses `{python}` without an environment or resolves to no tokens.
pub fn substituted(
    command: &str,
    record: &ModelRecord,
    payload: &JsonValue,
    workdir: &Path,
    outputs: &Path,
    env_dir: Option<&Path>,
    bin: Option<&Path>,
) -> Result<Vec<String>, ManifestError> {
    if command.contains("{python}") && env_dir.is_none() {
        return Err(ManifestError::Failed(
            "the command uses {python} but the manifest declares no [env]".to_owned(),
        ));
    }
    let paths = SidecarModelPaths::resolve(record);
    let mut replacements = BTreeMap::new();
    replacements.insert("{model}".to_owned(), paths.snapshot);
    replacements.insert("{prompt}".to_owned(), prompt_text(payload));
    replacements.insert(
        "{workdir}".to_owned(),
        workdir.to_string_lossy().into_owned(),
    );
    replacements.insert(
        "{outputs}".to_owned(),
        outputs.to_string_lossy().into_owned(),
    );
    if let Some(env_dir) = env_dir {
        replacements.insert(
            "{python}".to_owned(),
            env_dir.join("bin/python").to_string_lossy().into_owned(),
        );
    }
    if let Some(bin) = bin {
        replacements.insert("{bin}".to_owned(), bin.to_string_lossy().into_owned());
    }
    // Splitting on spaces drops empty subsequences (runs of spaces).
    let tokens: Vec<String> = command
        .split(' ')
        .filter(|token| !token.is_empty())
        .map(|token| expand_placeholders(token, &replacements))
        .collect();
    if tokens.is_empty() {
        return Err(ManifestError::Failed(
            "the manifest command is empty".to_owned(),
        ));
    }
    Ok(tokens)
}

/// The static tail of a starter manifest: the `[env]`/`[serve]` stanzas, the
/// commented one-shot `[invoke]` alternative, and `[permissions]`. Flush-left so
/// the generated TOML has no stray indentation.
const TEMPLATE_BODY: &str = r#"
[env]
manager  = "uv"
python   = "3.12"
lockfile = "requirements.lock"

[serve]
entrypoint = "main.py"
protocol   = "ndjson+frames"

# or replace [env]+[serve] with a one-shot command:
# [invoke]
# command = "your-tool --model {model} --prompt {prompt} --out {outputs}"

[permissions]
network = false
paths   = ["{model}", "{workdir}"]"#;

/// A starter TOML manifest for `record` — the scaffolding a user fills in to
/// declare a custom runtime: an id, the modality/capabilities/execution inferred
/// from identification, a detect rule, and the `[env]`/`[serve]`/`[invoke]`
/// stanzas.
pub fn template_for(record: &ModelRecord) -> String {
    render_template(record, &identify(record))
}

fn render_template(record: &ModelRecord, identified: &IdentifiedModel) -> String {
    let slug = slug(&record.display_name().to_lowercase());
    let modality = identified.modality.as_ref().unwrap_or(&record.modality);
    let capabilities = if identified.capabilities.is_empty() {
        default_capabilities(modality, identified.execution)
    } else {
        identified.capabilities.clone()
    };
    let execution = if capabilities.iter().any(Capability::generates) {
        "job"
    } else {
        "stream"
    };
    let capabilities = capabilities
        .iter()
        .map(|capability| format!("\"{}\"", capability.as_str()))
        .collect::<Vec<_>>()
        .join(", ");
    let detect = detect_line(record, identified);
    format!(
        "id           = \"{slug}\"\n\
         modalities   = [\"{}\"]\n\
         capabilities = [{capabilities}]\n\
         execution    = \"{execution}\"\n\
         {detect}\n{TEMPLATE_BODY}",
        modality.as_str(),
    )
}

/// The capabilities a manifest starts with for `modality` when identification
/// named none. An `audio` model identified as a job makes audio; any other
/// `audio` model transcribes it.
fn default_capabilities(modality: &Modality, execution: ExecutionMode) -> Vec<Capability> {
    if *modality == Modality::image() {
        vec![Capability::image()]
    } else if *modality == Modality::speech() {
        vec![Capability::speak()]
    } else if *modality == Modality::audio() && execution == ExecutionMode::Job {
        vec![Capability::audio()]
    } else if *modality == Modality::audio() {
        vec![Capability::transcribe()]
    } else {
        vec![Capability::chat(), Capability::complete()]
    }
}

/// The `detect = { … }` line that recognizes `record`: a diffusers pipeline
/// class, else the `model_type` its `config.json` names, else a weight-file
/// extension, else a `config.json` architecture, else a bare `config.json`
/// presence check.
fn detect_line(record: &ModelRecord, identified: &IdentifiedModel) -> String {
    if identified.format == ModelFormat::Diffusers
        && let Some(pipeline_class) = &identified.pipeline_class
    {
        return format!("detect       = {{ pipeline = \"{pipeline_class}\" }}");
    }
    let snapshot = SidecarModelPaths::resolve(record).snapshot;
    if let Some(model_type) = json_string(&Path::new(&snapshot).join("config.json"), "model_type") {
        return format!("detect       = {{ model_type = \"{model_type}\" }}");
    }
    if let Some(weight) = &record.primary_weight_path {
        let extension = Path::new(weight)
            .extension()
            .map(|extension| extension.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if !extension.is_empty() {
            return format!("detect       = {{ extension = \"{extension}\" }}");
        }
    }
    if let Some(architecture) = config_architecture(record) {
        return format!(
            "detect       = {{ file = \"config.json\", contains = \"{architecture}\" }}"
        );
    }
    "detect       = { file = \"config.json\" }".to_owned()
}

/// The first `architectures` entry in the model's `config.json`, if readable.
/// The whole array must be strings; a mixed array is rejected (falls back to a
/// bare `config.json` detect rule).
fn config_architecture(record: &ModelRecord) -> Option<String> {
    let paths = SidecarModelPaths::resolve(record);
    let data = kernel::fs::read_regular(&Path::new(&paths.snapshot).join("config.json")).ok()?;
    let json: serde_json::Value = serde_json::from_slice(&data).ok()?;
    let architectures: Vec<&str> = json
        .get("architectures")?
        .as_array()?
        .iter()
        .map(serde_json::Value::as_str)
        .collect::<Option<_>>()?;
    architectures
        .first()
        .map(|architecture| architecture.to_string())
}

#[cfg(test)]
mod template_tests {
    use super::*;
    use kernel::records::ModelSource;
    use kernel::resolution::ModelFormat;

    fn record(name: &str, modality: Modality) -> ModelRecord {
        ModelRecord::new(
            name,
            modality,
            Vec::new(),
            ModelSource::new(SourceKind::folder(), "/models/thing"),
        )
    }

    fn identified(format: ModelFormat, modality: Option<Modality>) -> IdentifiedModel {
        IdentifiedModel::new(format, modality, Vec::new(), ExecutionMode::Sync)
    }

    #[test]
    fn a_text_model_renders_a_streaming_chat_manifest() {
        let record = record("My Model", Modality::text());
        let manifest = render_template(&record, &identified(ModelFormat::Gguf, None));
        assert!(manifest.contains("id           = \"my-model\""));
        assert!(manifest.contains("modalities   = [\"text\"]"));
        assert!(manifest.contains("capabilities = [\"chat\", \"complete\"]"));
        assert!(manifest.contains("execution    = \"stream\""));
        assert!(manifest.contains("[permissions]"));
        // The commented one-shot invoke placeholders survive literally.
        assert!(manifest.contains("--model {model} --prompt {prompt}"));
    }

    #[test]
    fn an_audio_generator_renders_an_audio_job_manifest() {
        let record = record("YuE2-3B", Modality::audio());
        let mut identified = identified(ModelFormat::Safetensors, Some(Modality::audio()));
        identified.execution = ExecutionMode::Job;
        let manifest = render_template(&record, &identified);
        assert!(
            manifest.contains("capabilities = [\"audio\"]"),
            "{manifest}"
        );
        assert!(manifest.contains("execution    = \"job\""), "{manifest}");
    }

    #[test]
    fn identified_capabilities_win_over_the_modality_default() {
        let record = record("Stable Audio", Modality::audio());
        let identified = IdentifiedModel::new(
            ModelFormat::Diffusers,
            Some(Modality::audio()),
            vec![Capability::audio()],
            ExecutionMode::Job,
        );
        let manifest = render_template(&record, &identified);
        assert!(
            manifest.contains("capabilities = [\"audio\"]"),
            "{manifest}"
        );
        assert!(manifest.contains("execution    = \"job\""), "{manifest}");
    }

    #[test]
    fn an_image_model_renders_a_job_manifest() {
        let record = record("SDXL", Modality::image());
        let manifest = render_template(&record, &identified(ModelFormat::Diffusers, None));
        assert!(manifest.contains("execution    = \"job\""));
        assert!(manifest.contains("capabilities = [\"image\"]"));
    }

    #[test]
    fn the_detect_line_prefers_a_diffusers_pipeline_class() {
        let record = record("m", Modality::image());
        let mut identified = identified(ModelFormat::Diffusers, Some(Modality::image()));
        identified.pipeline_class = Some("FluxPipeline".to_owned());
        assert_eq!(
            detect_line(&record, &identified),
            "detect       = { pipeline = \"FluxPipeline\" }"
        );
    }

    #[test]
    fn the_detect_line_falls_back_to_a_weight_extension() {
        let mut record = record("m", Modality::text());
        record.primary_weight_path = Some("/models/thing/weights.gguf".to_owned());
        assert_eq!(
            detect_line(&record, &identified(ModelFormat::Gguf, None)),
            "detect       = { extension = \"gguf\" }"
        );
    }

    #[test]
    fn the_detect_line_falls_back_to_a_bare_config_check() {
        let record = record("m", Modality::text());
        assert_eq!(
            detect_line(&record, &identified(ModelFormat::Safetensors, None)),
            "detect       = { file = \"config.json\" }"
        );
    }

    /// A directory under the system temp dir, removed when dropped.
    struct Scratch(std::path::PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    /// A folder record over a fresh directory holding `file` with `contents`,
    /// with the directory.
    fn folder_with(name: &str, file: &str, contents: &str) -> (Scratch, ModelRecord) {
        let dir = std::env::temp_dir().join(format!(
            "hedos-manifest-detect-{name}-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(file), contents).unwrap();
        let mut record = record("m", Modality::audio());
        record.source.path = dir.to_string_lossy().into_owned();
        (Scratch(dir), record)
    }

    #[test]
    fn a_model_type_rule_reads_config_json_whatever_its_spacing() {
        let yue2 = ManifestDetect::ModelType("yue2".to_owned());
        let (_spaced, spaced) = folder_with(
            "spaced",
            "config.json",
            "{\n  \"model_type\" :\t\"yue2\"\n}",
        );
        assert!(detect_matches(&yue2, &spaced));
        let (_decoder, decoder) =
            folder_with("decoder", "config.json", r#"{"model_type":"yue2_vae"}"#);
        assert!(!detect_matches(&yue2, &decoder), "a prefix is not a match");
        let (_nested, nested) =
            folder_with("nested", "config.json", r#"{"text":{"model_type":"yue2"}}"#);
        assert!(
            !detect_matches(&yue2, &nested),
            "only the top-level key counts"
        );
    }

    #[test]
    fn a_pipeline_rule_reads_model_index_json() {
        let ace = ManifestDetect::Pipeline("AceStepPipeline".to_owned());
        let (_index, index) = folder_with(
            "ace",
            "model_index.json",
            r#"{"_class_name": "AceStepPipeline", "_diffusers_version": "0.38.0"}"#,
        );
        assert!(detect_matches(&ace, &index));
        let (_other, other) = folder_with(
            "ldm",
            "model_index.json",
            r#"{"_class_name":"AudioLDM2Pipeline"}"#,
        );
        assert!(!detect_matches(&ace, &other));
    }

    #[test]
    fn the_detect_line_prefers_a_config_model_type() {
        let (_dir, record) = folder_with(
            "template",
            "config.json",
            r#"{"model_type":"yue2","architectures":["YuE2ForCausalLM"]}"#,
        );
        assert_eq!(
            detect_line(&record, &identified(ModelFormat::Safetensors, None)),
            "detect       = { model_type = \"yue2\" }"
        );
    }

    #[test]
    fn a_stable_audio_snapshot_scaffolds_an_audio_job() {
        let (_dir, record) = folder_with(
            "stable-audio",
            "model_index.json",
            r#"{"_class_name":"StableAudioPipeline","_diffusers_version":"0.30.0"}"#,
        );
        let manifest = template_for(&record);
        assert!(
            manifest.contains("capabilities = [\"audio\"]"),
            "{manifest}"
        );
        assert!(manifest.contains("execution    = \"job\""), "{manifest}");
        assert!(
            manifest.contains("detect       = { pipeline = \"StableAudioPipeline\" }"),
            "{manifest}"
        );
    }

    #[test]
    fn config_architecture_requires_an_all_string_array() {
        let dir = std::env::temp_dir().join(format!("hedos-manifest-arch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut record = record("m", Modality::text());
        record.source.path = dir.to_string_lossy().into_owned();

        // A mixed array is rejected whole, even though the first element is a string.
        std::fs::write(
            dir.join("config.json"),
            r#"{"architectures": ["Foo", 123]}"#,
        )
        .unwrap();
        assert_eq!(config_architecture(&record), None);

        // An all-string array yields its first entry.
        std::fs::write(
            dir.join("config.json"),
            r#"{"architectures": ["Bar", "Baz"]}"#,
        )
        .unwrap();
        assert_eq!(config_architecture(&record), Some("Bar".to_owned()));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn default_capabilities_map_each_modality() {
        let stream = ExecutionMode::Stream;
        assert_eq!(
            default_capabilities(&Modality::image(), ExecutionMode::Job),
            vec![Capability::image()]
        );
        assert_eq!(
            default_capabilities(&Modality::speech(), stream),
            vec![Capability::speak()]
        );
        assert_eq!(
            default_capabilities(&Modality::audio(), stream),
            vec![Capability::transcribe()]
        );
        assert_eq!(
            default_capabilities(&Modality::audio(), ExecutionMode::Job),
            vec![Capability::audio()]
        );
        assert_eq!(
            default_capabilities(&Modality::text(), stream),
            vec![Capability::chat(), Capability::complete()]
        );
    }
}
