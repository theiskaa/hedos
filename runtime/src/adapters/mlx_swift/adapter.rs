//! The MLX-Swift adapter: serves chat and completion for MLX-safetensors text
//! models in-process through the Swift MLX bridge, ported from the Swift
//! kernel's `MlxSwiftAdapter`. It wins the bid over the Python `mlx-lm` sidecar
//! for the same models — but only when the in-process engine can actually run,
//! so a machine without the bridge falls through to the sidecar rather than
//! resolving to a runtime that cannot serve.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

use kernel::capabilities::{CapabilityChunk, ChatMessage, ChatRole, GenerationStats, ToolSpec};
use kernel::records::{BidPreference, Capability, JsonValue, ModelRecord, RunTier, RuntimeId};
use kernel::resolution::{IdentifiedModel, ModelFormat, RuntimeBid};

use super::backend::{MlxSwiftBackend, MlxSwiftEvent, MlxSwiftOptions};
use crate::adapters::{ChunkStream, RuntimeAdapter, RuntimeError};
use crate::governed::governed_one_shot;
use crate::governor::{GpuProducer, MemoryGovernor};
use crate::sidecar::SidecarModelPaths;

/// The adapter over the in-process MLX-Swift bridge.
pub struct MlxSwiftAdapter {
    id: RuntimeId,
    governor: MemoryGovernor,
    backend: Arc<dyn MlxSwiftBackend>,
}

impl MlxSwiftAdapter {
    /// An adapter that governs generations through `governor` and runs them on
    /// `backend`.
    pub fn new(governor: MemoryGovernor, backend: Arc<dyn MlxSwiftBackend>) -> Self {
        Self {
            id: RuntimeId::mlx_swift(),
            governor,
            backend,
        }
    }

    /// Parse the request's messages: a `complete` payload's prompt becomes a
    /// single user message; a `chat` payload's entries parse leniently,
    /// dropping any without a valid role. An empty list passes through — the
    /// backend owns the "needs a user message" rejection, as the Swift engine
    /// does.
    fn messages(
        payload: &JsonValue,
        capability: &Capability,
    ) -> Result<Vec<ChatMessage>, RuntimeError> {
        let JsonValue::Object(object) = payload else {
            return Err(RuntimeError::Failed(
                "chat payload must be an object".to_owned(),
            ));
        };
        if *capability == Capability::complete() {
            let Some(JsonValue::String(prompt)) = object.get("prompt") else {
                return Err(RuntimeError::Failed(
                    "complete payload must carry a prompt string".to_owned(),
                ));
            };
            return Ok(vec![ChatMessage::new(ChatRole::User, prompt.clone())]);
        }
        let Some(JsonValue::Array(entries)) = object.get("messages") else {
            return Err(RuntimeError::Failed(
                "chat payload must carry a messages array".to_owned(),
            ));
        };
        Ok(entries
            .iter()
            .filter_map(ChatMessage::from_payload)
            .collect())
    }

    /// The tools the request offers, parsed leniently: a malformed spec is
    /// dropped, not refused.
    fn tools(payload: &JsonValue) -> Vec<ToolSpec> {
        ToolSpec::from_payload_array(payload.as_object().and_then(|fields| fields.get("tools")))
    }

    /// Read the sampling options the MLX-Swift engine honors.
    fn options(payload: &JsonValue) -> MlxSwiftOptions {
        let object = payload.as_object();
        let get = |key: &str| object.and_then(|fields| fields.get(key));
        MlxSwiftOptions {
            temperature: get("temperature").and_then(JsonValue::as_f64),
            top_p: get("top_p").and_then(JsonValue::as_f64),
            repeat_penalty: get("repeat_penalty").and_then(JsonValue::as_f64),
            // A non-positive cap is treated as unset, so the engine's own
            // default applies — matching the Swift reference's `maxTokens > 0`.
            max_tokens: get("max_tokens")
                .and_then(JsonValue::as_i64)
                .filter(|&count| count > 0),
            stop: stop_strings(get("stop")),
        }
    }
}

