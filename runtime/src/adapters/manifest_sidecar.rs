//! The manifest sidecar adapter: it serves a model through a long-running sidecar
//! process a [`RuntimeManifest`]'s `[serve]` section declares, speaking the frame
//! protocol over the ported [`PythonSidecarRuntime`]. Like the command adapter it
//! requires approved host execution and launches the interpreter directly.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use kernel::manifests::RuntimeManifest;
use kernel::records::{
    BidPreference, Capability, ExecutionMode, JsonValue, ModelRecord, RunTier, RuntimeId,
};
use kernel::resolution::{IdentifiedModel, RuntimeBid};

use super::sidecar_stream::{bridge, separating};
use super::{ChunkStream, JobRunning, JobStream, RuntimeAdapter, RuntimeError, RuntimeStream};
use crate::environment::EnvironmentManager;
use crate::governor::MemoryGovernor;
use crate::manifests::{SidecarModelPaths, detect_matches, slug};
use crate::python_runtime::{Descriptor, PythonSidecarRuntime, StatusSink};
use crate::sidecar::{SidecarError, SidecarSpec, SidecarSupervisor, SidecarWorkdir};

/// How long the sidecar has to become ready before it is given up on.
const READY_TIMEOUT: Duration = Duration::from_secs(600);

/// A runtime declared by a manifest `[serve]` section, served through a sidecar.
pub struct ManifestSidecarAdapter {
    id: RuntimeId,
    manifest: Arc<RuntimeManifest>,
    approved_host_execution: bool,
    governor: MemoryGovernor,
    supervisor: SidecarSupervisor,
    environments: EnvironmentManager,
    workdir_root: PathBuf,
}

impl ManifestSidecarAdapter {
    /// An adapter for `manifest`. `approved_host_execution` must be true for it to
    /// bid or run. Governs through `governor`, launches sidecars through
    /// `supervisor`, prepares environments through `environments`, and runs under
    /// `workdir_root`.
    pub fn new(
        manifest: RuntimeManifest,
        approved_host_execution: bool,
        governor: MemoryGovernor,
        supervisor: SidecarSupervisor,
        environments: EnvironmentManager,
        workdir_root: PathBuf,
    ) -> Self {
        let id = RuntimeId::from(manifest.id.as_str());
        Self {
            id,
            manifest: Arc::new(manifest),
            approved_host_execution,
            governor,
            supervisor,
            environments,
            workdir_root,
        }
    }

    fn consent_error(&self) -> RuntimeError {
        RuntimeError::Unavailable(format!(
            "{} runs code on this machine and needs your approval. Approve it from the model's page.",
            self.id.as_str()
        ))
    }

    fn runtime(&self) -> PythonSidecarRuntime {
        PythonSidecarRuntime::new(
            self.descriptor(),
            self.governor.clone(),
            self.supervisor.clone(),
        )
    }

    fn descriptor(&self) -> Descriptor {
        let prepare_manifest = Arc::clone(&self.manifest);
        let prepare_environments = self.environments.clone();
        let spec_manifest = Arc::clone(&self.manifest);
        let spec_workdir_root = self.workdir_root.clone();
        Descriptor {
            runtime_id: self.manifest.id.clone(),
            preparing_status: format!("Preparing {}…", self.id.as_str()),
            starting_status: format!("Starting {}…", self.id.as_str()),
            warm_window: None,
            prepare_environment: Arc::new(move |status| {
                let manifest = Arc::clone(&prepare_manifest);
                let environments = prepare_environments.clone();
                Box::pin(async move { prepare_environment(&manifest, &environments, status).await })
            }),
            make_spec: Arc::new(move |record: &ModelRecord, env_dir: Option<&Path>| {
                build_spec(&spec_manifest, &spec_workdir_root, record, env_dir)
            }),
        }
    }
}

impl RuntimeAdapter for ManifestSidecarAdapter {
    fn id(&self) -> &RuntimeId {
        &self.id
    }

    fn can_serve(&self, record: &ModelRecord, capability: &Capability) -> bool {
        record.runtime.id.as_ref() == Some(&self.id)
            && self.manifest.capabilities.contains(capability)
    }

