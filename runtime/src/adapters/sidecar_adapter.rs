//! The shared machinery behind the Python-sidecar adapters. Each adapter differs
//! only in a handful of constants ([`SidecarSpec`]) and its decision logic
//! (`can_serve`/`bid`/…); [`SidecarAdapter`] holds the identical plumbing — the
//! governed runtime, the descriptor, and the stream bridge — once.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use kernel::records::{Capability, JsonValue, ModelRecord, RuntimeId};

use super::ChunkStream;
use super::sidecar_stream::bridge;
use crate::environment::EnvironmentManager;
use crate::governor::MemoryGovernor;
use crate::python_runtime::{Descriptor, PrepareEnvironment, PythonSidecarRuntime};
use crate::sidecar::{RuntimeBundle, SidecarError, bundle_spec};

/// How long to wait for a cooperatively-cancelled sidecar to acknowledge before
/// killing it — the default the MLX/embeddings adapters use.
const CANCEL_GRACE: Duration = Duration::from_secs(10);

/// What a cancelled stream does to the sidecar: keep it warm for the next request
/// (streaming models) or kill it (short one-shot models like embeddings).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CancelMode {
    /// Send a cooperative `cancel` and keep the sidecar warm.
    Cooperative,
    /// Hard-kill the sidecar.
    HardKill,
}

impl CancelMode {
    fn is_cooperative(self) -> bool {
        matches!(self, CancelMode::Cooperative)
    }
}

/// The per-runtime constants that distinguish one sidecar adapter from another.
pub(crate) struct SidecarSpec {
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
    /// What a cancelled stream does to the sidecar.
    pub cancel: CancelMode,
}

/// The shared plumbing every Python-sidecar adapter wraps: it builds the governed
/// runtime and its descriptor, and bridges a capability stream out.
pub(crate) struct SidecarAdapter {
    spec: SidecarSpec,
    governor: MemoryGovernor,
    supervisor: crate::sidecar::SidecarSupervisor,
    environments: EnvironmentManager,
    search_roots: Arc<Vec<PathBuf>>,
    workdir_root: PathBuf,
}

impl SidecarAdapter {
    /// Wrap the shared subsystems for a runtime described by `spec`.
    pub(crate) fn new(
        spec: SidecarSpec,
        governor: MemoryGovernor,
        supervisor: crate::sidecar::SidecarSupervisor,
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

    /// The per-runtime constants this adapter was built with, for tests that pin
    /// an adapter's own sidecar wiring (cancel mode, warm window).
    #[cfg(test)]
    pub(crate) fn spec(&self) -> &SidecarSpec {
        &self.spec
    }

    /// Serve `op` for `record`, bridging the sidecar stream into a runtime stream.
    pub(crate) fn stream(
        &self,
        record: &ModelRecord,
        op: Capability,
        payload: JsonValue,
    ) -> ChunkStream {
        bridge(self.runtime().stream(record, op, payload))
    }

    fn runtime(&self) -> PythonSidecarRuntime {
        PythonSidecarRuntime::new(
            sidecar_descriptor(
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

/// Build the sidecar [`Descriptor`] for `spec`: it locates the bundle under
/// `search_roots`, prepares the environment from its lockfile, and builds the
/// launch spec.
pub(crate) fn sidecar_descriptor(
    spec: &SidecarSpec,
    environments: EnvironmentManager,
    search_roots: Arc<Vec<PathBuf>>,
    workdir_root: PathBuf,
) -> Descriptor {
    let bundle_name = spec.bundle_name;
    let cooperative = spec.cancel.is_cooperative();
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
            bundle_spec(
                &spec_id,
                record,
                &bundle,
                env_dir,
                &workdir_root,
                bundle_name,
                &[],
                cooperative,
                CANCEL_GRACE,
            )
        }),
    }
}

/// Build the shared `prepare_environment` step: locate `bundle_name` under
/// `search_roots` for runtime `id`, then prepare its Python environment from the
/// bundle's lockfile. Identical for every sidecar runtime, streaming or job.
pub(crate) fn prepare_environment(
    bundle_name: &'static str,
    id: RuntimeId,
    environments: EnvironmentManager,
    search_roots: Arc<Vec<PathBuf>>,
) -> PrepareEnvironment {
    Arc::new(move |status| {
        let environments = environments.clone();
        let roots = Arc::clone(&search_roots);
        let id = id.clone();
        Box::pin(async move {
            let bundle = RuntimeBundle::require(bundle_name, &roots, &id)?;
            let env_dir = environments
                .prepare(id.as_str(), &bundle.join("requirements.lock"), status)
                .await
                .map_err(|error| SidecarError::RuntimeFailed(error.to_string()))?;
            Ok(Some(env_dir))
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(cancel: CancelMode) -> SidecarSpec {
        SidecarSpec {
            id: RuntimeId::embeddings(),
            bundle_name: "python-embeddings",
            preparing_status: "Preparing…",
            starting_status: "Starting…",
            warm_window: None,
            cancel,
        }
    }

    fn record() -> ModelRecord {
        use kernel::records::{Modality, ModelSource, SourceKind};
        ModelRecord::new(
            "m",
            Modality::text(),
            Vec::new(),
            ModelSource::new(SourceKind::folder(), "/models/m"),
        )
    }

    #[test]
    fn the_cancel_mode_flows_through_to_the_launch_spec() {
        let root = std::env::temp_dir().join("hedos-sidecar-adapter-cancel");
        let bundle = root.join("Runtimes").join("python-embeddings");
        let env_dir = root.join("env");
        std::fs::create_dir_all(&bundle).unwrap();
        std::fs::create_dir_all(&env_dir).unwrap();
        let roots = Arc::new(vec![root.clone()]);

        for (mode, expected) in [
            (CancelMode::Cooperative, true),
            (CancelMode::HardKill, false),
        ] {
            let descriptor = sidecar_descriptor(
                &spec(mode),
                EnvironmentManager::new(env_dir.clone()),
                Arc::clone(&roots),
                root.join("workdirs"),
            );
            let launch = (descriptor.make_spec)(&record(), Some(&env_dir)).unwrap();
            assert_eq!(launch.cooperative_cancel, expected);
        }
        std::fs::remove_dir_all(&root).ok();
    }
}
