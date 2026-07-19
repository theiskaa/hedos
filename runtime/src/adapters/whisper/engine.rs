//! The governed whisper engine: it brings a transcription model into residency
//! through the memory governor, serializes transcriptions behind a slot, and
//! streams the backend's segments out as capability chunks.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use kernel::capabilities::{CapabilityChunk, GenerationStats};
use tokio::sync::mpsc;

use super::backend::WhisperBackend;
use super::options::TranscriptionOptions;
use crate::adapters::{ChunkStream, RuntimeError, RuntimeStream};
use crate::governed::{
    EngineLoad, EngineLoader, FootprintProbe, LoadedProbe, PreviousModelProbe, StatusFn,
    UnloadPrevious, acquire_loaded,
};
use crate::governor::{GenerationSlot, GpuProducer, MemoryGovernor, RawUnloader, lock};
use crate::util::weights_mb;

/// The sample rate whisper expects its input at (16 kHz mono).
pub const EXPECTED_SAMPLE_RATE: i64 = 16_000;

const TIGHT_STATUS: &str = "Memory is tight — transcription may be slow";

type ChunkSender = mpsc::UnboundedSender<Result<CapabilityChunk, RuntimeError>>;

/// One transcription request the engine runs: which model, its footprint, the
/// audio samples, and the options.
pub struct TranscriptionJob {
    /// The model's stable id.
    pub model_id: String,
    /// The model's display name.
    pub model_name: String,
    /// The path to the model weights.
    pub path: String,
    /// The model's footprint estimate, if known.
    pub footprint_mb: Option<i64>,
    /// The mono samples to transcribe (at [`EXPECTED_SAMPLE_RATE`]).
    pub samples: Vec<f32>,
    /// The transcription options.
    pub options: TranscriptionOptions,
}

/// A transcription engine governing one whisper backend.
#[derive(Clone)]
pub struct WhisperEngine {
    inner: Arc<Inner>,
}

struct Inner {
    backend: Arc<dyn WhisperBackend>,
    loaded: Mutex<Loaded>,
    transcription_slot: GenerationSlot,
}

#[derive(Default)]
struct Loaded {
    path: Option<String>,
    model_id: Option<String>,
}

impl WhisperEngine {
    /// An engine driving `backend`.
    pub fn new(backend: Arc<dyn WhisperBackend>) -> Self {
        Self {
            inner: Arc::new(Inner {
                backend,
                loaded: Mutex::new(Loaded::default()),
                transcription_slot: GenerationSlot::new(),
            }),
        }
    }

    /// Run `job` as a governed transcription against `governor`, streaming status,
    /// segments, and a terminal `Done` (or a failure).
    pub fn run(&self, governor: MemoryGovernor, job: TranscriptionJob) -> ChunkStream {
        let (tx, stream) = RuntimeStream::channel();
        let inner = Arc::clone(&self.inner);
        tokio::spawn(async move {
            if let Err(error) = inner.drive(&governor, job, &tx).await {
                let _ = tx.send(Err(error));
            }
        });
        stream
    }
}

impl Inner {
    async fn drive(
        self: &Arc<Self>,
        governor: &MemoryGovernor,
        job: TranscriptionJob,
        tx: &ChunkSender,
    ) -> Result<(), RuntimeError> {
        governor.begin_generation(&job.model_id);
        let _lease = GenerationLease {
            governor,
            model_id: &job.model_id,
        };
        let status: StatusFn = &|message: &str| {
            let _ = tx.send(Ok(CapabilityChunk::Status(message.to_owned())));
        };
        let gate = acquire_loaded(EngineLoad {
            governor,
            producer: GpuProducer::Generation(job.model_id.clone()),
            model_id: &job.model_id,
            model_name: &job.model_name,
            footprint_mb: job.footprint_mb,
            tight_status: TIGHT_STATUS,
            status,
            is_loaded: self.is_loaded_probe(&job.path),
            previous_model_id: self.previous_model_probe(),
            unload_previous: self.unload_previous_probe(),
            load: self.load_probe(&job.path, &job.model_id),
            evict: self.evict_probe(&job.path),
            observed_footprint_mb: footprint_probe(&job.path),
        })
        .await?;

        let _slot = self.transcription_slot.acquire().await;
        let result = self.transcribe(job.samples, job.options, tx).await;
        drop(_slot);
        drop(gate);
        result
    }

