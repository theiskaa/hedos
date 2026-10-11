//! The `RuntimeManifest` value model and its validation.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

use crate::records::{Capability, ExecutionMode, JsonValue, Modality, ParamSpec, ParamType};

use super::provenance::RuntimeProvenance;

/// A manifest that failed validation. Its message is available via `Display`.
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
#[error("{0}")]
pub struct ManifestValidationError(String);

fn invalid(message: impl Into<String>) -> ManifestValidationError {
    ManifestValidationError(message.into())
}

/// How a runtime is auto-detected for a model: exactly one of these rules.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ManifestDetect {
    /// A marker file beside the model, holding `contains` when one is given.
    File {
        path: String,
        contains: Option<String>,
    },
    /// The extension of the model's weight file.
    Extension(String),
    /// The `model_type` the model's `config.json` names, read as JSON so its
    /// spacing does not matter.
    ModelType(String),
    /// The pipeline class the model's `model_index.json` names, read as JSON.
    Pipeline(String),
}

/// Another repository a model needs beside its own to run (YuE2's decoder),
/// passed to the runtime under its role.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ManifestCompanion {
    /// What it is to the runtime (`vae`).
    pub role: String,
    /// The Hugging Face repo it comes from (`m-a-p/YuE2-Vae`).
    pub repo: String,
}

/// The most companions one manifest may name.
const MAX_COMPANIONS: usize = 4;

/// The Python environment a runtime needs.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ManifestEnv {
    pub manager: String,
    pub python: String,
    pub lockfile: String,
}

/// A long-running sidecar entrypoint.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ManifestServe {
    pub entrypoint: String,
    pub wire_protocol: String,
}

/// A one-shot command template.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ManifestInvoke {
    pub command: String,
    /// What the command reads on stdin; it gets nothing when absent.
    pub stdin: Option<InvokeStdin>,
}

/// What a one-shot command reads on stdin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InvokeStdin {
    /// The invoke payload as compact JSON, then end of input. A request read
    /// this way never shows in the process's arguments, which every user of
    /// the machine can list.
    Payload,
}

/// A command the runtime downloads rather than finding on the machine: the
/// release it belongs to, and per platform the archive to fetch, the sha256
/// it must match, and where the binary sits inside it. The command names the
/// installed binary as `{bin}`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ManifestInstall {
    pub version: String,
    /// Keyed by Rust target triple (`aarch64-apple-darwin`).
    pub targets: BTreeMap<String, InstallTarget>,
}

/// One platform's build of an installed command.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct InstallTarget {
    /// An `https` URL of a `.tar.gz` archive.
    pub url: String,
    /// The archive's sha256, lowercase hex.
    pub sha256: String,
    /// The binary's path inside the archive.
    pub binary: String,
}

/// What the runtime is allowed to touch.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ManifestPermissions {
    pub network: bool,
    pub paths: Vec<String>,
}

/// A digest-pinned VM image the runtime runs inside.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ManifestVm {
    pub image: String,
    pub setup: Vec<String>,
}

/// A validated community runtime manifest.
#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeManifest {
    pub id: String,
    pub modalities: Vec<Modality>,
    pub capabilities: Vec<Capability>,
    pub execution: ExecutionMode,
    pub alternatives: Vec<String>,
    pub detect: Option<ManifestDetect>,
    pub env: Option<ManifestEnv>,
    pub serve: Option<ManifestServe>,
    pub invoke: Option<ManifestInvoke>,
    pub install: Option<ManifestInstall>,
    pub permissions: ManifestPermissions,
    pub vm: Option<ManifestVm>,
    /// What a request to the runtime may carry, lent to a model it serves
    /// when identification found no schema of its own.
    pub params: Vec<ParamSpec>,
    /// Other repositories its models need, sorted by role.
    pub companions: Vec<ManifestCompanion>,
    pub directory: Option<PathBuf>,
    pub provenance: Option<RuntimeProvenance>,
    pub content_hash: Option<String>,
}

impl RuntimeManifest {
    /// Parse and validate a `manifest.toml`, recording `directory` as where it
    /// was loaded from.
    pub fn parse(text: &str, directory: Option<PathBuf>) -> Result<Self, ManifestValidationError> {
        let raw: RawManifest = toml::from_str(text)
            .map_err(|err| invalid(format!("manifest is not valid TOML: {err}")))?;
        raw.validate(directory)
    }
}

