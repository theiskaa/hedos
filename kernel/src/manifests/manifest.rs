//! The `RuntimeManifest` value model and its validation.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

use crate::records::{Capability, ExecutionMode, Modality};

use super::provenance::RuntimeProvenance;

/// A manifest that failed validation. Its message is available via `Display`.
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
#[error("{0}")]
pub struct ManifestValidationError(String);

fn invalid(message: impl Into<String>) -> ManifestValidationError {
    ManifestValidationError(message.into())
}

/// How a runtime is auto-detected for a model: a weight-file extension, or a
/// marker file (optionally containing a string) beside the model.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ManifestDetect {
    pub file: Option<String>,
    pub contains: Option<String>,
    pub file_extension: Option<String>,
}

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
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
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
}

#[derive(Deserialize)]
struct RawDetect {
    file: Option<String>,
    contains: Option<String>,
    extension: Option<String>,
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

        let execution_raw = self
            .execution
            .ok_or_else(|| invalid(format!("manifest {id} is missing an execution mode")))?;
        let execution = parse_execution(&execution_raw)
            .ok_or_else(|| invalid(format!("manifest {id} has an unknown execution mode")))?;

        let detect = match self.detect {
            Some(raw) => {
                let detect = ManifestDetect {
                    file: raw.file,
                    contains: raw.contains,
                    file_extension: raw.extension,
                };
                if detect.file.is_none() && detect.file_extension.is_none() {
                    return Err(invalid(format!(
                        "manifest {id} has a detect rule with no file or extension"
                    )));
                }
                Some(detect)
            }
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

        let serves_job = capabilities.contains(&Capability::image());
        if (execution == ExecutionMode::Job) != serves_job {
            return Err(invalid(format!(
                "manifest {id} execution \"{execution_raw}\" does not match its capabilities"
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
            directory,
            provenance: None,
            content_hash: None,
        })
    }
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
