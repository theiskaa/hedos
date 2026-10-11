//! The mlx-audio speech adapter: serves the `speak` (text-to-speech) capability
//! for Kokoro checkpoints (safetensors or MLX safetensors) through a Python
//! sidecar. The sidecar drives Kokoro's own pipeline and voice packs, so other
//! voices (Bark, Parler, an MMS Vits) are left for a runtime that can load them.

use std::path::{Path, PathBuf};
use std::time::Duration;

use kernel::discovery::modality_hints::is_kokoro_layout;
use kernel::records::{
    BidPreference, Capability, JsonValue, Modality, ModelRecord, RunTier, RuntimeId,
};
use kernel::resolution::{IdentifiedModel, ModelFormat, RuntimeBid};

use super::sidecar_adapter::{CancelMode, SidecarAdapter, SidecarSpec};
use super::{ChunkStream, RuntimeAdapter};
use crate::environment::EnvironmentManager;
use crate::governor::MemoryGovernor;
use crate::manifests::SidecarModelPaths;
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

    fn bid(&self, record: &ModelRecord, identified: &IdentifiedModel) -> Option<RuntimeBid> {
        if identified.modality == Some(Modality::speech())
            && identified.capabilities.contains(&Capability::speak())
            && matches!(
                identified.format,
                ModelFormat::Safetensors | ModelFormat::MlxSafetensors
            )
            && is_kokoro_layout(Path::new(&SidecarModelPaths::resolve(record).snapshot))
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

    /// A directory under the system temp dir, removed when dropped.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("hedos-mlx-audio-{name}-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    /// A speech record over a folder whose `config.json` is `config`, with the
    /// folder that holds it.
    fn speech_folder(name: &str, config: &str) -> (Scratch, ModelRecord) {
        let dir = Scratch::new(name);
        std::fs::write(dir.0.join("config.json"), config).unwrap();
        let mut record = record();
        record.source = ModelSource::new(SourceKind::folder(), dir.0.to_str().unwrap());
        (dir, record)
    }

    const KOKORO_CONFIG: &str =
        r#"{"istftnet":{"upsample_rates":[10,6]},"plbert":{},"style_dim":128,"n_mels":80}"#;

    #[test]
    fn it_bids_on_kokoro_in_either_safetensors_format() {
        let adapter = adapter();
        let (_dir, kokoro) = speech_folder("kokoro", KOKORO_CONFIG);
        for format in [ModelFormat::Safetensors, ModelFormat::MlxSafetensors] {
            let bid = adapter
                .bid(
                    &kokoro,
                    &identified(format, Modality::speech(), vec![Capability::speak()]),
                )
                .unwrap();
            assert_eq!(bid.tier, RunTier::Managed);
            assert_eq!(bid.preference, BidPreference::MLX_AUDIO);
            assert!(bid.alternatives.is_empty());
        }
    }

    #[test]
    fn it_bids_on_the_real_kokoro_in_the_hub_cache() {
        let cache = Scratch::new("hub");
        let snapshot = cache.0.join("snapshots").join("abc");
        std::fs::create_dir_all(&snapshot).unwrap();
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../kernel/tests/fixtures/audio/kokoro-82m-bf16/config.json");
        std::fs::copy(fixture, snapshot.join("config.json")).unwrap();
        let mut kokoro = record();
        kokoro.source =
            ModelSource::new(SourceKind::huggingface_cache(), cache.0.to_str().unwrap());
        kokoro.source.reference = Some("abc".to_owned());
        let speech = identified(
            ModelFormat::MlxSafetensors,
            Modality::speech(),
            vec![Capability::speak()],
        );
        assert!(adapter().bid(&kokoro, &speech).is_some());
    }

    #[test]
    fn it_leaves_a_voice_its_kokoro_pipeline_cannot_load() {
        let adapter = adapter();
        let (_dir, bark) = speech_folder(
            "bark",
            r#"{"architectures":["BarkModel"],"model_type":"bark"}"#,
        );
        let speech = identified(
            ModelFormat::Safetensors,
            Modality::speech(),
            vec![Capability::speak()],
        );
        assert!(adapter.bid(&bark, &speech).is_none());
        assert!(
            adapter.bid(&record(), &speech).is_none(),
            "no config on disk"
        );
    }

    #[test]
    fn it_does_not_bid_without_speech_modality_speak_capability_or_a_supported_format() {
        let adapter = adapter();
        // A Kokoro folder, so only the condition under test can refuse.
        let (_dir, kokoro) = speech_folder("kokoro-refusals", KOKORO_CONFIG);
        // Wrong modality.
        assert!(
            adapter
                .bid(
                    &kokoro,
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
                    &kokoro,
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
                    &kokoro,
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