    fn bid(&self, record: &ModelRecord, _identified: &IdentifiedModel) -> Option<RuntimeBid> {
        let detect = self.manifest.detect.as_ref()?;
        if !self.approved_host_execution || !detect_matches(detect, record) {
            return None;
        }
        let alternatives = self
            .manifest
            .alternatives
            .iter()
            .map(|id| RuntimeId::from(id.as_str()))
            .collect();
        Some(RuntimeBid::with_alternatives(
            RunTier::Managed,
            BidPreference::MANIFEST,
            alternatives,
        ))
    }

    fn invoke(
        &self,
        record: &ModelRecord,
        capability: Capability,
        payload: JsonValue,
    ) -> ChunkStream {
        if !self.approved_host_execution {
            return RuntimeStream::failed(self.consent_error());
        }
        let is_text = capability == Capability::chat() || capability == Capability::complete();
        let stream = bridge(self.runtime().stream(record, capability, payload));
        // Chat/completion output is think-split; other capabilities pass through.
        if is_text { separating(stream) } else { stream }
    }
}

impl JobRunning for ManifestSidecarAdapter {
    fn run(&self, record: &ModelRecord, capability: Capability, payload: JsonValue) -> JobStream {
        if !self.approved_host_execution {
            return RuntimeStream::failed(self.consent_error());
        }
        bridge(self.runtime().job(record, capability.as_str(), payload))
    }
}

/// Prepare the manifest's Python environment if it declares one, returning the
/// environment directory (or `None`).
async fn prepare_environment(
    manifest: &RuntimeManifest,
    environments: &EnvironmentManager,
    status: StatusSink,
) -> Result<Option<PathBuf>, SidecarError> {
    let Some(env) = &manifest.env else {
        return Ok(None);
    };
    let Some(directory) = &manifest.directory else {
        return Err(SidecarError::RuntimeFailed(format!(
            "manifest {} declares [env] but has no directory",
            manifest.id
        )));
    };
    let env_dir = environments
        .prepare(&manifest.id, &directory.join(&env.lockfile), status)
        .await
        .map_err(|error| SidecarError::RuntimeFailed(error.to_string()))?;
    Ok(Some(env_dir))
}

