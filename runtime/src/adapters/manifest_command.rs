//! The manifest command adapter: it serves a model by running the one-shot
//! `[invoke]` command a [`RuntimeManifest`] declares, in a fresh workdir, with the
//! model/prompt/output placeholders substituted. Streaming (`invoke`) and job
//! (`run`) paths share the same governed execution; the interpreter is launched
//! directly (host execution must be approved).

use std::future::Future;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use kernel::capabilities::CapabilityChunk;
use kernel::jobs::JobRuntimeEvent;
use kernel::manifests::RuntimeManifest;
use kernel::records::{
    BidPreference, Capability, ExecutionMode, JsonValue, ModelRecord, RunTier, RuntimeId,
};
use kernel::resolution::{IdentifiedModel, RuntimeBid};
use tokio::process::Command;
use tokio::sync::mpsc;

use super::{ChunkStream, JobRunning, JobStream, RuntimeAdapter, RuntimeError, RuntimeStream};
use crate::environment::{EnvironmentManager, Progress};
use crate::governed::governed_one_shot;
use crate::governor::{GpuProducer, MemoryGovernor};
use crate::manifests::{
    MAX_OUTPUT_FILE_BYTES, bounded_output_data, detect_matches, error_summary, slug, substituted,
};
use crate::process::{drain_bounded, terminate_tree};
use crate::sidecar::SidecarWorkdir;

/// How long a manifest command may run before it is stopped.
const DEFAULT_EXECUTION_TIMEOUT: Duration = Duration::from_secs(1800);
/// How long a timed-out process tree gets to wind down before it is killed.
const KILL_GRACE: Duration = Duration::from_secs(5);
/// The most command output buffered before the excess is discarded.
const OUTPUT_CAP: usize = 256 * 1024 * 1024;

type ChunkSender = mpsc::UnboundedSender<Result<CapabilityChunk, RuntimeError>>;
type JobSender = mpsc::UnboundedSender<Result<JobRuntimeEvent, RuntimeError>>;

/// The manifest-declared command runtime.
pub struct ManifestCommandAdapter {
    execution: Execution,
    governor: MemoryGovernor,
}

#[derive(Clone)]
struct Execution {
    id: RuntimeId,
    manifest: Arc<RuntimeManifest>,
    approved_host_execution: bool,
    environments: EnvironmentManager,
    workdir_root: PathBuf,
    execution_timeout: Duration,
}

impl ManifestCommandAdapter {
    /// An adapter for `manifest`. `approved_host_execution` must be true for it to
    /// bid or run, since it launches a command on this machine. Governs through
    /// `governor`, prepares environments through `environments`, and runs under
    /// `workdir_root`.
    pub fn new(
        manifest: RuntimeManifest,
        approved_host_execution: bool,
        governor: MemoryGovernor,
        environments: EnvironmentManager,
        workdir_root: PathBuf,
    ) -> Self {
        let id = RuntimeId::from(manifest.id.as_str());
        Self {
            execution: Execution {
                id,
                manifest: Arc::new(manifest),
                approved_host_execution,
                environments,
                workdir_root,
                execution_timeout: DEFAULT_EXECUTION_TIMEOUT,
            },
            governor,
        }
    }

    /// Override the execution timeout (default 30 minutes).
    pub fn with_execution_timeout(mut self, timeout: Duration) -> Self {
        self.execution.execution_timeout = timeout;
        self
    }
}

impl RuntimeAdapter for ManifestCommandAdapter {
    fn id(&self) -> &RuntimeId {
        &self.execution.id
    }

    fn can_serve(&self, record: &ModelRecord, capability: &Capability) -> bool {
        record.runtime.id.as_ref() == Some(&self.execution.id)
            && self.execution.manifest.capabilities.contains(capability)
    }

