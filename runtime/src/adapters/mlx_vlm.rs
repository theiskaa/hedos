//! The mlx-vlm vision adapter: serves chat/vision for MLX-safetensors
//! vision-language models through a Python sidecar. Like [`MlxLmAdapter`] but for
//! `see`-capable models, and its output is not think-split.

use std::collections::HashSet;
use std::path::PathBuf;

use kernel::records::{BidPreference, Capability, JsonValue, ModelRecord, RunTier, RuntimeId};
use kernel::resolution::{IdentifiedModel, ModelFormat, RuntimeBid};

use super::sidecar_adapter::{CancelMode, SidecarAdapter, SidecarSpec};
use super::{ChunkStream, RuntimeAdapter};
use crate::environment::EnvironmentManager;
use crate::governor::MemoryGovernor;
use crate::sidecar::SidecarSupervisor;

/// The mlx-vlm vision Python-sidecar adapter.
pub struct MlxVlmAdapter {
    base: SidecarAdapter,
}

impl MlxVlmAdapter {
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
                    id: RuntimeId::mlx_vlm(),
                    bundle_name: "python-mlx-vlm",
                    preparing_status: "Preparing vision runtime…",
                    starting_status: "Starting vision runtime…",
                    warm_window: None,
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

impl RuntimeAdapter for MlxVlmAdapter {
    fn id(&self) -> &RuntimeId {
        self.base.id()
    }

    fn can_serve(&self, record: &ModelRecord, capability: &Capability) -> bool {
        record.runtime.id.as_ref() == Some(self.base.id()) && is_vision_capability(capability)
    }

    fn bid(&self, _record: &ModelRecord, identified: &IdentifiedModel) -> Option<RuntimeBid> {
        if identified.format == ModelFormat::MlxSafetensors
            && identified.capabilities.contains(&Capability::see())
        {
            Some(RuntimeBid::with_alternatives(
                RunTier::Managed,
                BidPreference::MLX_VLM,
                vec![RuntimeId::mlx_swift()],
            ))
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
        // Vision requests always run through the chat op; the output is not
        // think-split.
        self.base.stream(record, Capability::chat(), payload)
    }

    fn effective_context_window(
        &self,
        record: &ModelRecord,
        _requested: Option<i64>,
    ) -> Option<i64> {
        record.context_length
    }

    fn honored_param_keys(
        &self,
        _record: &ModelRecord,
        capability: &Capability,
    ) -> HashSet<String> {
        if is_vision_capability(capability) {
            ["temperature", "top_p", "max_tokens"]
                .into_iter()
                .map(str::to_owned)
                .collect()
        } else {
            HashSet::new()
        }
    }
}

fn is_vision_capability(capability: &Capability) -> bool {
    capability == &Capability::chat() || capability == &Capability::see()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::governor::GovernorConfig;
    use kernel::records::{ExecutionMode, Modality, ModelSource, ModelState, SourceKind};

    fn adapter() -> MlxVlmAdapter {
        MlxVlmAdapter::new(
            MemoryGovernor::new(GovernorConfig::with_total_mb(262_144)),
            SidecarSupervisor::default(),
            EnvironmentManager::new(std::env::temp_dir().join("hedos-mlx-vlm-env")),
            Vec::new(),
            std::env::temp_dir().join("hedos-mlx-vlm-workdirs"),
        )
    }

    fn record() -> ModelRecord {
        let mut record = ModelRecord::new(
            "m",
            Modality::vision(),
            Vec::new(),
            ModelSource::new(SourceKind::huggingface_cache(), "/models/m"),
        );
        record.runtime.id = Some(RuntimeId::mlx_vlm());
        record.state = ModelState::Ready;
        record.context_length = Some(4096);
        record
    }

    fn identified(format: ModelFormat, caps: Vec<Capability>) -> IdentifiedModel {
        IdentifiedModel::new(format, Some(Modality::vision()), caps, ExecutionMode::Sync)
    }

    #[test]
    fn it_serves_chat_and_see_for_its_own_runtime() {
        let adapter = adapter();
        assert_eq!(adapter.id(), &RuntimeId::mlx_vlm());
        assert!(adapter.can_serve(&record(), &Capability::chat()));
        assert!(adapter.can_serve(&record(), &Capability::see()));
        assert!(!adapter.can_serve(&record(), &Capability::embed()));
    }

    #[test]
    fn it_does_not_serve_another_runtimes_model() {
        let adapter = adapter();
        let mut other = record();
        other.runtime.id = Some(RuntimeId::llama_cpp());
        assert!(!adapter.can_serve(&other, &Capability::chat()));
    }

    #[test]
    fn it_bids_on_mlx_safetensors_vision_models_with_mlx_swift_as_an_alternative() {
        let bid = adapter()
            .bid(
                &record(),
                &identified(ModelFormat::MlxSafetensors, vec![Capability::see()]),
            )
            .unwrap();
        assert_eq!(bid.tier, RunTier::Managed);
        assert_eq!(bid.preference, BidPreference::MLX_VLM);
        assert_eq!(bid.alternatives, vec![RuntimeId::mlx_swift()]);
    }

    #[test]
    fn it_does_not_bid_without_the_see_capability_or_the_right_format() {
        let adapter = adapter();
        assert!(
            adapter
                .bid(
                    &record(),
                    &identified(ModelFormat::MlxSafetensors, vec![Capability::chat()])
                )
                .is_none()
        );
        assert!(
            adapter
                .bid(
                    &record(),
                    &identified(ModelFormat::Gguf, vec![Capability::see()])
                )
                .is_none()
        );
    }

    #[test]
    fn it_honors_the_vision_sampling_keys_for_chat_and_see_only() {
        let adapter = adapter();
        let honored = adapter.honored_param_keys(&record(), &Capability::see());
        assert!(honored.contains("temperature"));
        assert!(honored.contains("max_tokens"));
        assert!(!honored.contains("top_k"));
        assert!(
            adapter
                .honored_param_keys(&record(), &Capability::embed())
                .is_empty()
        );
    }

    #[test]
    fn its_context_window_is_the_records() {
        assert_eq!(
            adapter().effective_context_window(&record(), None),
            Some(4096)
        );
    }

    #[tokio::test]
    async fn invoke_without_a_bundle_yields_a_bundle_missing_error() {
        use super::super::RuntimeError;
        let adapter = adapter();
        let mut stream = adapter.invoke(
            &record(),
            Capability::see(),
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