    async fn transcribe(
        &self,
        samples: Vec<f32>,
        options: TranscriptionOptions,
        tx: &ChunkSender,
    ) -> Result<(), RuntimeError> {
        let started = Instant::now();
        let mut stream = self.backend.transcribe(samples, options);
        while let Some(item) = stream.recv().await {
            let segment = item?;
            let chunk = match (segment.start_ms, segment.end_ms) {
                (Some(start_ms), Some(end_ms)) => CapabilityChunk::Segment {
                    text: segment.text,
                    start_ms,
                    end_ms,
                },
                _ => CapabilityChunk::Text(segment.text),
            };
            if tx.send(Ok(chunk)).is_err() {
                // The consumer dropped the stream; stop.
                return Ok(());
            }
        }
        let duration_ms = started.elapsed().as_millis() as i64;
        let _ = tx.send(Ok(CapabilityChunk::Done(Some(GenerationStats {
            duration_ms: Some(duration_ms),
            ..Default::default()
        }))));
        Ok(())
    }

    fn is_loaded(&self, path: &str) -> bool {
        lock(&self.loaded).path.as_deref() == Some(path)
    }

    async fn load_backend(&self, path: &str, model_id: &str) -> Result<(), RuntimeError> {
        self.backend.load(path).await?;
        let mut loaded = lock(&self.loaded);
        loaded.path = Some(path.to_owned());
        loaded.model_id = Some(model_id.to_owned());
        Ok(())
    }

    async fn unload_backend(&self) {
        self.backend.unload().await;
        let mut loaded = lock(&self.loaded);
        loaded.path = None;
        loaded.model_id = None;
    }

    async fn unload_if_loaded(&self, path: &str) {
        if self.is_loaded(path) {
            self.unload_backend().await;
        }
    }

    fn is_loaded_probe(self: &Arc<Self>, path: &str) -> LoadedProbe {
        let this = Arc::clone(self);
        let path = path.to_owned();
        Arc::new(move || {
            let this = Arc::clone(&this);
            let path = path.clone();
            Box::pin(async move { this.is_loaded(&path) })
        })
    }

    fn previous_model_probe(self: &Arc<Self>) -> PreviousModelProbe {
        let this = Arc::clone(self);
        Arc::new(move || {
            let this = Arc::clone(&this);
            Box::pin(async move { lock(&this.loaded).model_id.clone() })
        })
    }

    fn unload_previous_probe(self: &Arc<Self>) -> UnloadPrevious {
        let this = Arc::clone(self);
        Arc::new(move || {
            let this = Arc::clone(&this);
            Box::pin(async move { this.unload_backend().await })
        })
    }

    fn load_probe(self: &Arc<Self>, path: &str, model_id: &str) -> EngineLoader<RuntimeError> {
        let this = Arc::clone(self);
        let path = path.to_owned();
        let model_id = model_id.to_owned();
        Arc::new(move || {
            let this = Arc::clone(&this);
            let path = path.clone();
            let model_id = model_id.clone();
            Box::pin(async move { this.load_backend(&path, &model_id).await })
        })
    }

    fn evict_probe(self: &Arc<Self>, path: &str) -> RawUnloader {
        let this = Arc::clone(self);
        let path = path.to_owned();
        Arc::new(move || {
            let this = Arc::clone(&this);
            let path = path.clone();
            Box::pin(async move { this.unload_if_loaded(&path).await })
        })
    }
}

fn footprint_probe(path: &str) -> FootprintProbe {
    let path = path.to_owned();
    Arc::new(move || weights_mb(Path::new(&path)))
}

struct GenerationLease<'a> {
    governor: &'a MemoryGovernor,
    model_id: &'a str,
}