    fn bid(&self, record: &ModelRecord, _identified: &IdentifiedModel) -> Option<RuntimeBid> {
        let detect = self.execution.manifest.detect.as_ref()?;
        if !self.execution.approved_host_execution || !detect_matches(detect, record) {
            return None;
        }
        let alternatives = self
            .execution
            .manifest
            .alternatives
            .iter()
            .map(|id| RuntimeId::from(id.as_str()))
            .collect();
        Some(RuntimeBid::with_alternatives(
            RunTier::Managed,
            BidPreference::MANIFEST,
            alternatives,
        ))
    }

    fn invoke(
        &self,
        record: &ModelRecord,
        _capability: Capability,
        payload: JsonValue,
    ) -> ChunkStream {
        if self.execution.manifest.execution == ExecutionMode::Job {
            return RuntimeStream::failed(RuntimeError::WrongExecutionMode);
        }
        let (tx, stream) = RuntimeStream::channel();
        let execution = self.execution.clone();
        let governor = self.governor.clone();
        let record = record.clone();
        tokio::spawn(async move {
            let progress = chunk_progress(&tx);
            let _ = tx.send(Ok(CapabilityChunk::Status(format!(
                "Running {}…",
                execution.id.as_str()
            ))));
            // `tx.closed()` threads consumer-drop into the run so a cancel kills
            // the whole process tree, not just the immediate child.
            let outcome = governed_one_shot(
                &governor,
                &record,
                GpuProducer::Generation(record.id.clone()),
                progress.as_ref(),
                execution.run(&record, &payload, &progress, tx.closed()),
            )
            .await;
            match outcome {
                Ok((stdout, _outputs)) => {
                    for line in stdout.split('\n') {
                        if tx
                            .send(Ok(CapabilityChunk::Text(format!("{line}\n"))))
                            .is_err()
                        {
                            return;
                        }
                    }
                    let _ = tx.send(Ok(CapabilityChunk::Done(None)));
                }
                Err(error) => {
                    let _ = tx.send(Err(error));
                }
            }
        });
        stream
    }
}

impl JobRunning for ManifestCommandAdapter {
    fn run(&self, record: &ModelRecord, _capability: Capability, payload: JsonValue) -> JobStream {
        if self.execution.manifest.execution != ExecutionMode::Job {
            return RuntimeStream::failed(RuntimeError::WrongExecutionMode);
        }
        let (tx, stream) = RuntimeStream::channel();
        let execution = self.execution.clone();
        let governor = self.governor.clone();
        let record = record.clone();
        tokio::spawn(async move {
            let progress = job_progress(&tx);
            let _ = tx.send(Ok(JobRuntimeEvent::Status(format!(
                "Running {}…",
                execution.id.as_str()
            ))));
            let outcome = governed_one_shot(
                &governor,
                &record,
                GpuProducer::Job(record.id.clone()),
                progress.as_ref(),
                execution.run(&record, &payload, &progress, tx.closed()),
            )
            .await;
            match outcome {
                Ok((_stdout, outputs)) => {
                    let _ = tx.send(Ok(JobRuntimeEvent::Started));
                    for file in output_entries(&outputs) {
                        match bounded_output_data(&file, MAX_OUTPUT_FILE_BYTES) {
                            Ok(data) => {
                                let extension = file
                                    .extension()
                                    .and_then(|extension| extension.to_str())
                                    .unwrap_or("")
                                    .to_owned();
                                if tx
                                    .send(Ok(JobRuntimeEvent::Result {
                                        data,
                                        file_extension: extension,
                                    }))
                                    .is_err()
                                {
                                    return;
                                }
                            }
                            Err(error) => {
                                let _ = tx.send(Err(RuntimeError::Failed(error.to_string())));
                                return;
                            }
                        }
                    }
                }
                Err(error) => {
                    let _ = tx.send(Err(error));
                }
            }
        });
        stream
    }
}

