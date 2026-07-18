//! Support logic for manifest-declared runtimes: resolving a model's on-disk
//! paths, substituting the `{model}`/`{prompt}`/`{workdir}`/`{outputs}`/`{python}`
//! placeholders in a manifest command, and the small string/JSON helpers the
//! manifest adapters share.
//!
//! The sandbox-profile assembly, the VM-guest command form, and the workdir
//! bundle helpers from the Swift original are Apple/sandbox-specific and are
//! deferred with the `ManifestSidecarAdapter`/`VMCommandAdapter` units; this
//! module is the pure, cross-platform core those adapters build on.

use std::collections::BTreeMap;
use std::path::Path;

use kernel::records::{JsonValue, ModelRecord, SourceKind};

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
    /// the path are resolved when possible; unlike the Swift original a leading
    /// `~` is not expanded (discovery passes absolute paths).
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

/// Whether `ch` is a line separator, matching Swift's `Character.isNewline` — not
/// just `\n`, since sidecars emit `\r`-based progress bars (tqdm, download bars)
/// whose final segment is the real error.
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
/// prompt, workdir, outputs, and (when an environment is prepared) python
/// placeholders. Errors if the command uses `{python}` without an environment or
/// resolves to no tokens.
pub fn substituted(
    command: &str,
    record: &ModelRecord,
    payload: &JsonValue,
    workdir: &Path,
    outputs: &Path,
    env_dir: Option<&Path>,
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
    // Swift `split(separator: " ")` drops empty subsequences (runs of spaces).
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
