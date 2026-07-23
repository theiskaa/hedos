//! The Apple-Foundation adapter: serves chat and completion through Apple's
//! on-device model, ported from the Swift kernel's `AppleFoundationAdapter`.
//! The backend emits whole-text snapshots; the adapter turns them into deltas.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

use kernel::capabilities::{CapabilityChunk, ChatMessage, ChatRole, GenerationStats, ToolSpec};
use kernel::records::{BidPreference, Capability, JsonValue, ModelRecord, RunTier, RuntimeId};
use kernel::resolution::{IdentifiedModel, ModelFormat, RuntimeBid};

use super::backend::{AppleFoundationBackend, BuiltinEvent, BuiltinOptions};
use crate::adapters::{ChunkStream, RuntimeAdapter, RuntimeError};

/// The adapter over a bridge to Apple's on-device model.
pub struct AppleFoundationAdapter {
    id: RuntimeId,
    backend: Arc<dyn AppleFoundationBackend>,
}

impl AppleFoundationAdapter {
    /// An adapter over `backend`.
    pub fn new(backend: Arc<dyn AppleFoundationBackend>) -> Self {
        Self {
            id: RuntimeId::apple_foundation(),
            backend,
        }
    }

    /// The text `current` adds over `previous`, or empty when it does not
    /// extend it (the model occasionally rewrites a snapshot; such steps are
    /// skipped rather than re-emitted).
    fn delta<'a>(previous: &str, current: &'a str) -> &'a str {
        current.strip_prefix(previous).unwrap_or("")
    }

    /// Parse the request's messages: a `complete` payload's prompt becomes a
    /// single user message; a `chat` payload's entries parse leniently,
    /// dropping any without a valid role. An empty list passes through — the
    /// backend owns the "needs a user message" rejection, as the Swift bridge
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

    /// Read the sampling options the model honors; both `top_p` and `top_k` at
    /// once is refused because Apple's sampler takes one or the other.
    fn options(payload: &JsonValue) -> Result<BuiltinOptions, RuntimeError> {
        let object = payload.as_object();
        let get = |key: &str| object.and_then(|fields| fields.get(key));
        let options = BuiltinOptions {
            temperature: get("temperature").and_then(JsonValue::as_f64),
            top_p: get("top_p").and_then(JsonValue::as_f64),
            top_k: get("top_k").and_then(JsonValue::as_i64),
            seed: get("seed")
                .and_then(JsonValue::as_i64)
                .map(|seed| seed as u64),
            max_tokens: get("max_tokens").and_then(JsonValue::as_i64),
        };
        if options.top_p.is_some() && options.top_k.is_some() {
            return Err(RuntimeError::Failed(
                "Apple Intelligence honors either top_p or top_k, not both".to_owned(),
            ));
        }
        Ok(options)
    }
}

impl RuntimeAdapter for AppleFoundationAdapter {
    fn id(&self) -> &RuntimeId {
        &self.id
    }

    fn wires_tools(&self) -> bool {
        // The bridge offers the request's tools to the session and captures
        // the calls the model makes back out as chunks.
        true
    }

    fn can_serve(&self, record: &ModelRecord, capability: &Capability) -> bool {
        record.runtime.id.as_ref() == Some(&self.id)
            && (*capability == Capability::chat() || *capability == Capability::complete())
    }

    fn bid(&self, _record: &ModelRecord, identified: &IdentifiedModel) -> Option<RuntimeBid> {
        if identified.format != ModelFormat::Builtin {
            return None;
        }
        Some(RuntimeBid::new(
            RunTier::Native,
            BidPreference::APPLE_FOUNDATION,
        ))
    }

