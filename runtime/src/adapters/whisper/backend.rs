//! The whisper transcription backend: the interface the engine loads and drives,
//! a "missing" backend for when no engine is bundled, and the shipped
//! sidecar-process backend.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use kernel::capabilities::CapabilityChunk;
use kernel::records::{JsonValue, RuntimeId};

use super::engine::EXPECTED_SAMPLE_RATE;
use super::options::{TranscriptionOptions, TranscriptionSegment};
use crate::adapters::{RuntimeError, RuntimeStream};
use crate::environment::{EnvironmentManager, Progress};
use crate::governor::{BoxFuture, lock};
use crate::sidecar::{RuntimeBundle, SidecarSpec, SidecarSupervisor, SidecarWorkdir};

/// A stream of transcribed segments (or a failure) a backend produces.
pub type SegmentStream = RuntimeStream<TranscriptionSegment>;

/// The sidecar runtime key the whisper engine registers under — note the
/// `python:` prefix, distinct from the `whisper-cpp` [`RuntimeId`].
const WHISPER_RUNTIME_ID: &str = "python:whisper-cpp";
/// The shipped whisper bundle directory name.
const WHISPER_BUNDLE: &str = "python-whisper-cpp";

/// A whisper engine backend: loadable weights that transcribe float samples into
/// timed segments.
pub trait WhisperBackend: Send + Sync {
    /// Load the model at `path`, replacing any currently-loaded one.
    fn load(&self, path: &str) -> BoxFuture<Result<(), RuntimeError>>;
    /// Unload the currently-loaded model, if any.
    fn unload(&self) -> BoxFuture<()>;
    /// Transcribe `samples` (mono, at the engine's expected sample rate).
    fn transcribe(&self, samples: Vec<f32>, options: TranscriptionOptions) -> SegmentStream;
}

/// The placeholder backend used when no whisper engine is bundled: every
/// operation reports the runtime as unavailable.
pub struct MissingWhisperBackend;

const MISSING_HINT: &str = "Transcription needs the whisper engine, which is not bundled yet.";

impl WhisperBackend for MissingWhisperBackend {
    fn load(&self, _path: &str) -> BoxFuture<Result<(), RuntimeError>> {
        Box::pin(async { Err(RuntimeError::Unavailable(MISSING_HINT.to_owned())) })
    }

    fn unload(&self) -> BoxFuture<()> {
        Box::pin(async {})
    }

    fn transcribe(&self, _samples: Vec<f32>, _options: TranscriptionOptions) -> SegmentStream {
        RuntimeStream::failed(RuntimeError::Unavailable(MISSING_HINT.to_owned()))
    }
}

/// The shipped whisper backend: it runs whisper.cpp as a Python sidecar process,
/// feeding it the samples through a temp file and streaming back segments.
pub struct SidecarWhisperBackend {
    supervisor: SidecarSupervisor,
    environments: EnvironmentManager,
    search_roots: Arc<Vec<PathBuf>>,
    workdir_root: PathBuf,
    spec: Arc<Mutex<Option<SidecarSpec>>>,
}

impl SidecarWhisperBackend {
    /// A backend launching sidecars through `supervisor`, preparing environments
    /// through `environments`, finding its bundle under `search_roots`, and
    /// running sidecars under `workdir_root`.
    pub fn new(
        supervisor: SidecarSupervisor,
        environments: EnvironmentManager,
        search_roots: Vec<PathBuf>,
        workdir_root: PathBuf,
    ) -> Self {
        Self {
            supervisor,
            environments,
            search_roots: Arc::new(search_roots),
            workdir_root,
            spec: Arc::new(Mutex::new(None)),
        }
    }
}

impl WhisperBackend for SidecarWhisperBackend {
    fn load(&self, path: &str) -> BoxFuture<Result<(), RuntimeError>> {
        let supervisor = self.supervisor.clone();
        let environments = self.environments.clone();
        let search_roots = Arc::clone(&self.search_roots);
        let workdir_root = self.workdir_root.clone();
        let spec_slot = Arc::clone(&self.spec);
        let path = path.to_owned();
        Box::pin(async move {
            do_unload(&supervisor, &spec_slot).await;
            let next = build_spec(&environments, &search_roots, &workdir_root, &path).await?;
            supervisor.ensure_running(&next).await?;
            *lock(&spec_slot) = Some(next);
            Ok(())
        })
    }