impl Execution {
    /// Prepare the environment, build a fresh workdir, substitute the command, and
    /// run it — returning its stdout and the outputs directory. Fails if host
    /// execution is not approved or the command exits non-zero, times out, or is
    /// otherwise unrunnable.
    async fn run<F: Future<Output = ()>>(
        &self,
        record: &ModelRecord,
        payload: &JsonValue,
        progress: &Progress,
        cancel: F,
    ) -> Result<(String, PathBuf), RuntimeError> {
        if !self.approved_host_execution {
            return Err(RuntimeError::Unavailable(format!(
                "{} runs code on this machine and needs your approval. Approve it from the model's page.",
                self.id.as_str()
            )));
        }
        let Some(invoke) = &self.manifest.invoke else {
            return Err(RuntimeError::Failed(format!(
                "{} declares no [invoke] command",
                self.id.as_str()
            )));
        };

        let env_dir = self.prepare_environment(progress).await?;
        let workdir = SidecarWorkdir::directory(&self.workdir_root, &slug(self.id.as_str()))
            .map_err(|error| RuntimeError::Failed(error.to_string()))?;
        let outputs = workdir.join("outputs");
        let _ = std::fs::remove_dir_all(&outputs);
        std::fs::create_dir_all(&outputs)
            .map_err(|error| RuntimeError::Failed(error.to_string()))?;

        let tokens = substituted(
            &invoke.command,
            record,
            payload,
            &workdir,
            &outputs,
            env_dir.as_deref(),
        )
        .map_err(|error| RuntimeError::Failed(error.to_string()))?;

        let stdout = self.execute(&tokens, &workdir, cancel).await?;
        Ok((stdout, outputs))
    }

    async fn prepare_environment(
        &self,
        progress: &Progress,
    ) -> Result<Option<PathBuf>, RuntimeError> {
        let Some(env) = &self.manifest.env else {
            return Ok(None);
        };
        let Some(directory) = &self.manifest.directory else {
            return Err(RuntimeError::Failed(format!(
                "manifest {} declares [env] but has no directory",
                self.id.as_str()
            )));
        };
        let env_dir = self
            .environments
            .prepare(
                self.id.as_str(),
                &directory.join(&env.lockfile),
                Arc::clone(progress),
            )
            .await
            .map_err(|error| RuntimeError::Failed(error.to_string()))?;
        Ok(Some(env_dir))
    }

    /// Spawn `tokens` (the substituted command) in `workdir`, draining its output
    /// and enforcing the execution timeout, and return its stdout. Killing the
    /// whole process tree on timeout or when `cancel` resolves (consumer drop).
    async fn execute<F: Future<Output = ()>>(
        &self,
        tokens: &[String],
        workdir: &Path,
        cancel: F,
    ) -> Result<String, RuntimeError> {
        let (program, arguments) = tokens
            .split_first()
            .ok_or_else(|| RuntimeError::Failed("the manifest command is empty".to_owned()))?;
        let mut command = Command::new(program);
        command
            .args(arguments)
            .current_dir(workdir)
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .env_remove("PYTHONPATH")
            .env_remove("PYTHONHOME")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut child = command
            .spawn()
            .map_err(|error| RuntimeError::Failed(format!("{}: {error}", self.id.as_str())))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| RuntimeError::Failed("the command produced no stdout".to_owned()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| RuntimeError::Failed("the command produced no stderr".to_owned()))?;

        let drain = drain_bounded(stdout, stderr, OUTPUT_CAP, || {});
        tokio::pin!(drain);
        tokio::pin!(cancel);
        let outcome = tokio::select! {
            drained = &mut drain => drained,
            _ = tokio::time::sleep(self.execution_timeout) => {
                terminate_tree(&mut child, KILL_GRACE).await;
                let minutes = (self.execution_timeout.as_secs() / 60).max(1);
                return Err(RuntimeError::Failed(format!(
                    "{} ran for more than {minutes} minutes and was stopped",
                    self.id.as_str()
                )));
            }
            _ = &mut cancel => {
                terminate_tree(&mut child, KILL_GRACE).await;
                return Err(RuntimeError::Cancelled);
            }
        };
        let status = child
            .wait()
            .await
            .map_err(|error| RuntimeError::Failed(error.to_string()))?;
        if !status.success() {
            let code = exit_code(&status);
            let tail = error_summary(&String::from_utf8_lossy(&outcome.stderr));
            return Err(RuntimeError::Failed(format!(
                "{} stopped with status {code}: {tail}",
                self.id.as_str()
            )));
        }
        Ok(String::from_utf8_lossy(&outcome.stdout).into_owned())
    }
}

