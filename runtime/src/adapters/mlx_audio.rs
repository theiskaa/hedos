//! The mlx-audio speech adapter: serves the `speak` (text-to-speech) capability
//! for safetensors / MLX-safetensors speech models through a Python sidecar.

use std::path::PathBuf;
use std::time::Duration;

use kernel::records::{
    BidPreference, Capability, JsonValue, Modality, ModelRecord, RunTier, RuntimeId,
};
use kernel::resolution::{IdentifiedModel, ModelFormat, RuntimeBid};

use super::sidecar_adapter::{CancelMode, SidecarAdapter, SidecarSpec};
use super::{ChunkStream, RuntimeAdapter};
use crate::environment::EnvironmentManager;
use crate::governor::MemoryGovernor;
use crate::sidecar::SidecarSupervisor;

/// How long a loaded speech model stays warm after its last use.
const WARM_WINDOW: Duration = Duration::from_secs(120);

/// The mlx-audio speech Python-sidecar adapter.
pub struct MlxAudioAdapter {
    base: SidecarAdapter,
}

impl MlxAudioAdapter {
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
            base: SidecarAdapter::new(
                SidecarSpec {
                    id: RuntimeId::mlx_audio(),
                    bundle_name: "python-mlx-audio",
                    preparing_status: "Preparing speech runtime…",
                    starting_status: "Starting speech runtime…",
                    warm_window: Some(WARM_WINDOW),
                    cancel: CancelMode::Cooperative,
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

impl RuntimeAdapter for MlxAudioAdapter {
    fn id(&self) -> &RuntimeId {
        self.base.id()
    }

    fn can_serve(&self, record: &ModelRecord, capability: &Capability) -> bool {
        record.runtime.id.as_ref() == Some(self.base.id()) && capability == &Capability::speak()
    }

    fn bid(&self, _record: &ModelRecord, identified: &IdentifiedModel) -> Option<RuntimeBid> {
        if identified.modality == Some(Modality::speech())
            && identified.capabilities.contains(&Capability::speak())
            && matches!(
                identified.format,
                ModelFormat::Safetensors | ModelFormat::MlxSafetensors
            )
        {
            Some(RuntimeBid::new(RunTier::Managed, BidPreference::MLX_AUDIO))
        } else {
            None
        }
    }

    fn invoke(
        &self,
        record: &ModelRecord,
        _capability: Capability,
        payload: JsonValue,
    ) -> ChunkStream {
        self.base.stream(record, Capability::speak(), payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::governor::GovernorConfig;
    use kernel::records::{ExecutionMode, ModelSource, ModelState, SourceKind};

    fn adapter() -> MlxAudioAdapter {
        MlxAudioAdapter::new(
            MemoryGovernor::new(GovernorConfig::with_total_mb(262_144)),
            SidecarSupervisor::default(),
            EnvironmentManager::new(std::env::temp_dir().join("hedos-mlx-audio-env")),
            Vec::new(),
            std::env::temp_dir().join("hedos-mlx-audio-workdirs"),
        )
    }

    fn record() -> ModelRecord {
        let mut record = ModelRecord::new(
            "m",
            Modality::speech(),
            Vec::new(),
            ModelSource::new(SourceKind::huggingface_cache(), "/models/m"),
        );
        record.runtime.id = Some(RuntimeId::mlx_audio());
        record.state = ModelState::Ready;
        record
    }

    fn identified(
        format: ModelFormat,
        modality: Modality,
        caps: Vec<Capability>,
    ) -> IdentifiedModel {
        IdentifiedModel::new(format, Some(modality), caps, ExecutionMode::Sync)
    }

    #[test]
    fn it_serves_only_speak_for_its_own_runtime() {
        let adapter = adapter();
        assert_eq!(adapter.id(), &RuntimeId::mlx_audio());
        assert!(adapter.can_serve(&record(), &Capability::speak()));
        assert!(!adapter.can_serve(&record(), &Capability::chat()));
    }

    #[test]
    fn it_does_not_serve_another_runtimes_model() {
        let adapter = adapter();
        let mut other = record();
        other.runtime.id = Some(RuntimeId::llama_cpp());
        assert!(!adapter.can_serve(&other, &Capability::speak()));
    }

    #[test]
    fn it_bids_on_speech_speak_models_in_either_safetensors_format() {
        let adapter = adapter();
        for format in [ModelFormat::Safetensors, ModelFormat::MlxSafetensors] {
            let bid = adapter
                .bid(
                    &record(),
                    &identified(format, Modality::speech(), vec![Capability::speak()]),
                )
                .unwrap();
            assert_eq!(bid.tier, RunTier::Managed);
            assert_eq!(bid.preference, BidPreference::MLX_AUDIO);
            assert!(bid.alternatives.is_empty());
        }
    }

    #[test]
    fn it_does_not_bid_without_speech_modality_speak_capability_or_a_supported_format() {
        let adapter = adapter();
        // Wrong modality.
        assert!(
            adapter
                .bid(
                    &record(),
                    &identified(
                        ModelFormat::Safetensors,
                        Modality::text(),
                        vec![Capability::speak()]
                    )
                )
                .is_none()
        );
        // No speak capability.
        assert!(
            adapter
                .bid(
                    &record(),
                    &identified(
                        ModelFormat::Safetensors,
                        Modality::speech(),
                        vec![Capability::chat()]
                    )
                )
                .is_none()
        );
        // Wrong format.
        assert!(
            adapter
                .bid(
                    &record(),
                    &identified(
                        ModelFormat::Gguf,
                        Modality::speech(),
                        vec![Capability::speak()]
                    )
                )
                .is_none()
        );
    }

    #[test]
    fn it_honors_no_params_and_has_no_context_window() {
        let adapter = adapter();
        assert!(
            adapter
                .honored_param_keys(&record(), &Capability::speak())
                .is_empty()
        );
        assert_eq!(
            adapter.effective_context_window(&record(), Some(4096)),
            None
        );
    }

    #[test]
    fn it_keeps_the_sidecar_warm_and_cooperatively_cancels() {
        let adapter = adapter();
        let spec = adapter.base.spec();
        assert_eq!(spec.cancel, CancelMode::Cooperative);
        assert_eq!(spec.warm_window, Some(WARM_WINDOW));
    }

    #[tokio::test]
    async fn invoke_without_a_bundle_yields_a_bundle_missing_error() {
        use super::super::RuntimeError;
        let adapter = adapter();
        let mut stream = adapter.invoke(
            &record(),
            Capability::speak(),
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
