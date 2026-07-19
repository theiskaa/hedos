//! The mlx-lm text adapter: serves chat/completion for MLX-safetensors text
//! models through a Python sidecar. The decision logic (which models it serves,
//! its bid, honored params) is here; the actual generation runs in the sidecar
//! launched by the [`Descriptor`] this builds.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use kernel::capabilities::{CapabilityChunk, Piece, ThinkSplitter};
use kernel::records::{
    BidPreference, Capability, JsonValue, Modality, ModelRecord, RunTier, RuntimeId,
};
use kernel::resolution::{IdentifiedModel, ModelFormat, RuntimeBid};
use tokio::sync::mpsc;

use super::{ChunkStream, RuntimeAdapter, RuntimeError, RuntimeStream};
use crate::environment::EnvironmentManager;
use crate::governor::MemoryGovernor;
use crate::python_runtime::{Descriptor, PythonSidecarRuntime, StatusSink};
use crate::sidecar::{RuntimeBundle, SidecarError, SidecarStream, SidecarSupervisor, bundle_spec};

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
        let prepare_environments = self.environments.clone();
        let prepare_roots = Arc::clone(&self.search_roots);
        let spec_roots = Arc::clone(&self.search_roots);
        let spec_workdir_root = self.workdir_root.clone();
        Descriptor {
            runtime_id: RuntimeId::mlx_lm().as_str().to_owned(),
            preparing_status: "Preparing text runtime…".to_owned(),
            starting_status: "Starting text runtime…".to_owned(),
            warm_window: None,
            prepare_environment: Arc::new(move |status: StatusSink| {
                let environments = prepare_environments.clone();
                let roots = Arc::clone(&prepare_roots);
                Box::pin(async move {
                    let bundle = RuntimeBundle::require(BUNDLE_NAME, &roots, &RuntimeId::mlx_lm())?;
                    let env_dir = environments
                        .prepare(
                            RuntimeId::mlx_lm().as_str(),
                            &bundle.join("requirements.lock"),
                            status,
                        )
                        .await
                        .map_err(|error| SidecarError::RuntimeFailed(error.to_string()))?;
                    Ok(Some(env_dir))
                })
            }),
            make_spec: Arc::new(move |record: &ModelRecord, env_dir: Option<&Path>| {
                let bundle =
                    RuntimeBundle::require(BUNDLE_NAME, &spec_roots, &RuntimeId::mlx_lm())?;
                bundle_spec(
                    &RuntimeId::mlx_lm(),
                    record,
                    &bundle,
                    env_dir,
                    &spec_workdir_root,
                    BUNDLE_NAME,
                    &[],
                    true,
                    Duration::from_secs(10),
                )
            }),
        }
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

/// Forward a sidecar stream as a runtime stream, mapping sidecar errors. Dropping
/// the returned stream drops the sidecar stream, triggering its cancellation.
fn bridge(mut sidecar: SidecarStream<CapabilityChunk>) -> ChunkStream {
    let (tx, stream) = RuntimeStream::channel();
    tokio::spawn(async move {
        while let Some(item) = sidecar.recv().await {
            if tx.send(item.map_err(RuntimeError::from)).is_err() {
                break;
            }
        }
    });
    stream
}

/// Run the stream's visible text through a [`ThinkSplitter`], emitting `Thinking`
/// chunks for reasoning delimited by think tags. `Done` flushes any pending text
/// (then carries its stats through); other chunks pass through unchanged. A port
/// of the Swift `ThinkSplitter.separating`.
fn separating(mut upstream: ChunkStream) -> ChunkStream {
    let (tx, stream) = RuntimeStream::channel();
    tokio::spawn(async move {
        let mut splitter = ThinkSplitter::new();
        while let Some(item) = upstream.recv().await {
            match item {
                Ok(CapabilityChunk::Text(text)) => {
                    if !drain_pieces(&tx, splitter.feed(&text)) {
                        return;
                    }
                }
                Ok(CapabilityChunk::Done(stats)) => {
                    if !drain_pieces(&tx, splitter.flush()) {
                        return;
                    }
                    if tx.send(Ok(CapabilityChunk::Done(stats))).is_err() {
                        return;
                    }
                }
                Ok(other) => {
                    if tx.send(Ok(other)).is_err() {
                        return;
                    }
                }
                Err(error) => {
                    let _ = tx.send(Err(error));
                    return;
                }
            }
        }
        let _ = drain_pieces(&tx, splitter.flush());
    });
    stream
}

/// Send each [`Piece`] as its corresponding chunk; returns `false` if the
/// receiver has gone away.
fn drain_pieces(
    tx: &mpsc::UnboundedSender<Result<CapabilityChunk, RuntimeError>>,
    pieces: Vec<Piece>,
) -> bool {
    for piece in pieces {
        let chunk = match piece {
            Piece::Text(value) => CapabilityChunk::Text(value),
            Piece::Thinking(value) => CapabilityChunk::Thinking(value),
        };
        if tx.send(Ok(chunk)).is_err() {
            return false;
        }
    }
    true
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
    async fn separating_splits_thinking_out_of_the_text() {
        let (tx, upstream) = RuntimeStream::channel();
        tx.send(Ok(CapabilityChunk::Text(
            "hi <think>reasoning</think> bye".to_owned(),
        )))
        .unwrap();
        tx.send(Ok(CapabilityChunk::Done(None))).unwrap();
        drop(tx);

        let mut out = separating(upstream);
        let mut texts = Vec::new();
        let mut thinking = Vec::new();
        let mut done = false;
        while let Some(item) = out.recv().await {
            match item.unwrap() {
                CapabilityChunk::Text(text) => texts.push(text),
                CapabilityChunk::Thinking(text) => thinking.push(text),
                CapabilityChunk::Done(_) => done = true,
                _ => {}
            }
        }
        assert!(thinking.iter().any(|text| text.contains("reasoning")));
        let visible = texts.concat();
        assert!(visible.contains("hi") && visible.contains("bye"));
        assert!(!visible.contains("reasoning"));
        assert!(done);
    }

    #[tokio::test]
    async fn invoke_without_a_bundle_yields_a_bundle_missing_error() {
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