/// The stop strings a request carries: a lone string, or the string entries of
/// an array; anything else yields none. Mirrors the Swift `StopMatcher.strings`.
fn stop_strings(value: Option<&JsonValue>) -> Vec<String> {
    match value {
        Some(JsonValue::String(single)) => vec![single.clone()],
        Some(JsonValue::Array(items)) => items
            .iter()
            .filter_map(|item| match item {
                JsonValue::String(text) => Some(text.clone()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

impl RuntimeAdapter for MlxSwiftAdapter {
    fn id(&self) -> &RuntimeId {
        &self.id
    }

    fn wires_tools(&self) -> bool {
        // The bridge renders the request's tools through the model's chat
        // template and captures the calls the model makes back out as chunks —
        // matching the mlx-lm sidecar that serves the same model set.
        true
    }

    fn can_serve(&self, record: &ModelRecord, capability: &Capability) -> bool {
        record.runtime.id.as_ref() == Some(&self.id)
            && (*capability == Capability::chat() || *capability == Capability::complete())
    }

    fn bid(&self, _record: &ModelRecord, identified: &IdentifiedModel) -> Option<RuntimeBid> {
        // Only bid when the in-process engine can actually run: an unavailable
        // bridge stays out of the auction so the Python mlx-lm sidecar
        // (BidPreference::MLX_LM, a higher number) wins the same model instead.
        // Since resolution picks a single runtime per model, the winner is the
        // only one that serves it — the two never double-serve.
        if !self.backend.is_available() {
            return None;
        }
        if identified.format == ModelFormat::MlxSafetensors
            && identified.modality == Some(kernel::records::Modality::text())
            && identified.capabilities.contains(&Capability::chat())
        {
            Some(RuntimeBid::new(RunTier::Native, BidPreference::MLX_SWIFT))
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
        let (tx, stream) = ChunkStream::channel();
        let messages = match Self::messages(&payload, &capability) {
            Ok(messages) => messages,
            Err(error) => {
                let _ = tx.send(Err(error));
                return stream;
            }
        };
        let tools = Self::tools(&payload);
        let options = Self::options(&payload);
        let model_dir = SidecarModelPaths::resolve(record).snapshot;
        let backend = Arc::clone(&self.backend);
        let governor = self.governor.clone();
        let record = record.clone();
        tokio::spawn(async move {
            let producer = GpuProducer::Generation(record.id.clone());
            let status = |line: &str| {
                let _ = tx.send(Ok(CapabilityChunk::Status(line.to_owned())));
            };
            let body = async {
                let started = Instant::now();
                let mut upstream = backend.stream(model_dir, messages, tools, options);
                while let Some(event) = upstream.recv().await {
                    match event {
                        Ok(MlxSwiftEvent::Text(text)) => {
                            if tx.send(Ok(CapabilityChunk::Text(text))).is_err() {
                                return Err(());
                            }
                        }
                        Ok(MlxSwiftEvent::Thinking(text)) => {
                            if tx.send(Ok(CapabilityChunk::Thinking(text))).is_err() {
                                return Err(());
                            }
                        }
                        Ok(MlxSwiftEvent::ToolCall(call)) => {
                            if tx.send(Ok(CapabilityChunk::ToolCall(call))).is_err() {
                                return Err(());
                            }
                        }
                        Ok(MlxSwiftEvent::Status(line)) => {
                            if tx.send(Ok(CapabilityChunk::Status(line))).is_err() {
                                return Err(());
                            }
                        }
                        Ok(MlxSwiftEvent::Done(done)) => {
                            let stats = GenerationStats {
                                prompt_tokens: done.prompt_tokens,
                                completion_tokens: done.completion_tokens,
                                duration_ms: Some(started.elapsed().as_millis() as i64),
                                load_ms: done.load_ms,
                                finish_reason: done.finish_reason,
                                token_counts_estimated: done.token_counts_estimated,
                                ..GenerationStats::default()
                            };
                            let _ = tx.send(Ok(CapabilityChunk::Done(Some(stats))));
                            // Done is terminal: the FFI backend's cancel watcher
                            // holds a sender past it, so break rather than wait
                            // for the channel to close.
                            return Ok(());
                        }
                        Err(error) => {
                            let _ = tx.send(Err(error));
                            return Err(());
                        }
                    }
                }
                Ok(())
            };
            let _: Result<(), ()> =
                governed_one_shot(&governor, &record, producer, &status, body).await;
        });
        stream
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
        if *capability != Capability::chat() && *capability != Capability::complete() {
            return HashSet::new();
        }
        [
            "temperature",
            "top_p",
            "max_tokens",
            "repeat_penalty",
            "stop",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::super::backend::{MissingMlxSwiftBackend, MlxSwiftDone, MlxSwiftEventStream};
    use super::*;
    use crate::adapters::RuntimeStream;
    use crate::governor::GovernorConfig;
    use kernel::capabilities::ToolCall;
    use kernel::records::{ExecutionMode, Modality, ModelSource, ModelState, SourceKind};

    type SeenRequest = (String, Vec<ChatMessage>, Vec<ToolSpec>, MlxSwiftOptions);

    struct FakeBackend {
        available: bool,
        events: std::sync::Mutex<Vec<Result<MlxSwiftEvent, RuntimeError>>>,
        seen: std::sync::Mutex<Option<SeenRequest>>,
    }

    impl FakeBackend {
        fn scripted(events: Vec<Result<MlxSwiftEvent, RuntimeError>>) -> Self {
            Self {
                available: true,
                events: std::sync::Mutex::new(events),
                seen: std::sync::Mutex::new(None),
            }
        }
    }

    impl MlxSwiftBackend for FakeBackend {
        fn is_available(&self) -> bool {
            self.available
        }

        fn stream(
            &self,
            model_dir: String,
            messages: Vec<ChatMessage>,
            tools: Vec<ToolSpec>,
            options: MlxSwiftOptions,
        ) -> MlxSwiftEventStream {
            *self.seen.lock().unwrap() = Some((model_dir, messages, tools, options));
            let (tx, stream) = RuntimeStream::channel();
            for event in std::mem::take(&mut *self.events.lock().unwrap()) {
                let _ = tx.send(event);
            }
            stream
        }
    }

    fn governor() -> MemoryGovernor {
        MemoryGovernor::new(GovernorConfig::with_total_mb(262_144))
    }

    fn adapter(backend: Arc<dyn MlxSwiftBackend>) -> MlxSwiftAdapter {
        MlxSwiftAdapter::new(governor(), backend)
    }

    fn record() -> ModelRecord {
        let mut record = ModelRecord::new(
            "qwen",
            Modality::text(),
            Vec::new(),
            ModelSource::new(SourceKind::folder(), "/models/qwen"),
        );
        record.runtime.id = Some(RuntimeId::mlx_swift());
        record.state = ModelState::Ready;
        record
    }

    fn chat_payload(fields: &[(&str, JsonValue)]) -> JsonValue {
        let mut object: BTreeMap<String, JsonValue> = BTreeMap::new();
        object.insert(
            "messages".to_owned(),
            JsonValue::Array(vec![JsonValue::Object(
                [
                    ("role".to_owned(), JsonValue::String("user".to_owned())),
                    ("content".to_owned(), JsonValue::String("hi".to_owned())),
                ]
                .into_iter()
                .collect(),
            )]),
        );
        for (key, value) in fields {
            object.insert((*key).to_owned(), value.clone());
        }
        JsonValue::Object(object)
    }

    fn identified(format: ModelFormat, capabilities: Vec<Capability>) -> IdentifiedModel {
        IdentifiedModel::new(
            format,
            Some(Modality::text()),
            capabilities,
            ExecutionMode::Stream,
        )
    }

    async fn collect(mut stream: ChunkStream) -> Vec<Result<CapabilityChunk, RuntimeError>> {
        let mut out = Vec::new();
        while let Some(item) = stream.recv().await {
            out.push(item);
        }
        out
    }

    #[test]
    fn it_serves_chat_and_complete_for_its_own_runtime() {
        let adapter = adapter(Arc::new(MissingMlxSwiftBackend));
        assert_eq!(adapter.id(), &RuntimeId::mlx_swift());
        assert!(adapter.can_serve(&record(), &Capability::chat()));
        assert!(adapter.can_serve(&record(), &Capability::complete()));
        assert!(!adapter.can_serve(&record(), &Capability::embed()));
        let mut other = record();
        other.runtime.id = Some(RuntimeId::mlx_lm());
        assert!(!adapter.can_serve(&other, &Capability::chat()));
    }

    #[test]
    fn it_wires_tools() {
        assert!(adapter(Arc::new(MissingMlxSwiftBackend)).wires_tools());
    }

    #[test]
    fn it_bids_native_on_mlx_text_chat_only_when_available() {
        let adapter = adapter(Arc::new(FakeBackend::scripted(vec![])));
        assert_eq!(
            adapter.bid(
                &record(),
                &identified(ModelFormat::MlxSafetensors, vec![Capability::chat()])
            ),
            Some(RuntimeBid::new(RunTier::Native, BidPreference::MLX_SWIFT))
        );
        // Non-MLX formats and non-chat capability sets do not bid.
        assert!(
            adapter
                .bid(
                    &record(),
                    &identified(ModelFormat::Gguf, vec![Capability::chat()])
                )
                .is_none()
        );
        assert!(
            adapter
                .bid(
                    &record(),
                    &identified(ModelFormat::MlxSafetensors, vec![Capability::embed()])
                )
                .is_none()
        );
    }

    // The whole yield-to-sidecar design rests on mlx-swift outranking the
    // Python mlx-lm sidecar (lower preference wins), pinned at compile time.
    const _: () = assert!(BidPreference::MLX_SWIFT < BidPreference::MLX_LM);

    #[test]
    fn an_unavailable_backend_yields_to_the_sidecar_by_not_bidding() {
        // The missing backend reports unavailable, so mlx-swift stays out of
        // the auction and the Python mlx-lm sidecar (a higher preference
        // number) wins the same model.
        let adapter = adapter(Arc::new(MissingMlxSwiftBackend));
        assert!(
            adapter
                .bid(
                    &record(),
                    &identified(ModelFormat::MlxSafetensors, vec![Capability::chat()])
                )
                .is_none()
        );
    }

    #[test]
    fn it_honors_the_mlx_swift_sampling_keys_for_text_only() {
        let adapter = adapter(Arc::new(MissingMlxSwiftBackend));
        let honored = adapter.honored_param_keys(&record(), &Capability::chat());
        for key in [
            "temperature",
            "top_p",
            "max_tokens",
            "repeat_penalty",
            "stop",
        ] {
            assert!(honored.contains(key), "missing {key}");
        }
        // Unlike mlx-lm, mlx-swift does not honor top_k/min_p/seed.
        assert!(!honored.contains("top_k"));
        assert!(!honored.contains("seed"));
        assert!(
            adapter
                .honored_param_keys(&record(), &Capability::speak())
                .is_empty()
        );
    }

    #[test]
    fn effective_context_window_is_the_records_length() {
        let adapter = adapter(Arc::new(MissingMlxSwiftBackend));
        let mut record = record();
        record.context_length = Some(8192);
        assert_eq!(adapter.effective_context_window(&record, None), Some(8192));
    }

    #[tokio::test]
    async fn events_forward_as_chunks_and_done_carries_stats() {
        let backend = Arc::new(FakeBackend::scripted(vec![
            Ok(MlxSwiftEvent::Status("Loading…".to_owned())),
            Ok(MlxSwiftEvent::Thinking("hmm".to_owned())),
            Ok(MlxSwiftEvent::Text("Hello ".to_owned())),
            Ok(MlxSwiftEvent::Text("world".to_owned())),
            Ok(MlxSwiftEvent::Done(MlxSwiftDone {
                prompt_tokens: Some(7),
                completion_tokens: Some(3),
                load_ms: Some(50),
                finish_reason: Some("stop".to_owned()),
                token_counts_estimated: false,
            })),
        ]));
        let adapter = adapter(backend);
        let chunks = collect(adapter.invoke(&record(), Capability::chat(), chat_payload(&[])))
            .await
            .into_iter()
            .map(Result::unwrap)
            .collect::<Vec<_>>();
        let text: String = chunks
            .iter()
            .filter_map(|chunk| match chunk {
                CapabilityChunk::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(text, "Hello world");
        assert!(chunks.contains(&CapabilityChunk::Thinking("hmm".to_owned())));
        assert!(chunks.contains(&CapabilityChunk::Status("Loading…".to_owned())));
        let Some(CapabilityChunk::Done(Some(stats))) = chunks.last() else {
            panic!("expected done stats, got {:?}", chunks.last());
        };
        assert_eq!(stats.prompt_tokens, Some(7));
        assert_eq!(stats.completion_tokens, Some(3));
        assert_eq!(stats.load_ms, Some(50));
        assert_eq!(stats.finish_reason.as_deref(), Some("stop"));
        assert!(stats.duration_ms.is_some());
    }

    #[tokio::test]
    async fn a_captured_tool_call_streams_as_a_chunk_before_done() {
        let call = ToolCall::with_id("call_1", "read", JsonValue::Object(BTreeMap::new()));
        let backend = Arc::new(FakeBackend::scripted(vec![
            Ok(MlxSwiftEvent::Text("Let me check.".to_owned())),
            Ok(MlxSwiftEvent::ToolCall(call.clone())),
            Ok(MlxSwiftEvent::Done(MlxSwiftDone::default())),
        ]));
        let adapter = adapter(backend);
        let chunks = collect(adapter.invoke(&record(), Capability::chat(), chat_payload(&[])))
            .await
            .into_iter()
            .map(Result::unwrap)
            .collect::<Vec<_>>();
        assert!(chunks.contains(&CapabilityChunk::ToolCall(call)));
        assert!(matches!(chunks.last(), Some(CapabilityChunk::Done(_))));
    }

    #[tokio::test]
    async fn the_request_reaches_the_backend_with_the_model_dir_tools_and_options() {
        let backend = Arc::new(FakeBackend::scripted(vec![]));
        let adapter = adapter(Arc::clone(&backend) as Arc<dyn MlxSwiftBackend>);
        let spec = JsonValue::Object(
            [("name".to_owned(), JsonValue::String("read".to_owned()))]
                .into_iter()
                .collect(),
        );
        let payload = chat_payload(&[
            ("temperature", JsonValue::Double(0.2)),
            ("repeat_penalty", JsonValue::Double(1.1)),
            ("max_tokens", JsonValue::Int(64)),
            (
                "stop",
                JsonValue::Array(vec![JsonValue::String("<end>".to_owned())]),
            ),
            ("tools", JsonValue::Array(vec![spec])),
        ]);
        collect(adapter.invoke(&record(), Capability::chat(), payload)).await;
        let seen = backend.seen.lock().unwrap().clone();
        let (model_dir, messages, tools, options) = seen.expect("backend saw the request");
        assert!(model_dir.ends_with("qwen"));
        assert_eq!(messages.len(), 1);
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "read");
        assert_eq!(options.temperature, Some(0.2));
        assert_eq!(options.repeat_penalty, Some(1.1));
        assert_eq!(options.max_tokens, Some(64));
        assert_eq!(options.stop, vec!["<end>".to_owned()]);
    }

    #[test]
    fn a_non_positive_max_tokens_is_treated_as_unset() {
        // Matches the Swift reference's `maxTokens > 0` guard: a 0/negative cap
        // falls back to the engine default rather than reaching the shim.
        let zero = chat_payload(&[("max_tokens", JsonValue::Int(0))]);
        assert_eq!(MlxSwiftAdapter::options(&zero).max_tokens, None);
        let negative = chat_payload(&[("max_tokens", JsonValue::Int(-5))]);
        assert_eq!(MlxSwiftAdapter::options(&negative).max_tokens, None);
        let positive = chat_payload(&[("max_tokens", JsonValue::Int(128))]);
        assert_eq!(MlxSwiftAdapter::options(&positive).max_tokens, Some(128));
    }

    #[tokio::test]
    async fn a_complete_payload_wraps_the_prompt_as_a_user_message() {
        let backend = Arc::new(FakeBackend::scripted(vec![]));
        let adapter = adapter(Arc::clone(&backend) as Arc<dyn MlxSwiftBackend>);
        let payload = JsonValue::Object(
            [(
                "prompt".to_owned(),
                JsonValue::String("finish this".to_owned()),
            )]
            .into_iter()
            .collect(),
        );
        collect(adapter.invoke(&record(), Capability::complete(), payload)).await;
        let seen = backend.seen.lock().unwrap().clone();
        let (_, messages, _, _) = seen.expect("backend saw the request");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, ChatRole::User);
        assert_eq!(messages[0].content, "finish this");
    }

    #[tokio::test]
    async fn a_payload_without_messages_or_prompt_errors() {
        let adapter = adapter(Arc::new(MissingMlxSwiftBackend));
        let empty = JsonValue::Object(BTreeMap::new());
        let results = collect(adapter.invoke(&record(), Capability::chat(), empty.clone())).await;
        assert!(matches!(
            results.first(),
            Some(Err(RuntimeError::Failed(_)))
        ));
        let results = collect(adapter.invoke(&record(), Capability::complete(), empty)).await;
        assert!(matches!(
            results.first(),
            Some(Err(RuntimeError::Failed(_)))
        ));
    }

    #[tokio::test]
    async fn the_missing_backend_reports_the_runtime_unavailable() {
        let adapter = adapter(Arc::new(MissingMlxSwiftBackend));
        let results =
            collect(adapter.invoke(&record(), Capability::chat(), chat_payload(&[]))).await;
        assert!(matches!(
            results.first(),
            Some(Err(RuntimeError::Unavailable(_)))
        ));
    }

    #[tokio::test]
    async fn done_ends_the_stream_even_while_the_backend_holds_a_sender() {
        // Reproduces the FFI backend's shape: its cancel watcher keeps a sender
        // alive after the terminal event, so the adapter must end on Done rather
        // than waiting for the channel to close.
        struct Holding {
            kept: std::sync::Mutex<
                Option<tokio::sync::mpsc::UnboundedSender<Result<MlxSwiftEvent, RuntimeError>>>,
            >,
        }

        impl MlxSwiftBackend for Holding {
            fn is_available(&self) -> bool {
                true
            }

            fn stream(
                &self,
                _model_dir: String,
                _messages: Vec<ChatMessage>,
                _tools: Vec<ToolSpec>,
                _options: MlxSwiftOptions,
            ) -> MlxSwiftEventStream {
                let (tx, stream) = RuntimeStream::channel();
                let _ = tx.send(Ok(MlxSwiftEvent::Text("hi".to_owned())));
                let _ = tx.send(Ok(MlxSwiftEvent::Done(MlxSwiftDone::default())));
                *self.kept.lock().unwrap() = Some(tx);
                stream
            }
        }

        let adapter = adapter(Arc::new(Holding {
            kept: std::sync::Mutex::new(None),
        }));
        let collected = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            collect(adapter.invoke(&record(), Capability::chat(), chat_payload(&[]))),
        )
        .await
        .expect("the stream must end on the done event");
        assert!(matches!(
            collected.last(),
            Some(Ok(CapabilityChunk::Done(Some(_))))
        ));
    }
}