    fn invoke(
        &self,
        _record: &ModelRecord,
        capability: Capability,
        payload: JsonValue,
    ) -> ChunkStream {
        let (tx, stream) = ChunkStream::channel();
        let parsed = Self::messages(&payload, &capability)
            .and_then(|messages| Self::options(&payload).map(|options| (messages, options)));
        let (messages, options) = match parsed {
            Ok(parts) => parts,
            Err(error) => {
                let _ = tx.send(Err(error));
                return stream;
            }
        };
        let tools = Self::tools(&payload);
        let backend = Arc::clone(&self.backend);
        tokio::spawn(async move {
            let started = Instant::now();
            let mut upstream = backend.stream(messages, tools, options);
            let mut previous = String::new();
            let mut prompt_tokens = None;
            let mut completion_tokens = None;
            while let Some(event) = upstream.recv().await {
                match event {
                    Ok(BuiltinEvent::Snapshot(current)) => {
                        let delta = Self::delta(&previous, &current);
                        if !delta.is_empty()
                            && tx
                                .send(Ok(CapabilityChunk::Text(delta.to_owned())))
                                .is_err()
                        {
                            return;
                        }
                        previous = current;
                    }
                    Ok(BuiltinEvent::ToolCall(call)) => {
                        if tx.send(Ok(CapabilityChunk::ToolCall(call))).is_err() {
                            return;
                        }
                    }
                    Ok(BuiltinEvent::Done {
                        prompt_tokens: prompt,
                        completion_tokens: completion,
                    }) => {
                        prompt_tokens = prompt;
                        completion_tokens = completion;
                        // Done is the terminal event: waiting for the channel
                        // to close instead would hang against a backend whose
                        // internals still hold a sender (the FFI backend's
                        // cancel watcher does).
                        break;
                    }
                    Err(error) => {
                        let _ = tx.send(Err(error));
                        return;
                    }
                }
            }
            let stats = GenerationStats {
                prompt_tokens,
                completion_tokens,
                duration_ms: Some(started.elapsed().as_millis() as i64),
                token_counts_estimated: true,
                ..GenerationStats::default()
            };
            let _ = tx.send(Ok(CapabilityChunk::Done(Some(stats))));
        });
        stream
    }

