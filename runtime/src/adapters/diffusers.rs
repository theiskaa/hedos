//! The diffusers image adapter: generates images for diffusers pipelines through
//! a Python sidecar. The general-purpose image runtime (mflux specializes in FLUX
//! and out-prefers it there). A [`JobRunning`] adapter — its streaming `invoke` is
//! rejected ([`RuntimeError::WrongExecutionMode`]); work happens through `run`.

use std::path::PathBuf;
use std::time::Duration;

use kernel::records::{
    BidPreference, Capability, JsonValue, Modality, ModelRecord, RunTier, RuntimeId,
};
use kernel::resolution::{IdentifiedModel, ModelFormat, RuntimeBid};

use super::sidecar_job_adapter::{SidecarJobAdapter, SidecarJobSpec};
use super::{ChunkStream, JobRunning, JobStream, RuntimeAdapter, RuntimeError, RuntimeStream};
use crate::environment::EnvironmentManager;
use crate::governor::MemoryGovernor;
use crate::sidecar::SidecarSupervisor;

/// How long a loaded diffusers model stays warm after its last use.
const WARM_WINDOW: Duration = Duration::from_secs(60);

/// The diffusers image Python-sidecar adapter.
pub struct DiffusersAdapter {
    base: SidecarJobAdapter,
}

impl DiffusersAdapter {
    /// An adapter that governs generations through `governor`, launches sidecars
    /// through `supervisor`, prepares environments through `environments`, finds
    /// its bundle under `search_roots`, and runs sidecars under `workdir_root`.
    pub fn new(
        governor: MemoryGovernor,
        supervisor: SidecarSupervisor,
        environments: EnvironmentManager,
        search_roots: Vec<PathBuf>,
        workdir_root: PathBuf,
    ) -> Self {
        Self {
            base: SidecarJobAdapter::new(
                SidecarJobSpec {
                    id: RuntimeId::diffusers(),
                    bundle_name: "python-diffusers",
                    preparing_status: "Preparing image runtime…",
                    starting_status: "Starting image runtime…",
                    warm_window: Some(WARM_WINDOW),
                },
                governor,
                supervisor,
                environments,
                search_roots,
                workdir_root,
            ),
        }
    }
}

impl RuntimeAdapter for DiffusersAdapter {
    fn id(&self) -> &RuntimeId {
        self.base.id()
    }

    fn can_serve(&self, record: &ModelRecord, capability: &Capability) -> bool {
        record.runtime.id.as_ref() == Some(self.base.id()) && capability == &Capability::image()
    }

    fn bid(&self, _record: &ModelRecord, identified: &IdentifiedModel) -> Option<RuntimeBid> {
        if identified.format == ModelFormat::Diffusers
            && identified.modality == Some(Modality::image())
            && identified.capabilities.contains(&Capability::image())
        {
            Some(RuntimeBid::new(RunTier::Managed, BidPreference::DIFFUSERS))
        } else {
            None
        }
    }

    fn invoke(
        &self,
        _record: &ModelRecord,
        _capability: Capability,
        _payload: JsonValue,
    ) -> ChunkStream {
        RuntimeStream::failed(RuntimeError::WrongExecutionMode)
    }
}

impl JobRunning for DiffusersAdapter {
    fn run(&self, record: &ModelRecord, _capability: Capability, payload: JsonValue) -> JobStream {
        self.base.job(record, "image", payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::governor::GovernorConfig;
    use kernel::records::{ExecutionMode, ModelSource, ModelState, SourceKind};

    fn adapter() -> DiffusersAdapter {
        DiffusersAdapter::new(
            MemoryGovernor::new(GovernorConfig::with_total_mb(262_144)),
            SidecarSupervisor::default(),
            EnvironmentManager::new(std::env::temp_dir().join("hedos-diffusers-env")),
            Vec::new(),
            std::env::temp_dir().join("hedos-diffusers-workdirs"),
        )
    }

    fn record() -> ModelRecord {
        let mut record = ModelRecord::new(
            "sd-model",
            Modality::image(),
            Vec::new(),
            ModelSource::new(SourceKind::huggingface_cache(), "/models/m"),
        );
        record.runtime.id = Some(RuntimeId::diffusers());
        record.state = ModelState::Ready;
        record
    }

    fn identified(format: ModelFormat, caps: Vec<Capability>) -> IdentifiedModel {
        IdentifiedModel::new(format, Some(Modality::image()), caps, ExecutionMode::Job)
    }

    #[test]
    fn it_serves_only_image_for_its_own_runtime() {
        let adapter = adapter();
        assert_eq!(adapter.id(), &RuntimeId::diffusers());
        assert!(adapter.can_serve(&record(), &Capability::image()));
        assert!(!adapter.can_serve(&record(), &Capability::chat()));
    }

    #[test]
    fn it_does_not_serve_another_runtimes_model() {
        let adapter = adapter();
        let mut other = record();
        other.runtime.id = Some(RuntimeId::mflux());
        assert!(!adapter.can_serve(&other, &Capability::image()));
    }

    #[test]
    fn it_bids_on_any_diffusers_image_model_with_no_alternatives() {
        let bid = adapter()
            .bid(
                &record(),
                &identified(ModelFormat::Diffusers, vec![Capability::image()]),
            )
            .unwrap();
        assert_eq!(bid.tier, RunTier::Managed);
        assert_eq!(bid.preference, BidPreference::DIFFUSERS);
        assert!(bid.alternatives.is_empty());
    }

    #[test]
    fn it_does_not_bid_without_the_diffusers_format_or_image_capability() {
        let adapter = adapter();
        assert!(
            adapter
                .bid(
                    &record(),
                    &identified(ModelFormat::Safetensors, vec![Capability::image()])
                )
                .is_none()
        );
        assert!(
            adapter
                .bid(
                    &record(),
                    &identified(ModelFormat::Diffusers, vec![Capability::chat()])
                )
                .is_none()
        );
    }

    #[tokio::test]
    async fn streaming_invoke_is_rejected_as_wrong_execution_mode() {
        let adapter = adapter();
        let mut stream = adapter.invoke(
            &record(),
            Capability::image(),
            JsonValue::Object(Default::default()),
        );
        assert!(matches!(
            stream.recv().await,
            Some(Err(RuntimeError::WrongExecutionMode))
        ));
        assert!(stream.recv().await.is_none());
    }

    #[tokio::test]
    async fn run_without_a_bundle_yields_a_bundle_missing_error() {
        let adapter = adapter();
        let mut stream = adapter.run(
            &record(),
            Capability::image(),
            JsonValue::Object(Default::default()),
        );
        let mut error = None;
        while let Some(item) = stream.recv().await {
            if let Err(RuntimeError::Failed(message)) = item {
                error = Some(message);
                break;
            }
        }
        assert!(
            error
                .as_deref()
                .is_some_and(|message| message.contains("missing")),
            "expected a bundle-missing failure, got {error:?}"
        );
    }
}