/// Build the launch spec for the manifest sidecar: the interpreter running the
/// `[serve]` entrypoint against the model, in a fresh workdir.
fn build_spec(
    manifest: &RuntimeManifest,
    workdir_root: &Path,
    record: &ModelRecord,
    env_dir: Option<&Path>,
) -> Result<SidecarSpec, SidecarError> {
    let (Some(serve), Some(directory)) = (&manifest.serve, &manifest.directory) else {
        return Err(SidecarError::RuntimeFailed(format!(
            "{} declares no [serve] entrypoint",
            manifest.id
        )));
    };
    let workdir = SidecarWorkdir::directory(workdir_root, &slug(&manifest.id))
        .map_err(|error| SidecarError::RuntimeFailed(error.to_string()))?;
    let paths = SidecarModelPaths::resolve(record);
    let python = env_dir
        .map(|dir| dir.join("bin").join("python"))
        .unwrap_or_else(|| PathBuf::from("/usr/bin/python3"));

    let arguments = vec![
        path_string(&directory.join(&serve.entrypoint)),
        "--model".to_owned(),
        paths.snapshot,
        "--workdir".to_owned(),
        path_string(&workdir),
    ];
    let mut spec = SidecarSpec::new(format!("{}#{}", manifest.id, record.id), python, arguments);
    spec.environment
        .insert("PYTHONDONTWRITEBYTECODE".to_owned(), "1".to_owned());
    spec.environment
        .insert("PYTHONPATH".to_owned(), String::new());
    spec.working_directory = Some(workdir);
    spec.ready_timeout = READY_TIMEOUT;
    // Streaming sidecars cancel cooperatively (keep warm); one-shot ones hard-kill.
    spec.cooperative_cancel = manifest.execution == ExecutionMode::Stream;
    Ok(spec)
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::governor::GovernorConfig;
    use kernel::records::{Modality, ModelSource, ModelState, SourceKind};

    fn manifest_text(execution: &str) -> String {
        // Job execution requires a job-shaped capability; stream a chat one.
        let capability = if execution == "job" { "image" } else { "chat" };
        format!(
            "id = \"serve-runtime\"\ncapabilities = [\"{capability}\"]\nexecution = \"{execution}\"\ndetect = {{ extension = \"gguf\" }}\n[serve]\nentrypoint = \"main.py\"\n[env]\nlockfile = \"requirements.lock\"\n"
        )
    }

    fn manifest(execution: &str) -> RuntimeManifest {
        // A directory manifest so [serve]/[env] are allowed.
        RuntimeManifest::parse(
            &manifest_text(execution),
            Some(PathBuf::from("/runtimes/serve")),
        )
        .unwrap()
    }

    fn adapter(manifest: RuntimeManifest, approved: bool) -> ManifestSidecarAdapter {
        ManifestSidecarAdapter::new(
            manifest,
            approved,
            MemoryGovernor::new(GovernorConfig::with_total_mb(262_144)),
            SidecarSupervisor::default(),
            EnvironmentManager::new(std::env::temp_dir().join("hedos-manifest-sidecar-env")),
            std::env::temp_dir().join("hedos-manifest-sidecar-workdirs"),
        )
    }

    fn record() -> ModelRecord {
        let mut record = ModelRecord::new(
            "m",
            Modality::text(),
            Vec::new(),
            ModelSource::new(SourceKind::folder(), "/models/m.gguf"),
        );
        record.runtime.id = Some(RuntimeId::from("serve-runtime"));
        record.primary_weight_path = Some("/models/m.gguf".to_owned());
        record.state = ModelState::Ready;
        record
    }

    #[test]
    fn it_serves_declared_capabilities_for_its_own_runtime() {
        let adapter = adapter(manifest("stream"), true);
        assert_eq!(adapter.id(), &RuntimeId::from("serve-runtime"));
        assert!(adapter.can_serve(&record(), &Capability::chat()));
        assert!(!adapter.can_serve(&record(), &Capability::embed()));
    }

    #[test]
    fn it_bids_only_when_approved() {
        let identified = IdentifiedModel::new(
            kernel::resolution::ModelFormat::Gguf,
            None,
            Vec::new(),
            ExecutionMode::Stream,
        );
        assert!(
            adapter(manifest("stream"), true)
                .bid(&record(), &identified)
                .is_some()
        );
        assert!(
            adapter(manifest("stream"), false)
                .bid(&record(), &identified)
                .is_none()
        );
    }

    #[tokio::test]
    async fn unapproved_invoke_is_unavailable() {
        let adapter = adapter(manifest("stream"), false);
        let mut stream = adapter.invoke(
            &record(),
            Capability::chat(),
            JsonValue::Object(Default::default()),
        );
        assert!(matches!(
            stream.recv().await,
            Some(Err(RuntimeError::Unavailable(_)))
        ));
    }

    #[test]
    fn the_spec_runs_the_entrypoint_against_the_model() {
        let manifest = manifest("stream");
        let env = PathBuf::from("/env");
        let spec = build_spec(
            &manifest,
            &std::env::temp_dir().join("hedos-manifest-sidecar-spec"),
            &record(),
            Some(&env),
        )
        .unwrap();
        assert_eq!(spec.executable, PathBuf::from("/env/bin/python"));
        assert!(spec.arguments.iter().any(|arg| arg.ends_with("main.py")));
        assert!(spec.arguments.iter().any(|arg| arg == "--model"));
        assert_eq!(spec.environment.get("PYTHONPATH"), Some(&String::new()));
        assert!(spec.cooperative_cancel);
    }

    #[test]
    fn a_job_execution_spec_hard_cancels() {
        let manifest = manifest("job");
        let spec = build_spec(
            &manifest,
            &std::env::temp_dir().join("hedos-manifest-sidecar-spec-job"),
            &record(),
            None,
        )
        .unwrap();
        // No env dir → the system python.
        assert_eq!(spec.executable, PathBuf::from("/usr/bin/python3"));
        assert!(!spec.cooperative_cancel);
    }
}