#[derive(Deserialize)]
struct RawManifest {
    id: Option<String>,
    #[serde(default)]
    modalities: Vec<String>,
    #[serde(default)]
    capabilities: Vec<String>,
    execution: Option<String>,
    #[serde(default)]
    alternatives: Vec<String>,
    detect: Option<RawDetect>,
    env: Option<RawEnv>,
    serve: Option<RawServe>,
    invoke: Option<RawInvoke>,
    install: Option<RawInstall>,
    permissions: Option<RawPermissions>,
    vm: Option<RawVm>,
    #[serde(default)]
    params: Vec<RawParam>,
    #[serde(default)]
    companions: BTreeMap<String, RawCompanion>,
}

#[derive(Deserialize)]
struct RawDetect {
    file: Option<String>,
    contains: Option<String>,
    extension: Option<String>,
    model_type: Option<String>,
    pipeline: Option<String>,
}

#[derive(Deserialize)]
struct RawParam {
    key: Option<String>,
    #[serde(rename = "type")]
    param_type: Option<String>,
    default: Option<toml::Value>,
    range: Option<Vec<toml::Value>>,
    values: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct RawCompanion {
    repo: Option<String>,
}

#[derive(Deserialize)]
struct RawEnv {
    manager: Option<String>,
    python: Option<String>,
    lockfile: Option<String>,
}

#[derive(Deserialize)]
struct RawServe {
    entrypoint: Option<String>,
    protocol: Option<String>,
}

#[derive(Deserialize)]
struct RawInvoke {
    command: Option<String>,
    stdin: Option<String>,
}

#[derive(Deserialize)]
struct RawInstall {
    version: Option<String>,
    #[serde(default)]
    targets: BTreeMap<String, RawTarget>,
}

#[derive(Deserialize)]
struct RawTarget {
    url: Option<String>,
    sha256: Option<String>,
    binary: Option<String>,
}

#[derive(Deserialize, Default)]
struct RawPermissions {
    network: Option<bool>,
    paths: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct RawVm {
    image: Option<String>,
    #[serde(default)]
    setup: Vec<String>,
}

impl RawManifest {
    fn validate(
        self,
        directory: Option<PathBuf>,
    ) -> Result<RuntimeManifest, ManifestValidationError> {
        let id = self
            .id
            .filter(|id| !id.is_empty())
            .ok_or_else(|| invalid("manifest is missing an id"))?;
        validate_id(&id)?;

        let modalities: Vec<Modality> = self
            .modalities
            .iter()
            .map(|m| Modality::from(m.as_str()))
            .collect();
        let capabilities: Vec<Capability> = self
            .capabilities
            .iter()
            .map(|c| Capability::from(c.as_str()))
            .collect();
        if capabilities.is_empty() {
            return Err(invalid(format!("manifest {id} declares no capabilities")));
        }
        if capabilities.contains(&Capability::speak())
            && capabilities.contains(&Capability::audio())
        {
            return Err(invalid(format!(
                "manifest {id} declares both speak and audio, but a model is either a voice \
                 reading text or a generator making audio from a description"
            )));
        }

        let execution_raw = self
            .execution
            .ok_or_else(|| invalid(format!("manifest {id} is missing an execution mode")))?;
        let execution = parse_execution(&execution_raw)
            .ok_or_else(|| invalid(format!("manifest {id} has an unknown execution mode")))?;

        let detect = match self.detect {
            Some(raw) => Some(validate_detect(&id, raw)?),
            None => None,
        };

        let env = match self.env {
            Some(raw) => {
                let lockfile = raw.lockfile.ok_or_else(|| {
                    invalid(format!("manifest {id} declares [env] without a lockfile"))
                })?;
                Some(ManifestEnv {
                    manager: raw.manager.unwrap_or_else(|| "uv".to_owned()),
                    python: raw.python.unwrap_or_else(|| "3.12".to_owned()),
                    lockfile,
                })
            }
            None => None,
        };

        let serve = match self.serve {
            Some(raw) => {
                let entrypoint = raw.entrypoint.ok_or_else(|| {
                    invalid(format!(
                        "manifest {id} declares [serve] without an entrypoint"
                    ))
                })?;
                Some(ManifestServe {
                    entrypoint,
                    wire_protocol: raw.protocol.unwrap_or_else(|| "ndjson+frames".to_owned()),
                })
            }
            None => None,
        };

        let invoke = match self.invoke {
            Some(raw) => {
                let command = raw
                    .command
                    .filter(|command| !command.is_empty())
                    .ok_or_else(|| {
                        invalid(format!("manifest {id} declares [invoke] without a command"))
                    })?;
                let stdin = match raw.stdin.as_deref() {
                    None => None,
                    Some("{payload}") => Some(InvokeStdin::Payload),
                    Some(other) => {
                        return Err(invalid(format!(
                            "manifest {id} [invoke] stdin \"{other}\" is not \"{{payload}}\", the one thing a command can read"
                        )));
                    }
                };
                Some(ManifestInvoke { command, stdin })
            }
            None => None,
        };

        let install = match self.install {
            Some(raw) => Some(validate_install(&id, raw)?),
            None => None,
        };
        let names_bin = invoke
            .as_ref()
            .is_some_and(|invoke| invoke.command.contains("{bin}"));
        match (&install, names_bin) {
            (Some(_), false) => {
                return Err(invalid(format!(
                    "manifest {id} declares [install] but its [invoke] command does not run {{bin}}"
                )));
            }
            (None, true) => {
                return Err(invalid(format!(
                    "manifest {id} runs {{bin}} but declares no [install] to provide it"
                )));
            }
            _ => {}
        }

        if serve.is_some() && invoke.is_some() {
            return Err(invalid(format!(
                "manifest {id} declares both [serve] and [invoke]"
            )));
        }
        if serve.is_none() && invoke.is_none() {
            return Err(invalid(format!(
                "manifest {id} declares neither [serve] nor [invoke]"
            )));
        }
        if invoke.is_some() && execution == ExecutionMode::Stream {
            return Err(invalid(
                "invoke manifests run to completion: declare sync (or job), or use [serve] to stream",
            ));
        }

        let serves_job = capabilities.iter().any(Capability::generates);
        if (execution == ExecutionMode::Job) != serves_job {
            return Err(invalid(format!(
                "manifest {id} execution \"{execution_raw}\" does not match its capabilities: \
                 \"job\" is for exactly the manifests that generate (image or audio)"
            )));
        }

        let raw_permissions = self.permissions.unwrap_or_default();
        let permissions = ManifestPermissions {
            network: raw_permissions.network.unwrap_or(false),
            paths: raw_permissions
                .paths
                .unwrap_or_else(|| vec!["{model}".to_owned(), "{workdir}".to_owned()]),
        };

        let vm = match self.vm {
            Some(raw) => {
                let image = raw.image.filter(|image| !image.is_empty()).ok_or_else(|| {
                    invalid(format!("manifest {id} declares [vm] without an image"))
                })?;
                if !image.contains("@sha256:") {
                    return Err(invalid(format!(
                        "manifest {id} [vm] image must be digest-pinned (…@sha256:…), since tags can move"
                    )));
                }
                if serve.is_some() {
                    return Err(invalid(format!(
                        "manifest {id} [vm] runtimes support [invoke] only"
                    )));
                }
                if env.is_some() {
                    return Err(invalid(format!(
                        "manifest {id} declares both [vm] and [env], but the image and its setup are the environment"
                    )));
                }
                if install.is_some() {
                    return Err(invalid(format!(
                        "manifest {id} declares both [vm] and [install], but the image holds what it runs"
                    )));
                }
                if permissions.network {
                    return Err(invalid(
                        "vm runtimes always run offline: remove permissions.network",
                    ));
                }
                Some(ManifestVm {
                    image,
                    setup: raw.setup,
                })
            }
            None => None,
        };

        let params = validate_params(&id, self.params)?;
        let companions = validate_companions(&id, self.companions)?;

        Ok(RuntimeManifest {
            id,
            modalities,
            capabilities,
            execution,
            alternatives: self.alternatives,
            detect,
            env,
            serve,
            invoke,
            install,
            permissions,
            vm,
            params,
            companions,
            directory,
            provenance: None,
            content_hash: None,
        })
    }
}

/// `raw` checked: exactly one of a marker file (with an optional string it must
/// hold), a weight extension, a `model_type` or a pipeline class.
fn validate_detect(id: &str, raw: RawDetect) -> Result<ManifestDetect, ManifestValidationError> {
    let empty = [&raw.file, &raw.extension, &raw.model_type, &raw.pipeline]
        .into_iter()
        .any(|value| value.as_deref() == Some(""));
    if empty {
        return Err(invalid(format!("manifest {id} has an empty detect value")));
    }
    if raw.contains.is_some() && raw.file.is_none() {
        return Err(invalid(format!(
            "manifest {id} has a detect rule with contains but no file to look in"
        )));
    }
    match (raw.file, raw.extension, raw.model_type, raw.pipeline) {
        (Some(path), None, None, None) => Ok(ManifestDetect::File {
            path,
            contains: raw.contains,
        }),
        (None, Some(extension), None, None) => Ok(ManifestDetect::Extension(extension)),
        (None, None, Some(model_type), None) => Ok(ManifestDetect::ModelType(model_type)),
        (None, None, None, Some(pipeline)) => Ok(ManifestDetect::Pipeline(pipeline)),
        (None, None, None, None) => Err(invalid(format!(
            "manifest {id} has a detect rule with no file, extension, model_type or pipeline"
        ))),
        _ => Err(invalid(format!(
            "manifest {id} has a detect rule with more than one of file, extension, model_type \
             and pipeline; use one"
        ))),
    }
}

/// Each `[[params]]` entry checked and turned into the schema a record carries:
/// a unique lowercase key, a known type, enum values only on an enum, a
/// two-number range only on a number, and a default of the declared type.
fn validate_params(
    id: &str,
    raw: Vec<RawParam>,
) -> Result<Vec<ParamSpec>, ManifestValidationError> {
    let mut specs: Vec<ParamSpec> = Vec::new();
    for param in raw {
        let key = param
            .key
            .filter(|key| {
                !key.is_empty()
                    && key
                        .chars()
                        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
            })
            .ok_or_else(|| {
                invalid(format!(
                    "manifest {id} declares a param without a lowercase key"
                ))
            })?;
        if specs.iter().any(|spec| spec.key == key) {
            return Err(invalid(format!("manifest {id} declares param {key} twice")));
        }
        let param_type = match param.param_type.as_deref() {
            Some("int") => ParamType::Int,
            Some("float") => ParamType::Float,
            Some("bool") => ParamType::Bool,
            Some("string") => ParamType::String,
            Some("enum") => ParamType::Enum,
            _ => {
                return Err(invalid(format!(
                    "manifest {id} param {key} needs a type: int, float, bool, string or enum"
                )));
            }
        };
        let values = match (param_type, param.values) {
            (ParamType::Enum, Some(values)) if !values.is_empty() => Some(values),
            (ParamType::Enum, _) => {
                return Err(invalid(format!(
                    "manifest {id} param {key} is an enum without values"
                )));
            }
            (_, Some(_)) => {
                return Err(invalid(format!(
                    "manifest {id} param {key} has values but is not an enum"
                )));
            }
            (_, None) => None,
        };
        let range = match param.range {
            None => None,
            Some(bounds) => {
                let bounds = match (param_type, bounds.as_slice()) {
                    (ParamType::Int | ParamType::Float, [low, high]) => {
                        let low = number(param_type, low);
                        let high = number(param_type, high);
                        match (low, high) {
                            (Some(low), Some(high)) if ordered(&low, &high) => {
                                vec![low, high]
                            }
                            _ => {
                                return Err(invalid(format!(
                                    "manifest {id} param {key} range must be [min, max] of its type"
                                )));
                            }
                        }
                    }
                    _ => {
                        return Err(invalid(format!(
                            "manifest {id} param {key} range must be [min, max] on an int or float"
                        )));
                    }
                };
                Some(bounds)
            }
        };
        let default_value = match param.default {
            None => None,
            Some(value) => {
                let converted = match (param_type, value) {
                    (ParamType::Int | ParamType::Float, value) => number(param_type, &value),
                    (ParamType::Bool, toml::Value::Boolean(value)) => Some(JsonValue::Bool(value)),
                    (ParamType::String, toml::Value::String(value)) => {
                        Some(JsonValue::String(value))
                    }
                    (ParamType::Enum, toml::Value::String(value))
                        if values
                            .as_ref()
                            .is_some_and(|values| values.contains(&value)) =>
                    {
                        Some(JsonValue::String(value))
                    }
                    _ => None,
                };
                Some(converted.ok_or_else(|| {
                    invalid(format!(
                        "manifest {id} param {key} has a default that is not of its type"
                    ))
                })?)
            }
        };
        if let (Some(default), Some([low, high])) = (&default_value, range.as_deref())
            && !(ordered(low, default) && ordered(default, high))
        {
            return Err(invalid(format!(
                "manifest {id} param {key} has a default outside its range"
            )));
        }
        specs.push(ParamSpec {
            key,
            param_type,
            default_value,
            range,
            values,
        });
    }
    Ok(specs)
}

/// A TOML number as the JSON value a param of `param_type` holds: an integer
/// for an int, a double for a float (an integer is widened).
fn number(param_type: ParamType, value: &toml::Value) -> Option<JsonValue> {
    match (param_type, value) {
        (ParamType::Int, toml::Value::Integer(value)) => Some(JsonValue::Int(*value)),
        (ParamType::Float, toml::Value::Integer(value)) => Some(JsonValue::Double(*value as f64)),
        (ParamType::Float, toml::Value::Float(value)) if value.is_finite() => {
            Some(JsonValue::Double(*value))
        }
        _ => None,
    }
}

/// Whether number `low` is at most `high`: integers compared exactly, floats
/// as floats.
fn ordered(low: &JsonValue, high: &JsonValue) -> bool {
    match (low, high) {
        (JsonValue::Int(low), JsonValue::Int(high)) => low <= high,
        (JsonValue::Double(low), JsonValue::Double(high)) => low <= high,
        _ => false,
    }
}

/// Each `[companions]` entry checked: a lowercase role, and a repo written
/// `owner/name` in the characters Hugging Face allows. At most
/// [`MAX_COMPANIONS`].
fn validate_companions(
    id: &str,
    raw: BTreeMap<String, RawCompanion>,
) -> Result<Vec<ManifestCompanion>, ManifestValidationError> {
    if raw.len() > MAX_COMPANIONS {
        return Err(invalid(format!(
            "manifest {id} names more than {MAX_COMPANIONS} companions"
        )));
    }
    raw.into_iter()
        .map(|(role, companion)| {
            let role_ok = !role.is_empty()
                && role
                    .chars()
                    .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_');
            if !role_ok {
                return Err(invalid(format!(
                    "manifest {id} companion role {role:?} must be lowercase letters, digits or _"
                )));
            }
            let repo = companion
                .repo
                .filter(|repo| is_repo_id(repo))
                .ok_or_else(|| {
                    invalid(format!(
                        "manifest {id} companion {role} needs a repo written owner/name"
                    ))
                })?;
            Ok(ManifestCompanion { role, repo })
        })
        .collect()
}

/// Whether `repo` reads as a Hugging Face repo id: `owner/name`, each part made
/// of letters, digits, `-`, `_` and `.`, and neither starting with a dot.
fn is_repo_id(repo: &str) -> bool {
    let mut parts = repo.split('/');
    let (Some(owner), Some(name), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    [owner, name].iter().all(|part| {
        !part.is_empty()
            && !part.starts_with('.')
            && part
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    })
}

/// `raw` checked: a version, at least one target, and for each an `https`
/// archive, a sha256 of 64 lowercase hex digits, and a binary path inside the
/// archive that cannot climb out of it.
fn validate_install(id: &str, raw: RawInstall) -> Result<ManifestInstall, ManifestValidationError> {
    let version = raw
        .version
        .filter(|version| !version.is_empty())
        .ok_or_else(|| {
            invalid(format!(
                "manifest {id} declares [install] without a version"
            ))
        })?;
    if raw.targets.is_empty() {
        return Err(invalid(format!(
            "manifest {id} declares [install] without any targets"
        )));
    }
    let mut targets = BTreeMap::new();
    for (triple, target) in raw.targets {
        let missing = |field: &str| {
            invalid(format!(
                "manifest {id} [install] target {triple} is missing its {field}"
            ))
        };
        let url = target.url.ok_or_else(|| missing("url"))?;
        let sha256 = target.sha256.ok_or_else(|| missing("sha256"))?;
        let binary = target.binary.ok_or_else(|| missing("binary"))?;
        if !url.starts_with("https://") {
            return Err(invalid(format!(
                "manifest {id} [install] target {triple} must be fetched over https"
            )));
        }
        if sha256.len() != 64
            || !sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(invalid(format!(
                "manifest {id} [install] target {triple} sha256 is not 64 lowercase hex digits"
            )));
        }
        let inside = Path::new(&binary)
            .components()
            .all(|part| matches!(part, Component::Normal(_)));
        if binary.is_empty() || !inside {
            return Err(invalid(format!(
                "manifest {id} [install] target {triple} binary must be a path inside the archive"
            )));
        }
        targets.insert(
            triple,
            InstallTarget {
                url,
                sha256,
                binary,
            },
        );
    }
    Ok(ManifestInstall { version, targets })
}

fn parse_execution(raw: &str) -> Option<ExecutionMode> {
    match raw {
        "stream" => Some(ExecutionMode::Stream),
        "job" => Some(ExecutionMode::Job),
        "sync" => Some(ExecutionMode::Sync),
        _ => None,
    }
}

fn validate_id(id: &str) -> Result<(), ManifestValidationError> {
    let allowed = |c: char| c.is_ascii() && (c.is_ascii_alphanumeric() || "._:-".contains(c));
    let has_alnum = id.chars().any(|c| c.is_ascii_alphanumeric());
    let not_all_dots = id.chars().any(|c| c != '.');
    if id.chars().all(allowed) && has_alnum && not_all_dots {
        Ok(())
    } else {
        Err(invalid(
            "manifest id may only contain letters, digits, dots, underscores, colons, and hyphens",
        ))
    }
}