    fn unload(&self) -> BoxFuture<()> {
        let supervisor = self.supervisor.clone();
        let spec_slot = Arc::clone(&self.spec);
        Box::pin(async move { do_unload(&supervisor, &spec_slot).await })
    }

    fn transcribe(&self, samples: Vec<f32>, options: TranscriptionOptions) -> SegmentStream {
        let (tx, stream) = RuntimeStream::channel();
        let supervisor = self.supervisor.clone();
        let spec = lock(&self.spec).clone();
        tokio::spawn(async move {
            let Some(spec) = spec else {
                let _ = tx.send(Err(RuntimeError::Failed(
                    "the whisper sidecar is not loaded".to_owned(),
                )));
                return;
            };
            let workdir = spec
                .working_directory
                .clone()
                .unwrap_or_else(std::env::temp_dir);
            let pcm_path = workdir.join(format!(
                "transcribe-{}-{}.f32",
                std::process::id(),
                next_sequence()
            ));
            if let Err(error) = write_samples(&pcm_path, &samples).await {
                let _ = tx.send(Err(RuntimeError::Failed(error.to_string())));
                return;
            }
            let control = control_message(&pcm_path, &options);
            let mut sidecar = supervisor.request(&spec, control);
            while let Some(item) = sidecar.recv().await {
                let segment = match item {
                    Ok(CapabilityChunk::Segment {
                        text,
                        start_ms,
                        end_ms,
                    }) => TranscriptionSegment {
                        text,
                        start_ms: Some(start_ms),
                        end_ms: Some(end_ms),
                    },
                    Ok(CapabilityChunk::Text(text)) => TranscriptionSegment {
                        text,
                        start_ms: None,
                        end_ms: None,
                    },
                    // Non-transcription chunks (status, etc.) are ignored.
                    Ok(_) => continue,
                    Err(error) => {
                        let _ = tx.send(Err(RuntimeError::from(error)));
                        break;
                    }
                };
                if tx.send(Ok(segment)).is_err() {
                    break;
                }
            }
            let _ = tokio::fs::remove_file(&pcm_path).await;
        });
        stream
    }
}

/// Shut down and forget the currently-loaded sidecar, if any.
async fn do_unload(supervisor: &SidecarSupervisor, spec_slot: &Mutex<Option<SidecarSpec>>) {
    let existing = lock(spec_slot).take();
    if let Some(spec) = existing {
        supervisor.shutdown(&spec.runtime_id).await;
    }
}

/// Build the launch spec for the whisper sidecar loading the weights at `path`.
/// Direct-python (the sanctioned `sandbox-exec` carve-out): the interpreter runs
/// the bundle's `main.py` with `--model <path> --workdir <workdir>`.
async fn build_spec(
    environments: &EnvironmentManager,
    search_roots: &[PathBuf],
    workdir_root: &Path,
    path: &str,
) -> Result<SidecarSpec, RuntimeError> {
    let bundle = RuntimeBundle::require(WHISPER_BUNDLE, search_roots, &RuntimeId::whisper_cpp())?;
    let progress: Progress = Arc::new(|_: &str| {});
    let env_dir = environments
        .prepare(
            WHISPER_RUNTIME_ID,
            &bundle.join("requirements.lock"),
            progress,
        )
        .await
        .map_err(|error| RuntimeError::Failed(error.to_string()))?;
    let workdir = SidecarWorkdir::directory(workdir_root, WHISPER_BUNDLE)
        .map_err(|error| RuntimeError::Failed(error.to_string()))?;

    let python = env_dir.join("bin").join("python");
    let arguments = vec![
        path_string(&bundle.join("main.py")),
        "--model".to_owned(),
        path.to_owned(),
        "--workdir".to_owned(),
        path_string(&workdir),
    ];
    let mut spec = SidecarSpec::new(format!("{WHISPER_RUNTIME_ID}#{path}"), python, arguments);
    spec.environment
        .insert("PYTHONDONTWRITEBYTECODE".to_owned(), "1".to_owned());
    spec.working_directory = Some(workdir);
    spec.ready_timeout = Duration::from_secs(600);
    spec.cooperative_cancel = true;
    spec.cancel_grace_timeout = Duration::from_secs(30);
    Ok(spec)
}

