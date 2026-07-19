//! The embeddings adapter: serves the `embed` capability for safetensors /
//! MLX-safetensors embedding models through a Python sidecar. A one-shot runtime
//! (no honored sampling params, no context window, hard-cancelled).

use std::path::PathBuf;
use std::sync::Arc;

use kernel::records::{BidPreference, Capability, JsonValue, ModelRecord, RunTier, RuntimeId};
use kernel::resolution::{IdentifiedModel, ModelFormat, RuntimeBid};

use super::sidecar_adapter::sidecar_descriptor;
use super::sidecar_stream::bridge;
use super::{ChunkStream, RuntimeAdapter};
use crate::environment::EnvironmentManager;
use crate::governor::MemoryGovernor;
use crate::python_runtime::{Descriptor, PythonSidecarRuntime};
use crate::sidecar::SidecarSupervisor;

/// The shipped bundle name for the embeddings runtime.
const BUNDLE_NAME: &str = "python-embeddings";

/// The embeddings Python-sidecar adapter.
pub struct EmbeddingsAdapter {
    id: RuntimeId,
    governor: MemoryGovernor,
    supervisor: SidecarSupervisor,
    environments: EnvironmentManager,
    search_roots: Arc<Vec<PathBuf>>,
    workdir_root: PathBuf,
}

impl EmbeddingsAdapter {
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
            id: RuntimeId::embeddings(),
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
            RuntimeId::embeddings(),
            BUNDLE_NAME,
            "Preparing embedding runtime…",
            "Starting embedding runtime…",
            None,
            // Embedding requests are short one-shots, so a cancelled stream
            // hard-kills the sidecar rather than keeping it warm.
            false,
            self.environments.clone(),
            Arc::clone(&self.search_roots),
            self.workdir_root.clone(),
        )
    }
}

impl RuntimeAdapter for EmbeddingsAdapter {
    fn id(&self) -> &RuntimeId {
        &self.id
    }

    fn can_serve(&self, record: &ModelRecord, capability: &Capability) -> bool {
        record.runtime.id.as_ref() == Some(&self.id) && capability == &Capability::embed()
    }

    fn bid(&self, _record: &ModelRecord, identified: &IdentifiedModel) -> Option<RuntimeBid> {
        if identified.capabilities.contains(&Capability::embed())
            && (identified.format == ModelFormat::Safetensors
                || identified.format == ModelFormat::MlxSafetensors)
        {
            Some(RuntimeBid::new(RunTier::Managed, BidPreference::EMBEDDINGS))
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
        bridge(self.runtime().stream(record, Capability::embed(), payload))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::governor::GovernorConfig;
    use kernel::records::{ExecutionMode, Modality, ModelSource, ModelState, SourceKind};

    fn adapter() -> EmbeddingsAdapter {
        EmbeddingsAdapter::new(
            MemoryGovernor::new(GovernorConfig::with_total_mb(262_144)),
            SidecarSupervisor::default(),
            EnvironmentManager::new(std::env::temp_dir().join("hedos-embeddings-env")),
            Vec::new(),
            std::env::temp_dir().join("hedos-embeddings-workdirs"),
        )
    }

    fn record() -> ModelRecord {
        let mut record = ModelRecord::new(
            "m",
            Modality::embedding(),
            Vec::new(),
            ModelSource::new(SourceKind::huggingface_cache(), "/models/m"),
        );
        record.runtime.id = Some(RuntimeId::embeddings());
        record.state = ModelState::Ready;
        record
    }

    fn identified(format: ModelFormat, caps: Vec<Capability>) -> IdentifiedModel {
        IdentifiedModel::new(
            format,
            Some(Modality::embedding()),
            caps,
            ExecutionMode::Sync,
        )
    }

    #[test]
    fn it_serves_only_embed_for_its_own_runtime() {
        let adapter = adapter();
        assert_eq!(adapter.id(), &RuntimeId::embeddings());
        assert!(adapter.can_serve(&record(), &Capability::embed()));
        assert!(!adapter.can_serve(&record(), &Capability::chat()));
    }

    #[test]
    fn it_does_not_serve_another_runtimes_model() {
        let adapter = adapter();
        let mut other = record();
        other.runtime.id = Some(RuntimeId::llama_cpp());
        assert!(!adapter.can_serve(&other, &Capability::embed()));
    }

    #[test]
    fn it_bids_on_safetensors_and_mlx_safetensors_embedding_models() {
        let adapter = adapter();
        for format in [ModelFormat::Safetensors, ModelFormat::MlxSafetensors] {
            let bid = adapter
                .bid(&record(), &identified(format, vec![Capability::embed()]))
                .unwrap();
            assert_eq!(bid.tier, RunTier::Managed);
            assert_eq!(bid.preference, BidPreference::EMBEDDINGS);
            assert!(bid.alternatives.is_empty());
        }
    }

    #[test]
    fn it_does_not_bid_without_the_embed_capability_or_a_supported_format() {
        let adapter = adapter();
        assert!(
            adapter
                .bid(
                    &record(),
                    &identified(ModelFormat::Safetensors, vec![Capability::chat()])
                )
                .is_none()
        );
        assert!(
            adapter
                .bid(
                    &record(),
                    &identified(ModelFormat::Gguf, vec![Capability::embed()])
                )
                .is_none()
        );
    }

    #[test]
    fn it_honors_no_params_and_has_no_context_window() {
        let adapter = adapter();
        assert!(
            adapter
                .honored_param_keys(&record(), &Capability::embed())
                .is_empty()
        );
        assert_eq!(
            adapter.effective_context_window(&record(), Some(4096)),
            None
        );
    }

    #[tokio::test]
    async fn invoke_without_a_bundle_yields_a_bundle_missing_error() {
        use super::super::RuntimeError;
        let adapter = adapter();
        let mut stream = adapter.invoke(
            &record(),
            Capability::embed(),
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