impl Drop for GenerationLease<'_> {
    fn drop(&mut self) {
        self.governor.end_generation(self.model_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::governor::{BoxFuture, GovernorConfig};
    use kernel::capabilities::CapabilityChunk;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct FakeBackend {
        loads: Arc<AtomicUsize>,
        segments: Vec<super::super::options::TranscriptionSegment>,
    }

    impl WhisperBackend for FakeBackend {
        fn load(&self, _path: &str) -> BoxFuture<Result<(), RuntimeError>> {
            self.loads.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(()) })
        }
        fn unload(&self) -> BoxFuture<()> {
            Box::pin(async {})
        }
        fn transcribe(
            &self,
            _samples: Vec<f32>,
            _options: TranscriptionOptions,
        ) -> super::super::backend::SegmentStream {
            let (tx, stream) = RuntimeStream::channel();
            for segment in &self.segments {
                let _ = tx.send(Ok(segment.clone()));
            }
            stream
        }
    }

    fn governor() -> MemoryGovernor {
        MemoryGovernor::new(GovernorConfig::with_total_mb(262_144))
    }

    fn job() -> TranscriptionJob {
        TranscriptionJob {
            model_id: "m".to_owned(),
            model_name: "M".to_owned(),
            path: "/models/whisper.bin".to_owned(),
            footprint_mb: Some(512),
            samples: vec![0.0, 1.0, 2.0],
            options: TranscriptionOptions::default(),
        }
    }

    #[tokio::test]
    async fn it_loads_then_streams_segments_and_a_terminal_done() {
        use super::super::options::TranscriptionSegment;
        let loads = Arc::new(AtomicUsize::new(0));
        let backend = Arc::new(FakeBackend {
            loads: Arc::clone(&loads),
            segments: vec![
                TranscriptionSegment {
                    text: "hello".to_owned(),
                    start_ms: Some(0),
                    end_ms: Some(500),
                },
                TranscriptionSegment {
                    text: " world".to_owned(),
                    start_ms: None,
                    end_ms: None,
                },
            ],
        });
        let engine = WhisperEngine::new(backend);
        let governor = governor();
        let mut stream = engine.run(governor.clone(), job());

        let mut chunks = Vec::new();
        while let Some(item) = stream.recv().await {
            chunks.push(item.unwrap());
        }

        assert_eq!(loads.load(Ordering::SeqCst), 1);
        assert!(chunks.iter().any(|c| matches!(
            c,
            CapabilityChunk::Segment { text, start_ms: 0, end_ms: 500 } if text == "hello"
        )));
        assert!(
            chunks
                .iter()
                .any(|c| matches!(c, CapabilityChunk::Text(t) if t == " world"))
        );
        assert!(matches!(chunks.last(), Some(CapabilityChunk::Done(_))));
        assert!(governor.is_resident("m"));
    }

    #[tokio::test]
    async fn a_backend_load_failure_surfaces_and_leaves_no_resident() {
        struct FailingBackend;
        impl WhisperBackend for FailingBackend {
            fn load(&self, _path: &str) -> BoxFuture<Result<(), RuntimeError>> {
                Box::pin(async { Err(RuntimeError::Unavailable("no engine".to_owned())) })
            }
            fn unload(&self) -> BoxFuture<()> {
                Box::pin(async {})
            }
            fn transcribe(
                &self,
                _samples: Vec<f32>,
                _options: TranscriptionOptions,
            ) -> super::super::backend::SegmentStream {
                RuntimeStream::failed(RuntimeError::Unavailable("no engine".to_owned()))
            }
        }
        let engine = WhisperEngine::new(Arc::new(FailingBackend));
        let governor = governor();
        let mut stream = engine.run(governor.clone(), job());

        let mut last_error = None;
        while let Some(item) = stream.recv().await {
            if let Err(error) = item {
                last_error = Some(error);
            }
        }
        assert!(matches!(last_error, Some(RuntimeError::Unavailable(_))));
        assert!(!governor.is_resident("m"));
    }
}