/// The non-hidden entries in `outputs`, sorted by name. Directories are included
/// (not filtered out): reading one as an artifact fails the job, matching a
/// runtime that wrote a stray directory into its outputs.
fn output_entries(outputs: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(outputs) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| !name.starts_with('.'))
        })
        .collect();
    files.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
    files
}

/// The exit code to report: the process's exit status, else the terminating
/// signal number (Unix), else `-1`.
fn exit_code(status: &std::process::ExitStatus) -> i64 {
    if let Some(code) = status.code() {
        return i64::from(code);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return i64::from(signal);
        }
    }
    -1
}

/// A status reporter that forwards each message as a `Status` capability chunk.
fn chunk_progress(tx: &ChunkSender) -> Progress {
    let tx = tx.clone();
    Arc::new(move |message: &str| {
        let _ = tx.send(Ok(CapabilityChunk::Status(message.to_owned())));
    })
}

/// A status reporter that forwards each message as a `Status` job event.
fn job_progress(tx: &JobSender) -> Progress {
    let tx = tx.clone();
    Arc::new(move |message: &str| {
        let _ = tx.send(Ok(JobRuntimeEvent::Status(message.to_owned())));
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::governor::GovernorConfig;
    use kernel::records::{Modality, ModelSource, ModelState, SourceKind};

    fn manifest(execution: &str, command: &str) -> RuntimeManifest {
        // Job execution must declare a job-shaped capability; stream/sync a chat one.
        let capability = if execution == "job" { "image" } else { "chat" };
        let text = format!(
            "id = \"echo-runtime\"\ncapabilities = [\"{capability}\"]\nexecution = \"{execution}\"\ndetect = {{ extension = \"gguf\" }}\n[invoke]\ncommand = \"{command}\"\n"
        );
        RuntimeManifest::parse(&text, None).unwrap()
    }

    fn adapter(manifest: RuntimeManifest, approved: bool) -> ManifestCommandAdapter {
        ManifestCommandAdapter::new(
            manifest,
            approved,
            MemoryGovernor::new(GovernorConfig::with_total_mb(262_144)),
            EnvironmentManager::new(std::env::temp_dir().join("hedos-manifest-env")),
            std::env::temp_dir().join("hedos-manifest-workdirs"),
        )
    }

    fn record() -> ModelRecord {
        let mut record = ModelRecord::new(
            "m",
            Modality::text(),
            Vec::new(),
            ModelSource::new(SourceKind::folder(), "/models/m.gguf"),
        );
        record.runtime.id = Some(RuntimeId::from("echo-runtime"));
        record.primary_weight_path = Some("/models/m.gguf".to_owned());
        record.state = ModelState::Ready;
        record
    }

    #[test]
    fn it_serves_a_declared_capability_for_its_own_runtime() {
        let adapter = adapter(manifest("sync", "true"), true);
        assert_eq!(adapter.id(), &RuntimeId::from("echo-runtime"));
        assert!(adapter.can_serve(&record(), &Capability::chat()));
        assert!(!adapter.can_serve(&record(), &Capability::embed()));
    }

    #[test]
    fn it_bids_when_detect_matches_and_execution_is_approved() {
        let identified = IdentifiedModel::new(
            kernel::resolution::ModelFormat::Gguf,
            None,
            Vec::new(),
            ExecutionMode::Sync,
        );
        let bid = adapter(manifest("sync", "true"), true).bid(&record(), &identified);
        assert!(bid.is_some());
        assert_eq!(bid.unwrap().preference, BidPreference::MANIFEST);
        // Not approved → no bid.
        assert!(
            adapter(manifest("sync", "true"), false)
                .bid(&record(), &identified)
                .is_none()
        );
    }

    #[tokio::test]
    async fn a_job_execution_rejects_streaming_invoke() {
        let mut stream = adapter(manifest("job", "true"), true).invoke(
            &record(),
            Capability::chat(),
            JsonValue::Object(Default::default()),
        );
        assert!(matches!(
            stream.recv().await,
            Some(Err(RuntimeError::WrongExecutionMode))
        ));
    }

    #[tokio::test]
    async fn a_stream_execution_rejects_the_job_run_path() {
        let mut stream = adapter(manifest("sync", "true"), true).run(
            &record(),
            Capability::chat(),
            JsonValue::Object(Default::default()),
        );
        assert!(matches!(
            stream.recv().await,
            Some(Err(RuntimeError::WrongExecutionMode))
        ));
    }

    #[tokio::test]
    async fn unapproved_execution_fails_with_unavailable() {
        let adapter = adapter(manifest("sync", "true"), false);
        let mut stream = adapter.invoke(
            &record(),
            Capability::chat(),
            JsonValue::Object(Default::default()),
        );
        let mut error = None;
        while let Some(item) = stream.recv().await {
            if let Err(err) = item {
                error = Some(err);
            }
        }
        assert!(matches!(error, Some(RuntimeError::Unavailable(_))));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn it_runs_a_command_and_streams_its_stdout() {
        let adapter = adapter(manifest("sync", "printf hello"), true);
        let mut stream = adapter.invoke(
            &record(),
            Capability::chat(),
            JsonValue::Object(Default::default()),
        );
        let mut text = String::new();
        let mut done = false;
        while let Some(item) = stream.recv().await {
            match item.unwrap() {
                CapabilityChunk::Text(chunk) => text.push_str(&chunk),
                CapabilityChunk::Done(_) => done = true,
                _ => {}
            }
        }
        assert!(done);
        assert!(text.contains("hello"), "got {text:?}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_job_run_emits_started_then_a_result_per_output_file() {
        // The command writes one file into the outputs directory.
        let adapter = adapter(manifest("job", "touch {outputs}/frame.png"), true);
        let mut stream = adapter.run(
            &record(),
            Capability::image(),
            JsonValue::Object(Default::default()),
        );
        let mut started = false;
        let mut extensions = Vec::new();
        while let Some(item) = stream.recv().await {
            match item.unwrap() {
                JobRuntimeEvent::Started => started = true,
                JobRuntimeEvent::Result { file_extension, .. } => extensions.push(file_extension),
                _ => {}
            }
        }
        assert!(started);
        assert_eq!(extensions, vec!["png".to_owned()]);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_command_that_overruns_its_timeout_is_stopped() {
        let adapter = adapter(manifest("sync", "sleep 30"), true)
            .with_execution_timeout(Duration::from_millis(150));
        let mut stream = adapter.invoke(
            &record(),
            Capability::chat(),
            JsonValue::Object(Default::default()),
        );
        let mut error = None;
        while let Some(item) = stream.recv().await {
            if let Err(err) = item {
                error = Some(err);
            }
        }
        assert!(
            matches!(error, Some(RuntimeError::Failed(message)) if message.contains("more than")),
            "expected a timeout failure"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_nonzero_exit_becomes_a_failure() {
        let adapter = adapter(manifest("sync", "false"), true);
        let mut stream = adapter.invoke(
            &record(),
            Capability::chat(),
            JsonValue::Object(Default::default()),
        );
        let mut error = None;
        while let Some(item) = stream.recv().await {
            if let Err(err) = item {
                error = Some(err);
            }
        }
        assert!(matches!(error, Some(RuntimeError::Failed(message)) if message.contains("status")));
    }
}