/// The control message driving one transcription: the pcm file, the sample rate,
/// and the optional language / translate flags.
fn control_message(pcm_path: &Path, options: &TranscriptionOptions) -> JsonValue {
    let mut control = BTreeMap::new();
    control.insert("op".to_owned(), JsonValue::String("transcribe".to_owned()));
    control.insert("pcm".to_owned(), JsonValue::String(path_string(pcm_path)));
    control.insert(
        "sample_rate".to_owned(),
        JsonValue::Int(EXPECTED_SAMPLE_RATE),
    );
    if let Some(language) = options.language.as_deref().filter(|lang| !lang.is_empty()) {
        control.insert(
            "language".to_owned(),
            JsonValue::String(language.to_owned()),
        );
    }
    if options.translate {
        control.insert("translate".to_owned(), JsonValue::Bool(true));
    }
    JsonValue::Object(control)
}

/// Write `samples` to `path` as little-endian 32-bit float frames.
async fn write_samples(path: &Path, samples: &[f32]) -> std::io::Result<()> {
    let mut bytes = Vec::with_capacity(samples.len() * 4);
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    tokio::fs::write(path, bytes).await
}

fn next_sequence() -> u64 {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    SEQUENCE.fetch_add(1, Ordering::Relaxed)
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_missing_backend_reports_unavailable() {
        let backend = MissingWhisperBackend;
        let error = backend.load("/models/whisper.bin").await.unwrap_err();
        assert!(matches!(error, RuntimeError::Unavailable(_)));
        let mut stream = backend.transcribe(vec![0.0, 1.0], TranscriptionOptions::default());
        assert!(matches!(
            stream.recv().await,
            Some(Err(RuntimeError::Unavailable(_)))
        ));
    }

    #[test]
    fn the_control_message_carries_the_op_rate_and_flags() {
        let options = TranscriptionOptions {
            language: Some("en".to_owned()),
            translate: true,
        };
        let control = control_message(Path::new("/tmp/x.f32"), &options);
        let fields = control.as_object().unwrap();
        assert_eq!(fields["op"].as_str(), Some("transcribe"));
        assert_eq!(fields["pcm"].as_str(), Some("/tmp/x.f32"));
        assert_eq!(fields["sample_rate"].as_i64(), Some(EXPECTED_SAMPLE_RATE));
        assert_eq!(fields["language"].as_str(), Some("en"));
        assert_eq!(fields["translate"].as_bool(), Some(true));
    }

    #[test]
    fn the_control_message_omits_empty_language_and_absent_translate() {
        let options = TranscriptionOptions {
            language: Some(String::new()),
            translate: false,
        };
        let control = control_message(Path::new("/tmp/x.f32"), &options);
        let fields = control.as_object().unwrap();
        assert!(!fields.contains_key("language"));
        assert!(!fields.contains_key("translate"));
    }

    #[tokio::test]
    async fn transcribe_without_a_loaded_model_fails() {
        let backend = SidecarWhisperBackend::new(
            SidecarSupervisor::default(),
            EnvironmentManager::new(std::env::temp_dir().join("hedos-whisper-env")),
            Vec::new(),
            std::env::temp_dir().join("hedos-whisper-workdirs"),
        );
        let mut stream = backend.transcribe(vec![0.0], TranscriptionOptions::default());
        assert!(matches!(
            stream.recv().await,
            Some(Err(RuntimeError::Failed(_)))
        ));
    }

    #[tokio::test]
    async fn load_without_a_bundle_fails() {
        let backend = SidecarWhisperBackend::new(
            SidecarSupervisor::default(),
            EnvironmentManager::new(std::env::temp_dir().join("hedos-whisper-env")),
            Vec::new(),
            std::env::temp_dir().join("hedos-whisper-workdirs"),
        );
        let error = backend.load("/models/whisper.bin").await.unwrap_err();
        assert!(matches!(error, RuntimeError::Failed(message) if message.contains("missing")));
    }
}
