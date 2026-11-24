//! The mlx-vlm vision adapter: serves chat and vision for MLX-safetensors
//! vision-language models, and on Apple Silicon for the original Inkling 2
//! checkpoints, through a Python sidecar. Like [`MlxLmAdapter`] but for
//! `see`-capable models, and its output is not think-split.
//!
//! [`MlxLmAdapter`]: super::MlxLmAdapter

use std::collections::HashSet;
use std::path::PathBuf;

use kernel::records::{BidPreference, Capability, JsonValue, ModelRecord, RunTier, RuntimeId};
use kernel::resolution::{IdentifiedModel, ModelFormat, RuntimeBid};

use super::inkling::InklingCheckpoint;
use super::sidecar_adapter::{CancelMode, SidecarAdapter, SidecarSpec};
use super::{ChunkStream, RuntimeAdapter};
use crate::environment::EnvironmentManager;
use crate::governor::MemoryGovernor;
use crate::sidecar::SidecarSupervisor;

/// Whether this build reads original checkpoints in place. Their mixed
/// NVFP4/MXFP8 experts are verified on Apple Silicon's Metal backend only, so
/// elsewhere only an MLX conversion bids.
const LOADS_ORIGINAL_CHECKPOINTS: bool = cfg!(all(target_os = "macos", target_arch = "aarch64"));

/// The bundle this sidecar ships as.
pub(crate) const MLX_VLM_BUNDLE: &str = "python-mlx-vlm";

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
                    bundle_name: MLX_VLM_BUNDLE,
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

    fn wires_tools(&self) -> bool {
        // The sidecar offers the request's tools (in Inkling's own template, or
        // through a system block) and parses tool calls back out of the turn.
        true
    }

    fn can_serve(&self, record: &ModelRecord, capability: &Capability) -> bool {
        record.runtime.id.as_ref() == Some(self.base.id()) && is_vision_capability(capability)
    }

    fn bid(&self, record: &ModelRecord, identified: &IdentifiedModel) -> Option<RuntimeBid> {
        if !identified.capabilities.contains(&Capability::see()) {
            return None;
        }
        match identified.format {
            ModelFormat::MlxSafetensors => InklingCheckpoint::of(record)
                .is_none_or(|inkling| inkling.loads_converted())
                .then(|| {
                    RuntimeBid::with_alternatives(
                        RunTier::Managed,
                        BidPreference::MLX_VLM,
                        vec![RuntimeId::mlx_swift()],
                    )
                }),
            // The checkpoint stays plain safetensors, so every other runtime
            // still sees the format it really is.
            ModelFormat::Safetensors if LOADS_ORIGINAL_CHECKPOINTS => InklingCheckpoint::of(record)
                .is_some_and(|inkling| inkling.loads_native())
                .then(|| RuntimeBid::new(RunTier::Managed, BidPreference::MLX_VLM)),
            _ => None,
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
    fn it_wires_tools() {
        // Keeps the `tools` capability fold granting tools to mlx-vlm models
        // (resolution::fold_tool_capability keys on this).
        assert!(adapter().wires_tools());
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

    /// A directory under the system temp dir, removed when dropped.
    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    /// A record over a folder whose `config.json` is `config`, holding the
    /// ModelOpt quantization sidecar too, with the folder that holds it.
    fn checkpoint(name: &str, config: serde_json::Value) -> (Scratch, ModelRecord) {
        let dir = std::env::temp_dir().join(format!("hedos-mlx-vlm-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.json"), config.to_string()).unwrap();
        std::fs::write(dir.join("hf_quant_config.json"), "{}").unwrap();
        let mut record = record();
        record.source = ModelSource::new(SourceKind::folder(), dir.to_str().unwrap());
        (Scratch(dir), record)
    }

    fn inkling_mini(is_varloop: bool) -> serde_json::Value {
        serde_json::json!({
            "model_type": "inkling2_loop",
            "vision_config": {},
            "quantization_config": {"quant_method": "modelopt"},
            "text_config": {"loop_config": {"is_varloop": is_varloop}},
        })
    }

    #[test]
    fn it_bids_on_an_original_inkling_2_checkpoint_on_apple_silicon_only() {
        let (_dir, mini) = checkpoint("native-mini", inkling_mini(false));
        let bid = adapter().bid(
            &mini,
            &identified(ModelFormat::Safetensors, vec![Capability::see()]),
        );
        if LOADS_ORIGINAL_CHECKPOINTS {
            let bid = bid.unwrap();
            assert_eq!(bid.preference, BidPreference::MLX_VLM);
            assert!(bid.alternatives.is_empty());
        } else {
            assert!(bid.is_none());
        }
    }

    #[test]
    fn it_does_not_bid_on_another_original_vision_checkpoint() {
        let (_dir, other) = checkpoint(
            "native-other",
            serde_json::json!({"model_type": "qwen2_vl", "vision_config": {}}),
        );
        let plain = identified(ModelFormat::Safetensors, vec![Capability::see()]);
        assert!(adapter().bid(&other, &plain).is_none());
        assert!(
            adapter().bid(&record(), &plain).is_none(),
            "no config on disk"
        );
    }

    #[test]
    fn it_does_not_bid_on_a_variable_loop_inkling_in_either_form() {
        let (_dir, variable) = checkpoint("variable", inkling_mini(true));
        for format in [ModelFormat::Safetensors, ModelFormat::MlxSafetensors] {
            assert!(
                adapter()
                    .bid(&variable, &identified(format, vec![Capability::see()]))
                    .is_none(),
                "{format:?}"
            );
        }
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
