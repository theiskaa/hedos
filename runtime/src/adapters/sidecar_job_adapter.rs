//! The shared machinery behind the Python-sidecar *job* adapters (image
//! generation). Analogous to [`SidecarAdapter`](super::sidecar_adapter::SidecarAdapter)
//! but for the [`JobRunning`](super::JobRunning) path: it opens a job stream
//! rather than a capability stream, launches with a per-record `--name` argument,
//! and hard-cancels with a longer grace. Each job adapter differs only in a
//! [`SidecarJobSpec`] and its `bid`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use kernel::records::{JsonValue, ModelRecord, RuntimeId};

use super::JobStream;
use super::sidecar_adapter::prepare_environment;
use super::sidecar_stream::bridge;
use crate::environment::EnvironmentManager;
use crate::governor::MemoryGovernor;
use crate::python_runtime::{Descriptor, PythonSidecarRuntime};
use crate::sidecar::{RuntimeBundle, SidecarSupervisor, bundle_spec};

/// How long to wait for a cancelled image job to wind down before killing it.
/// Image runtimes need longer than the streaming default because a diffusion step
/// in flight cannot be interrupted mid-kernel.
const JOB_CANCEL_GRACE: Duration = Duration::from_secs(60);

/// The per-runtime constants that distinguish one image-job adapter from another.
pub(crate) struct SidecarJobSpec {
    /// The runtime id (and bundle key).
    pub id: RuntimeId,
    /// The shipped bundle directory name.
    pub bundle_name: &'static str,
    /// The status shown while preparing the environment.
    pub preparing_status: &'static str,
    /// The status shown while the sidecar starts.
    pub starting_status: &'static str,
    /// How long a loaded model stays warm after its last use, if bounded.
    pub warm_window: Option<Duration>,
}

/// The shared plumbing every image-job adapter wraps: it builds the governed
/// runtime and its descriptor, and bridges a job stream out.
pub(crate) struct SidecarJobAdapter {
    spec: SidecarJobSpec,
    governor: MemoryGovernor,
    supervisor: SidecarSupervisor,
    environments: EnvironmentManager,
    search_roots: Arc<Vec<PathBuf>>,
    workdir_root: PathBuf,
}

impl SidecarJobAdapter {
    /// Wrap the shared subsystems for an image runtime described by `spec`.
    pub(crate) fn new(
        spec: SidecarJobSpec,
        governor: MemoryGovernor,
        supervisor: SidecarSupervisor,
        environments: EnvironmentManager,
        search_roots: Vec<PathBuf>,
        workdir_root: PathBuf,
    ) -> Self {
        Self {
            spec,
            governor,
            supervisor,
            environments,
            search_roots: Arc::new(search_roots),
            workdir_root,
        }
    }

    /// The runtime id this adapter serves.
    pub(crate) fn id(&self) -> &RuntimeId {
        &self.spec.id
    }

    /// Run `op` for `record` as a job, bridging the sidecar job stream out.
    pub(crate) fn job(&self, record: &ModelRecord, op: &str, payload: JsonValue) -> JobStream {
        bridge(self.runtime().job(record, op, payload))
    }

    fn runtime(&self) -> PythonSidecarRuntime {
        PythonSidecarRuntime::new(
            job_descriptor(
                &self.spec,
                self.environments.clone(),
                Arc::clone(&self.search_roots),
                self.workdir_root.clone(),
            ),
            self.governor.clone(),
            self.supervisor.clone(),
        )
    }
}

/// Build the sidecar [`Descriptor`] for an image-job `spec`: it locates the bundle
/// under `search_roots`, prepares the environment, and builds a launch spec that
/// passes the model name through (`--name`) and hard-cancels with the longer image
/// grace.
pub(crate) fn job_descriptor(
    spec: &SidecarJobSpec,
    environments: EnvironmentManager,
    search_roots: Arc<Vec<PathBuf>>,
    workdir_root: PathBuf,
) -> Descriptor {
    let bundle_name = spec.bundle_name;
    let spec_id = spec.id.clone();
    Descriptor {
        runtime_id: spec.id.as_str().to_owned(),
        preparing_status: spec.preparing_status.to_owned(),
        starting_status: spec.starting_status.to_owned(),
        warm_window: spec.warm_window,
        prepare_environment: prepare_environment(
            bundle_name,
            spec.id.clone(),
            environments,
            Arc::clone(&search_roots),
        ),
        make_spec: Arc::new(move |record: &ModelRecord, env_dir: Option<&Path>| {
            let bundle = RuntimeBundle::require(bundle_name, &search_roots, &spec_id)?;
            let extra = [String::from("--name"), record.name.clone()];
            bundle_spec(
                &spec_id,
                record,
                &bundle,
                env_dir,
                &workdir_root,
                bundle_name,
                &extra,
                false,
                JOB_CANCEL_GRACE,
            )
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel::records::{Modality, ModelSource, SourceKind};

    fn record() -> ModelRecord {
        ModelRecord::new(
            "flux-model",
            Modality::image(),
            Vec::new(),
            ModelSource::new(SourceKind::huggingface_cache(), "/models/m"),
        )
    }

    #[test]
    fn the_launch_spec_passes_the_model_name_and_hard_cancels_with_the_image_grace() {
        let root = std::env::temp_dir().join("hedos-sidecar-job-adapter");
        let bundle = root.join("Runtimes").join("python-mflux");
        let env_dir = root.join("env");
        std::fs::create_dir_all(&bundle).unwrap();
        std::fs::create_dir_all(&env_dir).unwrap();

        let descriptor = job_descriptor(
            &SidecarJobSpec {
                id: RuntimeId::mflux(),
                bundle_name: "python-mflux",
                preparing_status: "Preparing…",
                starting_status: "Starting…",
                warm_window: Some(Duration::from_secs(60)),
            },
            EnvironmentManager::new(env_dir.clone()),
            Arc::new(vec![root.clone()]),
            root.join("workdirs"),
        );
        let launch = (descriptor.make_spec)(&record(), Some(&env_dir)).unwrap();

        assert!(!launch.cooperative_cancel);
        assert_eq!(launch.cancel_grace_timeout, JOB_CANCEL_GRACE);
        assert!(
            launch
                .arguments
                .windows(2)
                .any(|w| w[0] == "--name" && w[1] == "flux-model"),
            "expected --name flux-model in {:?}",
            launch.arguments
        );
        std::fs::remove_dir_all(&root).ok();
    }
}