    fn honored_param_keys(
        &self,
        _record: &ModelRecord,
        capability: &Capability,
    ) -> HashSet<String> {
        if *capability != Capability::chat() && *capability != Capability::complete() {
            return HashSet::new();
        }
        ["temperature", "max_tokens", "top_p", "top_k", "seed"]
            .into_iter()
            .map(str::to_owned)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::super::backend::{BuiltinAvailability, BuiltinEventStream, MissingAppleBackend};
    use super::*;
    use crate::adapters::RuntimeStream;
    use kernel::capabilities::ToolCall;
    use kernel::records::{ExecutionMode, Modality, ModelSource, ModelState, SourceKind};

    type SeenRequest = (Vec<ChatMessage>, Vec<ToolSpec>, BuiltinOptions);

    /// A backend that replays a scripted event sequence (once) and records the
    /// request it saw.
    struct FakeBackend {
        events: std::sync::Mutex<Vec<Result<BuiltinEvent, RuntimeError>>>,
        seen: std::sync::Mutex<Option<SeenRequest>>,
    }

    impl FakeBackend {
        fn scripted(events: Vec<Result<BuiltinEvent, RuntimeError>>) -> Self {
            Self {
                events: std::sync::Mutex::new(events),
                seen: std::sync::Mutex::new(None),
            }
        }
    }

    impl AppleFoundationBackend for FakeBackend {
        fn availability(&self) -> BuiltinAvailability {
            BuiltinAvailability::Available
        }

        fn stream(
            &self,
            messages: Vec<ChatMessage>,
            tools: Vec<ToolSpec>,
            options: BuiltinOptions,
        ) -> BuiltinEventStream {
            *self.seen.lock().unwrap() = Some((messages, tools, options));
            let (tx, stream) = RuntimeStream::channel();
            for event in std::mem::take(&mut *self.events.lock().unwrap()) {
                let _ = tx.send(event);
            }
            stream
        }
    }

    fn record() -> ModelRecord {
        let mut record = ModelRecord::new(
            "apple",
            Modality::text(),
            Vec::new(),
            ModelSource::new(SourceKind::builtin(), "/System/x"),
        );
        record.runtime.id = Some(RuntimeId::apple_foundation());
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

    async fn collect(mut stream: ChunkStream) -> Vec<Result<CapabilityChunk, RuntimeError>> {
        let mut out = Vec::new();
        while let Some(item) = stream.recv().await {
            out.push(item);
        }
        out
    }

    #[test]
    fn it_serves_chat_and_complete_for_its_own_runtime() {
        let adapter = AppleFoundationAdapter::new(Arc::new(MissingAppleBackend));
        assert_eq!(adapter.id(), &RuntimeId::apple_foundation());
        assert!(adapter.can_serve(&record(), &Capability::chat()));
        assert!(adapter.can_serve(&record(), &Capability::complete()));
        assert!(!adapter.can_serve(&record(), &Capability::embed()));
        let mut other = record();
        other.runtime.id = Some(RuntimeId::ollama());
        assert!(!adapter.can_serve(&other, &Capability::chat()));
    }

    #[test]
    fn it_wires_tools() {
        // Keeps the `tools` capability fold granting tools to the builtin
        // model (resolution::fold_tool_capability keys on this).
        assert!(AppleFoundationAdapter::new(Arc::new(MissingAppleBackend)).wires_tools());
    }

    #[test]
    fn it_bids_native_only_on_builtin_models() {
        let adapter = AppleFoundationAdapter::new(Arc::new(MissingAppleBackend));
        let builtin = IdentifiedModel::new(
            ModelFormat::Builtin,
            Some(Modality::text()),
            vec![Capability::chat()],
            ExecutionMode::Stream,
        );
        assert_eq!(
            adapter.bid(&record(), &builtin),
            Some(RuntimeBid::new(
                RunTier::Native,
                BidPreference::APPLE_FOUNDATION
            ))
        );
        let gguf = IdentifiedModel::new(
            ModelFormat::Gguf,
            Some(Modality::text()),
            vec![Capability::chat()],
            ExecutionMode::Stream,
        );
        assert!(adapter.bid(&record(), &gguf).is_none());
    }

    #[test]
    fn it_honors_apples_sampling_keys_for_text_only() {
        let adapter = AppleFoundationAdapter::new(Arc::new(MissingAppleBackend));
        let honored = adapter.honored_param_keys(&record(), &Capability::chat());
        for key in ["temperature", "max_tokens", "top_p", "top_k", "seed"] {
            assert!(honored.contains(key), "missing {key}");
        }
        assert!(
            adapter
                .honored_param_keys(&record(), &Capability::speak())
                .is_empty()
        );
    }

    #[test]
    fn delta_extends_and_skips_rewrites() {
        assert_eq!(AppleFoundationAdapter::delta("", "Hel"), "Hel");
        assert_eq!(AppleFoundationAdapter::delta("Hel", "Hello w"), "lo w");
        assert_eq!(AppleFoundationAdapter::delta("Hello w", "Goodbye"), "");
    }

    #[tokio::test]
    async fn snapshots_stream_as_deltas_and_done_carries_stats() {
        let backend = Arc::new(FakeBackend::scripted(vec![
            Ok(BuiltinEvent::Snapshot("Hel".to_owned())),
            Ok(BuiltinEvent::Snapshot("Hello w".to_owned())),
            Ok(BuiltinEvent::Snapshot("Hello world".to_owned())),
            Ok(BuiltinEvent::Done {
                prompt_tokens: Some(7),
                completion_tokens: Some(3),
            }),
        ]));
        let adapter = AppleFoundationAdapter::new(backend);
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
        let Some(CapabilityChunk::Done(Some(stats))) = chunks.last() else {
            panic!("expected done stats, got {:?}", chunks.last());
        };
        assert_eq!(stats.prompt_tokens, Some(7));
        assert_eq!(stats.completion_tokens, Some(3));
        assert!(stats.token_counts_estimated);
        assert!(stats.duration_ms.is_some());
    }

    #[tokio::test]
    async fn a_captured_tool_call_streams_as_a_chunk_before_done() {
        let call = ToolCall::with_id("call_1", "read", JsonValue::Object(BTreeMap::new()));
        let backend = Arc::new(FakeBackend::scripted(vec![
            Ok(BuiltinEvent::Snapshot("Let me check.".to_owned())),
            Ok(BuiltinEvent::ToolCall(call.clone())),
            Ok(BuiltinEvent::Done {
                prompt_tokens: None,
                completion_tokens: None,
            }),
        ]));
        let adapter = AppleFoundationAdapter::new(backend);
        let chunks = collect(adapter.invoke(&record(), Capability::chat(), chat_payload(&[])))
            .await
            .into_iter()
            .map(Result::unwrap)
            .collect::<Vec<_>>();
        assert!(chunks.contains(&CapabilityChunk::ToolCall(call)));
        assert!(matches!(chunks.last(), Some(CapabilityChunk::Done(_))));
    }

    #[tokio::test]
    async fn the_requests_tools_and_tool_history_reach_the_backend() {
        let backend = Arc::new(FakeBackend::scripted(vec![]));
        let adapter =
            AppleFoundationAdapter::new(Arc::clone(&backend) as Arc<dyn AppleFoundationBackend>);
        let tool_call = JsonValue::Object(
            [
                ("id".to_owned(), JsonValue::String("call_1".to_owned())),
                ("name".to_owned(), JsonValue::String("read".to_owned())),
                ("arguments".to_owned(), JsonValue::Object(BTreeMap::new())),
            ]
            .into_iter()
            .collect(),
        );
        let messages = JsonValue::Array(vec![
            JsonValue::Object(
                [
                    ("role".to_owned(), JsonValue::String("assistant".to_owned())),
                    ("content".to_owned(), JsonValue::String(String::new())),
                    ("tool_calls".to_owned(), JsonValue::Array(vec![tool_call])),
                ]
                .into_iter()
                .collect(),
            ),
            JsonValue::Object(
                [
                    ("role".to_owned(), JsonValue::String("tool".to_owned())),
                    (
                        "content".to_owned(),
                        JsonValue::String("file text".to_owned()),
                    ),
                    (
                        "tool_call_id".to_owned(),
                        JsonValue::String("call_1".to_owned()),
                    ),
                    ("tool_name".to_owned(), JsonValue::String("read".to_owned())),
                ]
                .into_iter()
                .collect(),
            ),
            JsonValue::Object(
                [
                    ("role".to_owned(), JsonValue::String("user".to_owned())),
                    ("content".to_owned(), JsonValue::String("go on".to_owned())),
                ]
                .into_iter()
                .collect(),
            ),
        ]);
        let spec = JsonValue::Object(
            [
                ("name".to_owned(), JsonValue::String("read".to_owned())),
                (
                    "description".to_owned(),
                    JsonValue::String("read a file".to_owned()),
                ),
                ("parameters".to_owned(), JsonValue::Object(BTreeMap::new())),
            ]
            .into_iter()
            .collect(),
        );
        let payload = JsonValue::Object(
            [
                ("messages".to_owned(), messages),
                ("tools".to_owned(), JsonValue::Array(vec![spec])),
            ]
            .into_iter()
            .collect(),
        );
        collect(adapter.invoke(&record(), Capability::chat(), payload)).await;
        let seen = backend.seen.lock().unwrap().clone();
        let (messages, tools, _) = seen.expect("backend saw the request");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "read");
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].tool_calls.len(), 1);
        assert_eq!(messages[0].tool_calls[0].id, "call_1");
        assert_eq!(messages[1].role, ChatRole::Tool);
        assert_eq!(messages[1].tool_call_id.as_deref(), Some("call_1"));
        assert_eq!(messages[1].tool_name.as_deref(), Some("read"));
    }

    #[tokio::test]
    async fn a_rewritten_snapshot_is_skipped_not_reemitted() {
        let backend = Arc::new(FakeBackend::scripted(vec![
            Ok(BuiltinEvent::Snapshot("draft".to_owned())),
            Ok(BuiltinEvent::Snapshot("other".to_owned())),
        ]));
        let adapter = AppleFoundationAdapter::new(backend);
        let chunks = collect(adapter.invoke(&record(), Capability::chat(), chat_payload(&[])))
            .await
            .into_iter()
            .map(Result::unwrap)
            .collect::<Vec<_>>();
        let texts: Vec<_> = chunks
            .iter()
            .filter_map(|chunk| match chunk {
                CapabilityChunk::Text(text) => Some(text.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(texts, vec!["draft".to_owned()]);
    }

    #[tokio::test]
    async fn top_p_and_top_k_together_are_refused() {
        let adapter = AppleFoundationAdapter::new(Arc::new(MissingAppleBackend));
        let payload = chat_payload(&[
            ("top_p", JsonValue::Double(0.9)),
            ("top_k", JsonValue::Int(40)),
        ]);
        let results = collect(adapter.invoke(&record(), Capability::chat(), payload)).await;
        let Some(Err(RuntimeError::Failed(message))) = results.first() else {
            panic!("expected a refusal, got {:?}", results.first());
        };
        assert!(message.contains("either top_p or top_k"));
    }

    #[tokio::test]
    async fn the_sampling_options_reach_the_backend() {
        let backend = Arc::new(FakeBackend::scripted(vec![]));
        let adapter =
            AppleFoundationAdapter::new(Arc::clone(&backend) as Arc<dyn AppleFoundationBackend>);
        let payload = chat_payload(&[
            ("temperature", JsonValue::Double(0.2)),
            ("top_k", JsonValue::Int(5)),
            ("seed", JsonValue::Int(11)),
            ("max_tokens", JsonValue::Int(64)),
        ]);
        collect(adapter.invoke(&record(), Capability::chat(), payload)).await;
        let seen = backend.seen.lock().unwrap().clone();
        let (messages, _, options) = seen.expect("backend saw the request");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content, "hi");
        assert_eq!(options.temperature, Some(0.2));
        assert_eq!(options.top_k, Some(5));
        assert_eq!(options.seed, Some(11));
        assert_eq!(options.max_tokens, Some(64));
        assert_eq!(options.top_p, None);
    }

    #[tokio::test]
    async fn tools_on_a_complete_payload_still_reach_the_backend() {
        // The gateway never builds a complete payload with tools, but the
        // adapter forwards them uniformly rather than special-casing the
        // capability — this pins that as intended.
        let backend = Arc::new(FakeBackend::scripted(vec![]));
        let adapter =
            AppleFoundationAdapter::new(Arc::clone(&backend) as Arc<dyn AppleFoundationBackend>);
        let spec = JsonValue::Object(
            [("name".to_owned(), JsonValue::String("read".to_owned()))]
                .into_iter()
                .collect(),
        );
        let payload = JsonValue::Object(
            [
                (
                    "prompt".to_owned(),
                    JsonValue::String("finish this".to_owned()),
                ),
                ("tools".to_owned(), JsonValue::Array(vec![spec])),
            ]
            .into_iter()
            .collect(),
        );
        collect(adapter.invoke(&record(), Capability::complete(), payload)).await;
        let seen = backend.seen.lock().unwrap().clone();
        let (_, tools, _) = seen.expect("backend saw the request");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "read");
    }

    #[tokio::test]
    async fn a_complete_payload_wraps_the_prompt_as_a_user_message() {
        let backend = Arc::new(FakeBackend::scripted(vec![]));
        let adapter =
            AppleFoundationAdapter::new(Arc::clone(&backend) as Arc<dyn AppleFoundationBackend>);
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
        let (messages, _, _) = seen.expect("backend saw the request");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, ChatRole::User);
        assert_eq!(messages[0].content, "finish this");
    }

    #[tokio::test]
    async fn a_payload_without_messages_or_prompt_errors() {
        let adapter = AppleFoundationAdapter::new(Arc::new(MissingAppleBackend));
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
    async fn done_ends_the_stream_even_while_the_backend_holds_a_sender() {
        // Reproduces the FFI backend's shape: its cancel watcher keeps a
        // sender alive after the terminal event, so the adapter must end on
        // Done rather than waiting for the channel to close.
        struct Holding {
            kept: std::sync::Mutex<
                Option<tokio::sync::mpsc::UnboundedSender<Result<BuiltinEvent, RuntimeError>>>,
            >,
        }

        impl AppleFoundationBackend for Holding {
            fn availability(&self) -> BuiltinAvailability {
                BuiltinAvailability::Available
            }

            fn stream(
                &self,
                _messages: Vec<ChatMessage>,
                _tools: Vec<ToolSpec>,
                _options: BuiltinOptions,
            ) -> BuiltinEventStream {
                let (tx, stream) = RuntimeStream::channel();
                let _ = tx.send(Ok(BuiltinEvent::Snapshot("hi".to_owned())));
                let _ = tx.send(Ok(BuiltinEvent::Done {
                    prompt_tokens: None,
                    completion_tokens: None,
                }));
                *self.kept.lock().unwrap() = Some(tx);
                stream
            }
        }

        let adapter = AppleFoundationAdapter::new(Arc::new(Holding {
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

    #[tokio::test]
    async fn an_empty_messages_array_reaches_the_backend_as_zero_messages() {
        // The "needs a user message" rejection belongs to the backend (the
        // Swift bridge's message split performs it), so the adapter forwards
        // an empty list rather than second-guessing it.
        let backend = Arc::new(FakeBackend::scripted(vec![]));
        let adapter =
            AppleFoundationAdapter::new(Arc::clone(&backend) as Arc<dyn AppleFoundationBackend>);
        let payload = JsonValue::Object(
            [("messages".to_owned(), JsonValue::Array(Vec::new()))]
                .into_iter()
                .collect(),
        );
        collect(adapter.invoke(&record(), Capability::chat(), payload)).await;
        let seen = backend.seen.lock().unwrap().clone();
        let (messages, _, _) = seen.expect("backend saw the request");
        assert!(messages.is_empty());
    }

    #[tokio::test]
    async fn the_missing_backend_reports_the_runtime_unavailable() {
        let adapter = AppleFoundationAdapter::new(Arc::new(MissingAppleBackend));
        assert_eq!(
            MissingAppleBackend.availability(),
            BuiltinAvailability::NotEligible
        );
        let results =
            collect(adapter.invoke(&record(), Capability::chat(), chat_payload(&[]))).await;
        assert!(matches!(
            results.first(),
            Some(Err(RuntimeError::Unavailable(_)))
        ));
    }
}
