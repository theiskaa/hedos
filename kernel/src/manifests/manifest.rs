//! The `RuntimeManifest` value model and its validation.

use std::path::PathBuf;

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
                Some(ManifestInvoke { command })
            }
            None => None,
        };

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
                "invoke manifests run to completion — declare sync (or job), or use [serve] to stream",
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
                        "manifest {id} [vm] image must be digest-pinned (…@sha256:…) — tags can move"
                    )));
                }
                if serve.is_some() {
                    return Err(invalid(format!(
                        "manifest {id} [vm] runtimes support [invoke] only"
                    )));
                }
                if env.is_some() {
                    return Err(invalid(format!(
                        "manifest {id} declares both [vm] and [env] — the image and its setup are the environment"
                    )));
                }
                if permissions.network {
                    return Err(invalid(
                        "vm runtimes always run offline — remove permissions.network",
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
            permissions,
            vm,
            directory,
            provenance: None,
            content_hash: None,
        })
    }
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
