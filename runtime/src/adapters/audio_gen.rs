//! The audio-generation adapter: makes songs, music and sound effects with the
//! models diffusers and transformers can run by themselves (the diffusers audio
//! pipelines, ACE-Step's `AceStepPipeline`, MusicGen) through one Python
//! sidecar. Models that ship their own inference code are served by a manifest
//! runtime of their own instead. A [`JobRunning`] adapter: its streaming
//! `invoke` is rejected ([`RuntimeError::WrongExecutionMode`]).

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

/// How long a loaded audio model stays warm after its last use.
const WARM_WINDOW: Duration = Duration::from_secs(60);

/// The bundle this sidecar ships as.
pub(crate) const AUDIO_BUNDLE: &str = "python-audio";

/// The audio-generation Python-sidecar adapter.
pub struct AudioGenAdapter {
    base: SidecarJobAdapter,
}

impl AudioGenAdapter {
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
                    id: RuntimeId::audio(),
                    bundle_name: AUDIO_BUNDLE,
                    preparing_status: "Preparing audio runtime…",
                    starting_status: "Starting audio runtime…",
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

impl RuntimeAdapter for AudioGenAdapter {
    fn id(&self) -> &RuntimeId {
        self.base.id()
    }

    fn can_serve(&self, record: &ModelRecord, capability: &Capability) -> bool {
        record.runtime.id.as_ref() == Some(self.base.id()) && capability == &Capability::audio()
    }

    /// A diffusers audio pipeline, or a transformers checkpoint identification
    /// already marked as generating audio (MusicGen). A model that makes audio
    /// with its own code claims no capability, so it draws no bid here.
    fn bid(&self, _record: &ModelRecord, identified: &IdentifiedModel) -> Option<RuntimeBid> {
        let library = matches!(
            identified.format,
            ModelFormat::Diffusers | ModelFormat::Safetensors
        );
        (library
            && identified.modality == Some(Modality::audio())
            && identified.capabilities.contains(&Capability::audio()))
        .then(|| RuntimeBid::new(RunTier::Managed, BidPreference::AUDIO))
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

impl JobRunning for AudioGenAdapter {
    fn run(&self, record: &ModelRecord, _capability: Capability, payload: JsonValue) -> JobStream {
        self.base.job(record, "audio", payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::governor::GovernorConfig;
    use kernel::records::{ExecutionMode, ModelSource, ModelState, SourceKind};

    fn adapter() -> AudioGenAdapter {
        AudioGenAdapter::new(
            MemoryGovernor::new(GovernorConfig::with_total_mb(65_536)),
            SidecarSupervisor::default(),
            EnvironmentManager::new(std::env::temp_dir().join("hedos-audio-gen-env")),
            Vec::new(),
            std::env::temp_dir().join("hedos-audio-gen-workdirs"),
        )
    }

    fn record() -> ModelRecord {
        let mut record = ModelRecord::new(
            "m",
            Modality::audio(),
            vec![Capability::audio()],
            ModelSource::new(SourceKind::huggingface_cache(), "/models/m"),
        );
        record.runtime.id = Some(RuntimeId::audio());
        record.state = ModelState::Ready;
        record
    }

    fn identified(
        format: ModelFormat,
        modality: Modality,
        capabilities: Vec<Capability>,
    ) -> IdentifiedModel {
        IdentifiedModel::new(format, Some(modality), capabilities, ExecutionMode::Job)
    }

    #[test]
    fn it_serves_only_audio_for_its_own_runtime() {
        let adapter = adapter();
        assert_eq!(adapter.id(), &RuntimeId::audio());
        assert!(adapter.can_serve(&record(), &Capability::audio()));
        assert!(!adapter.can_serve(&record(), &Capability::speak()));
        let mut other = record();
        other.runtime.id = Some(RuntimeId::diffusers());
        assert!(!adapter.can_serve(&other, &Capability::audio()));
    }

    #[test]
    fn it_bids_on_library_audio_generators() {
        let adapter = adapter();
        for format in [ModelFormat::Diffusers, ModelFormat::Safetensors] {
            let bid = adapter
                .bid(
                    &record(),
                    &identified(format, Modality::audio(), vec![Capability::audio()]),
                )
                .expect("a bid");
            assert_eq!(bid.tier, RunTier::Managed);
            assert_eq!(bid.preference, BidPreference::AUDIO);
        }
    }

    #[test]
    fn it_leaves_custom_code_voices_and_transcribers_alone() {
        let adapter = adapter();
        for refused in [
            // YuE2: identified as audio, but nothing built in drives it.
            identified(ModelFormat::Safetensors, Modality::audio(), Vec::new()),
            identified(
                ModelFormat::MlxSafetensors,
                Modality::audio(),
                vec![Capability::audio()],
            ),
            identified(
                ModelFormat::Safetensors,
                Modality::audio(),
                vec![Capability::transcribe()],
            ),
            identified(
                ModelFormat::Safetensors,
                Modality::speech(),
                vec![Capability::speak()],
            ),
            identified(
                ModelFormat::Diffusers,
                Modality::image(),
                vec![Capability::image()],
            ),
        ] {
            assert!(adapter.bid(&record(), &refused).is_none(), "{refused:?}");
        }
    }

    #[tokio::test]
    async fn streaming_invoke_is_the_wrong_execution_mode() {
        let mut stream = adapter().invoke(
            &record(),
            Capability::audio(),
            JsonValue::Object(Default::default()),
        );
        assert!(matches!(
            stream.recv().await,
            Some(Err(RuntimeError::WrongExecutionMode))
        ));
    }
}
