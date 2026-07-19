//! Locating a shipped Python runtime bundle and building the sidecar spec that
//! launches it. The launch runs the bundle's `python main.py` directly for
//! cross-platform support (a macOS `sandbox-exec` hardening layer is deferred),
//! keeping the same argument convention so a shipped bundle's `main.py` runs
//! unchanged.

use std::path::{Path, PathBuf};
use std::time::Duration;

use kernel::records::{ModelRecord, RuntimeId};

use super::model_paths::SidecarModelPaths;
use super::spec::SidecarSpec;
use crate::sidecar::SidecarError;

/// Locates a shipped runtime bundle (a directory holding `main.py`,
/// `requirements.lock`, etc.) by name under a set of search roots.
pub struct RuntimeBundle;

impl RuntimeBundle {
    /// The bundle directory named `name` under one of `search_roots` (each root
    /// is checked for `Runtimes/<name>` and `Resources/Runtimes/<name>`), or
    /// `None` if none exists — e.g. when no Python runtimes are installed yet.
    pub fn directory(name: &str, search_roots: &[PathBuf]) -> Option<PathBuf> {
        for root in search_roots {
            for suffix in ["Runtimes", "Resources/Runtimes"] {
                let candidate = root.join(suffix).join(name);
                if candidate.exists() {
                    return Some(candidate);
                }
            }
        }
        None
    }

    /// Like [`directory`](Self::directory) but returns a bundle-missing error
    /// naming `runtime_id` when the bundle isn't found.
    pub fn require(
        name: &str,
        search_roots: &[PathBuf],
        runtime_id: &RuntimeId,
    ) -> Result<PathBuf, SidecarError> {
        Self::directory(name, search_roots).ok_or_else(|| {
            SidecarError::RuntimeFailed(format!(
                "the {} runtime bundle is missing",
                runtime_id.as_str()
            ))
        })
    }
}

/// The per-model scratch directory a sidecar runs in.
pub struct SidecarWorkdir;

impl SidecarWorkdir {
    /// Create (if needed) and return `root/<name>`.
    pub fn directory(root: &Path, name: &str) -> std::io::Result<PathBuf> {
        let workdir = root.join(name);
        std::fs::create_dir_all(&workdir)?;
        Ok(workdir)
    }
}

/// Build the sidecar spec launching `bundle`'s `main.py` in a prepared
/// environment for `record`. The environment directory must have been prepared
/// (its `bin/python` is the interpreter). Arguments follow the shipped convention
/// `main.py --model <snapshot> [extra…] --workdir <workdir>`.
#[allow(clippy::too_many_arguments)]
pub fn spec(
    runtime_id: &RuntimeId,
    record: &ModelRecord,
    bundle: &Path,
    env_dir: Option<&Path>,
    workdir_root: &Path,
    workdir_name: &str,
    extra_arguments: &[String],
    cooperative_cancel: bool,
    cancel_grace_timeout: Duration,
) -> Result<SidecarSpec, SidecarError> {
    let Some(env_dir) = env_dir else {
        return Err(SidecarError::RuntimeFailed(format!(
            "the {} environment was not prepared",
            runtime_id.as_str()
        )));
    };
    let paths = SidecarModelPaths::resolve(record);
    let workdir = SidecarWorkdir::directory(workdir_root, workdir_name)
        .map_err(|error| SidecarError::RuntimeFailed(error.to_string()))?;

    let python = env_dir.join("bin").join("python");
    let mut arguments = vec![
        path_string(&bundle.join("main.py")),
        "--model".to_owned(),
        paths.snapshot,
    ];
    arguments.extend(extra_arguments.iter().cloned());
    arguments.push("--workdir".to_owned());
    arguments.push(path_string(&workdir));

    let mut spec = SidecarSpec::new(
        format!("{}#{}", runtime_id.as_str(), record.id),
        python,
        arguments,
    );
    spec.environment
        .insert("PYTHONDONTWRITEBYTECODE".to_owned(), "1".to_owned());
    spec.working_directory = Some(workdir);
    spec.ready_timeout = Duration::from_secs(600);
    spec.cooperative_cancel = cooperative_cancel;
    spec.cancel_grace_timeout = cancel_grace_timeout;
    Ok(spec)
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel::records::{Modality, ModelSource, SourceKind};

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hedos-sidecar-bundle-{name}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn record(path: &str) -> ModelRecord {
        ModelRecord::new(
            "m",
            Modality::text(),
            Vec::new(),
            ModelSource::new(SourceKind::folder(), path),
        )
    }

    #[test]
    fn a_missing_bundle_directory_is_none_then_an_error() {
        let root = temp_dir("locate");
        assert!(RuntimeBundle::directory("python-mlx-lm", std::slice::from_ref(&root)).is_none());
        let error = RuntimeBundle::require(
            "python-mlx-lm",
            std::slice::from_ref(&root),
            &RuntimeId::mlx_lm(),
        )
        .unwrap_err();
        assert!(
            matches!(error, SidecarError::RuntimeFailed(message) if message.contains("missing"))
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_bundle_is_found_under_the_runtimes_subdir() {
        let root = temp_dir("found");
        let bundle = root.join("Runtimes").join("python-mlx-lm");
        std::fs::create_dir_all(&bundle).unwrap();
        assert_eq!(
            RuntimeBundle::directory("python-mlx-lm", std::slice::from_ref(&root)),
            Some(bundle)
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_spec_without_a_prepared_environment_errors() {
        let root = temp_dir("no-env");
        let error = spec(
            &RuntimeId::mlx_lm(),
            &record("/models/m"),
            &root,
            None,
            &root,
            "python-mlx-lm",
            &[],
            true,
            Duration::from_secs(10),
        )
        .unwrap_err();
        assert!(
            matches!(error, SidecarError::RuntimeFailed(message) if message.contains("not prepared"))
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_spec_launches_the_bundles_python_main_with_the_model() {
        let root = temp_dir("spec");
        let bundle = root.join("bundle");
        let env_dir = root.join("env");
        let workdir_root = root.join("workdirs");
        std::fs::create_dir_all(&bundle).unwrap();
        std::fs::create_dir_all(&env_dir).unwrap();

        let spec = spec(
            &RuntimeId::mlx_lm(),
            &record("/models/m"),
            &bundle,
            Some(&env_dir),
            &workdir_root,
            "python-mlx-lm",
            &["--extra".to_owned()],
            true,
            Duration::from_secs(10),
        )
        .unwrap();

        assert!(spec.executable.ends_with("bin/python"));
        assert_eq!(spec.arguments[0], path_string(&bundle.join("main.py")));
        assert!(spec.arguments.contains(&"--model".to_owned()));
        assert!(spec.arguments.contains(&"--extra".to_owned()));
        assert!(spec.arguments.contains(&"--workdir".to_owned()));
        assert_eq!(
            spec.environment
                .get("PYTHONDONTWRITEBYTECODE")
                .map(String::as_str),
            Some("1")
        );
        assert!(spec.cooperative_cancel);
        assert_eq!(spec.ready_timeout, Duration::from_secs(600));
        assert!(spec.runtime_id.contains('#')); // "python:mlx-lm#<record id>"
        // The workdir was created.
        assert!(workdir_root.join("python-mlx-lm").exists());
        std::fs::remove_dir_all(&root).ok();
    }
}
