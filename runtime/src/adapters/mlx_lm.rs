//! The mlx-lm text adapter: serves chat/completion for MLX-safetensors text
//! models through a Python sidecar. The decision logic (which models it serves,
//! its bid, honored params) is here; the actual generation runs in the sidecar
//! launched by the [`Descriptor`] this builds.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use kernel::records::{
    BidPreference, Capability, JsonValue, Modality, ModelRecord, RunTier, RuntimeId,
};
use kernel::resolution::{IdentifiedModel, ModelFormat, RuntimeBid};

use super::sidecar_adapter::sidecar_descriptor;
use super::sidecar_stream::{bridge, separating};
use super::{ChunkStream, RuntimeAdapter};
use crate::environment::EnvironmentManager;
use crate::governor::MemoryGovernor;
use crate::python_runtime::{Descriptor, PythonSidecarRuntime};
use crate::sidecar::SidecarSupervisor;

/// The shipped bundle name for the mlx-lm runtime.
const BUNDLE_NAME: &str = "python-mlx-lm";

/// The mlx-lm Python-sidecar adapter.
pub struct MlxLmAdapter {
    id: RuntimeId,
    governor: MemoryGovernor,
    supervisor: SidecarSupervisor,
    environments: EnvironmentManager,
    search_roots: Arc<Vec<PathBuf>>,
    workdir_root: PathBuf,
}

impl MlxLmAdapter {
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
            id: RuntimeId::mlx_lm(),
            governor,
            supervisor,
            environments,
            search_roots: Arc::new(search_roots),
            workdir_root,
        }
    }

    fn runtime(&self) -> PythonSidecarRuntime {
        PythonSidecarRuntime::new(
            self.descriptor(),
            self.governor.clone(),
            self.supervisor.clone(),
        )
    }

    fn descriptor(&self) -> Descriptor {
        sidecar_descriptor(
            RuntimeId::mlx_lm(),
            BUNDLE_NAME,
            "Preparing text runtime…",
            "Starting text runtime…",
            None,
            true,
            self.environments.clone(),
            Arc::clone(&self.search_roots),
            self.workdir_root.clone(),
        )
    }
}

impl RuntimeAdapter for MlxLmAdapter {
    fn id(&self) -> &RuntimeId {
        &self.id
    }

    fn can_serve(&self, record: &ModelRecord, capability: &Capability) -> bool {
        record.runtime.id.as_ref() == Some(&self.id) && is_text_capability(capability)
    }

    fn bid(&self, _record: &ModelRecord, identified: &IdentifiedModel) -> Option<RuntimeBid> {
        if identified.format == ModelFormat::MlxSafetensors
            && identified.modality == Some(Modality::text())
            && identified.capabilities.contains(&Capability::chat())
        {
            Some(RuntimeBid::new(RunTier::Managed, BidPreference::MLX_LM))
        } else {
            None
        }
    }

    fn invoke(
        &self,
        record: &ModelRecord,
        capability: Capability,
        payload: JsonValue,
    ) -> ChunkStream {
        let is_text = is_text_capability(&capability);
        let stream = bridge(self.runtime().stream(record, capability, payload));
        // Chat/completion output is think-split, separating reasoning into
        // `Thinking` chunks; other capabilities pass through unchanged.
        if is_text { separating(stream) } else { stream }
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
        if is_text_capability(capability) {
            [
                "temperature",
                "top_p",
                "top_k",
                "min_p",
                "max_tokens",
                "repeat_penalty",
                "seed",
                "stop",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect()
        } else {
            HashSet::new()
        }
    }
}

fn is_text_capability(capability: &Capability) -> bool {
    capability == &Capability::chat() || capability == &Capability::complete()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::governor::{GovernorConfig, MemoryGovernor};
    use kernel::records::{ModelSource, ModelState, SourceKind};

    fn adapter() -> MlxLmAdapter {
        MlxLmAdapter::new(
            MemoryGovernor::new(GovernorConfig::with_total_mb(262_144)),
            SidecarSupervisor::default(),
            EnvironmentManager::new(std::env::temp_dir().join("hedos-mlx-lm-env")),
            Vec::new(),
            std::env::temp_dir().join("hedos-mlx-lm-workdirs"),
        )
    }

    fn record() -> ModelRecord {
        let mut record = ModelRecord::new(
            "m",
            Modality::text(),
            Vec::new(),
            ModelSource::new(SourceKind::huggingface_cache(), "/models/m"),
        );
        record.runtime.id = Some(RuntimeId::mlx_lm());
        record.state = ModelState::Ready;
        record.context_length = Some(8192);
        record
    }

    fn identified(
        format: ModelFormat,
        modality: Modality,
        caps: Vec<Capability>,
    ) -> IdentifiedModel {
        IdentifiedModel::new(
            format,
            Some(modality),
            caps,
            kernel::records::ExecutionMode::Sync,
        )
    }

    #[test]
    fn it_serves_chat_and_complete_for_its_own_runtime() {
        let adapter = adapter();
        assert_eq!(adapter.id(), &RuntimeId::mlx_lm());
        assert!(adapter.can_serve(&record(), &Capability::chat()));
        assert!(adapter.can_serve(&record(), &Capability::complete()));
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
    fn it_bids_only_on_mlx_safetensors_text_chat_models() {
        let adapter = adapter();
        let bid = adapter.bid(
            &record(),
            &identified(
                ModelFormat::MlxSafetensors,
                Modality::text(),
                vec![Capability::chat()],
            ),
        );
        assert_eq!(
            bid,
            Some(RuntimeBid::new(RunTier::Managed, BidPreference::MLX_LM))
        );
        // Wrong format, or no chat capability → no bid.
        assert!(
            adapter
                .bid(
                    &record(),
                    &identified(
                        ModelFormat::Gguf,
                        Modality::text(),
                        vec![Capability::chat()]
                    )
                )
                .is_none()
        );
        assert!(
            adapter
                .bid(
                    &record(),
                    &identified(ModelFormat::MlxSafetensors, Modality::text(), vec![])
                )
                .is_none()
        );
    }

    #[test]
    fn it_honors_the_sampling_keys_for_text_capabilities_only() {
        let adapter = adapter();
        let honored = adapter.honored_param_keys(&record(), &Capability::chat());
        assert!(honored.contains("temperature"));
        assert!(honored.contains("stop"));
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
            Some(8192)
        );
    }

    #[tokio::test]
    async fn invoke_without_a_bundle_yields_a_bundle_missing_error() {
        use super::super::RuntimeError;
        // No search roots → the sidecar's environment prep can't find the bundle,
        // so the stream ends with a runtime error rather than hanging.
        let adapter = adapter();
        let mut stream = adapter.invoke(
            &record(),
            Capability::chat(),
            JsonValue::Object(Default::default()),
        );
        // Status chunks may precede the failure; drain until the error arrives.
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
