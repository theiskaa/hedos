//! The whisper.cpp runtime adapter: it serves the `transcribe` capability for
//! GGUF / GGML models by decoding the request's audio and driving the governed
//! whisper engine.

use kernel::records::{BidPreference, Capability, JsonValue, ModelRecord, RunTier, RuntimeId};
use kernel::resolution::{IdentifiedModel, ModelFormat, RuntimeBid};

use super::audio::TranscriptionAudio;
use super::engine::{EXPECTED_SAMPLE_RATE, TranscriptionJob, WhisperEngine};
use super::expand_tilde;
use super::options::TranscriptionOptions;
use crate::adapters::{ChunkStream, RuntimeAdapter, RuntimeError, RuntimeStream};
use crate::governor::MemoryGovernor;

/// The whisper.cpp transcription adapter.
pub struct WhisperCppAdapter {
    id: RuntimeId,
    governor: MemoryGovernor,
    engine: WhisperEngine,
}

impl WhisperCppAdapter {
    /// An adapter governing generations through `governor` and transcribing
    /// through `engine`.
    pub fn new(governor: MemoryGovernor, engine: WhisperEngine) -> Self {
        Self {
            id: RuntimeId::whisper_cpp(),
            governor,
            engine,
        }
    }

    /// The transcription options a payload requests: a non-empty forced language
    /// and the translate flag.
    fn options(payload: &JsonValue) -> TranscriptionOptions {
        let Some(fields) = payload.as_object() else {
            return TranscriptionOptions::default();
        };
        let language = fields
            .get("language")
            .and_then(JsonValue::as_str)
            .filter(|language| !language.is_empty())
            .map(str::to_owned);
        let translate = fields
            .get("translate")
            .and_then(JsonValue::as_bool)
            .unwrap_or(false);
        TranscriptionOptions {
            language,
            translate,
        }
    }
}

impl RuntimeAdapter for WhisperCppAdapter {
    fn id(&self) -> &RuntimeId {
        &self.id
    }

    fn can_serve(&self, record: &ModelRecord, capability: &Capability) -> bool {
        capability == &Capability::transcribe() && record.runtime.id.as_ref() == Some(&self.id)
    }

    fn bid(&self, _record: &ModelRecord, identified: &IdentifiedModel) -> Option<RuntimeBid> {
        if matches!(identified.format, ModelFormat::Gguf | ModelFormat::GgmlBin)
            && identified.capabilities.contains(&Capability::transcribe())
        {
            Some(RuntimeBid::new(
                RunTier::Managed,
                BidPreference::WHISPER_CPP,
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
        let audio = match TranscriptionAudio::from(&payload) {
            Ok(audio) => audio,
            Err(error) => return RuntimeStream::failed(RuntimeError::Failed(error.to_string())),
        };
        let samples = audio.mono_samples(EXPECTED_SAMPLE_RATE);
        if samples.is_empty() {
            return RuntimeStream::failed(RuntimeError::Failed(
                "transcribe payload carries no audio".to_owned(),
            ));
        }
        let path = expand_tilde(
            record
                .primary_weight_path
                .as_deref()
                .unwrap_or(&record.source.path),
        );
        self.engine.run(
            self.governor.clone(),
            TranscriptionJob {
                model_id: record.id.clone(),
                model_name: record.name.clone(),
                path,
                footprint_mb: record.footprint_mb,
                samples,
                options: Self::options(&payload),
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::super::backend::MissingWhisperBackend;
    use super::*;
    use crate::governor::GovernorConfig;
    use kernel::records::{ExecutionMode, Modality, ModelSource, ModelState, SourceKind};
    use std::sync::Arc;

    fn adapter() -> WhisperCppAdapter {
        let engine = WhisperEngine::new(Arc::new(MissingWhisperBackend));
        WhisperCppAdapter::new(
            MemoryGovernor::new(GovernorConfig::with_total_mb(262_144)),
            engine,
        )
    }

    fn record() -> ModelRecord {
        let mut record = ModelRecord::new(
            "w",
            Modality::audio(),
            Vec::new(),
            ModelSource::new(SourceKind::folder(), "/models/whisper.bin"),
        );
        record.runtime.id = Some(RuntimeId::whisper_cpp());
        record.state = ModelState::Ready;
        record
    }

    fn identified(format: ModelFormat, caps: Vec<Capability>) -> IdentifiedModel {
        IdentifiedModel::new(format, Some(Modality::audio()), caps, ExecutionMode::Sync)
    }

    #[test]
    fn it_serves_only_transcribe_for_its_own_runtime() {
        let adapter = adapter();
        assert_eq!(adapter.id(), &RuntimeId::whisper_cpp());
        assert!(adapter.can_serve(&record(), &Capability::transcribe()));
        assert!(!adapter.can_serve(&record(), &Capability::chat()));
    }

    #[test]
    fn it_bids_on_gguf_and_ggml_transcribe_models() {
        let adapter = adapter();
        for format in [ModelFormat::Gguf, ModelFormat::GgmlBin] {
            let bid = adapter
                .bid(
                    &record(),
                    &identified(format, vec![Capability::transcribe()]),
                )
                .unwrap();
            assert_eq!(bid.tier, RunTier::Managed);
            assert_eq!(bid.preference, BidPreference::WHISPER_CPP);
        }
    }

    #[test]
    fn it_does_not_bid_without_transcribe_or_a_supported_format() {
        let adapter = adapter();
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
                    &identified(ModelFormat::Safetensors, vec![Capability::transcribe()])
                )
                .is_none()
        );
    }

    #[test]
    fn options_reads_language_and_translate() {
        let mut fields = std::collections::BTreeMap::new();
        fields.insert("language".to_owned(), JsonValue::String("en".to_owned()));
        fields.insert("translate".to_owned(), JsonValue::Bool(true));
        let options = WhisperCppAdapter::options(&JsonValue::Object(fields));
        assert_eq!(options.language.as_deref(), Some("en"));
        assert!(options.translate);
    }

    #[test]
    fn options_ignores_an_empty_language() {
        let mut fields = std::collections::BTreeMap::new();
        fields.insert("language".to_owned(), JsonValue::String(String::new()));
        let options = WhisperCppAdapter::options(&JsonValue::Object(fields));
        assert!(options.language.is_none());
        assert!(!options.translate);
    }

    #[tokio::test]
    async fn invoke_rejects_a_payload_without_audio() {
        let adapter = adapter();
        let mut stream = adapter.invoke(
            &record(),
            Capability::transcribe(),
            JsonValue::Object(Default::default()),
        );
        assert!(matches!(
            stream.recv().await,
            Some(Err(RuntimeError::Failed(_)))
        ));
    }
}
