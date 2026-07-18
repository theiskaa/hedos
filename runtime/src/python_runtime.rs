//! The governed streaming driver shared by every Python-sidecar adapter. A
//! `Descriptor` says how to prepare the environment and build the spec; `run`
//! then yields the preparing status, warm-loads the sidecar through the governor,
//! opens the supervisor stream, and forwards its events — releasing the producer
//! gate and ending the generation lease on every exit path.
//!
//! The consumer dropping the returned stream is the cancellation signal: it is
//! observed at every `.await` (env prep, warm load, and each forwarded frame) via
//! `tx.closed()`, so an abandoned request stops promptly and cascades a cancel
//! into the sidecar.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use kernel::capabilities::CapabilityChunk;
use kernel::jobs::JobRuntimeEvent;
use kernel::records::{Capability, JsonValue, ModelRecord};
use tokio::sync::mpsc;

use crate::governed::warm_load_acquire;
use crate::governor::{BoxFuture, GpuProducer, MemoryGovernor};
use crate::sidecar::{SidecarError, SidecarSpec, SidecarStream, SidecarSupervisor};

/// A status reporter passed into environment preparation.
pub type StatusSink = Arc<dyn Fn(&str) + Send + Sync>;

/// Prepares the runtime's Python environment, reporting steps through the sink,
/// and returns the environment directory (or `None` if it needs none).
pub type PrepareEnvironment =
    Arc<dyn Fn(StatusSink) -> BoxFuture<Result<Option<PathBuf>, SidecarError>> + Send + Sync>;

/// Builds the sidecar spec for a model and its (optional) environment directory.
pub type MakeSpec =
    Arc<dyn Fn(&ModelRecord, Option<&Path>) -> Result<SidecarSpec, SidecarError> + Send + Sync>;

/// How one Python-sidecar runtime prepares and launches.
#[derive(Clone)]
pub struct Descriptor {
    /// The runtime's identity (for logging/config); the sidecar's own key comes
    /// from the spec `make_spec` builds, not this field.
    pub runtime_id: String,
    pub preparing_status: String,
    pub starting_status: String,
    pub warm_window: Option<Duration>,
    pub prepare_environment: PrepareEnvironment,
    pub make_spec: MakeSpec,
}

/// Drives a governed generation against a Python sidecar. Cheap to clone (`Arc`
/// handles).
#[derive(Clone)]
pub struct PythonSidecarRuntime {
    descriptor: Arc<Descriptor>,
    governor: MemoryGovernor,
    supervisor: SidecarSupervisor,
}

impl PythonSidecarRuntime {
    /// A runtime for `descriptor`, governed by `governor` and running its sidecar
    /// through `supervisor`.
    pub fn new(
        descriptor: Descriptor,
        governor: MemoryGovernor,
        supervisor: SidecarSupervisor,
    ) -> Self {
        Self {
            descriptor: Arc::new(descriptor),
            governor,
            supervisor,
        }
    }

    /// Open a capability stream (chat/vision/speech/transcription/embeddings).
    pub fn stream(
        &self,
        record: &ModelRecord,
        op: Capability,
        payload: JsonValue,
    ) -> SidecarStream<CapabilityChunk> {
        self.run(
            record,
            op.as_ref().to_owned(),
            payload,
            GpuProducer::Generation(record.id.clone()),
            CapabilityChunk::Status,
            |supervisor, spec, control| supervisor.request(spec, control),
        )
    }

    /// Open a job stream (image generation).
    pub fn job(
        &self,
        record: &ModelRecord,
        op: &str,
        payload: JsonValue,
    ) -> SidecarStream<JobRuntimeEvent> {
        self.run(
            record,
            op.to_owned(),
            payload,
            GpuProducer::Job(record.id.clone()),
            JobRuntimeEvent::Status,
            |supervisor, spec, control| supervisor.job_request(spec, control),
        )
    }

    fn run<E, S, O>(
        &self,
        record: &ModelRecord,
        op: String,
        payload: JsonValue,
        producer: GpuProducer,
        make_status: S,
        open: O,
    ) -> SidecarStream<E>
    where
        E: Send + 'static,
        S: Fn(String) -> E + Send + Sync + 'static,
        O: FnOnce(&SidecarSupervisor, &SidecarSpec, JsonValue) -> SidecarStream<E> + Send + 'static,
    {
        let (tx, rx) = mpsc::unbounded_channel();
        let runtime = self.clone();
        let record = record.clone();
        let make_status = Arc::new(make_status);
        tokio::spawn(async move {
            let result = runtime
                .drive(&record, op, payload, producer, make_status, open, &tx)
                .await;
            if let Err(err) = result {
                let _ = tx.send(Err(err));
            }
        });
        SidecarStream { rx }
    }

    #[allow(clippy::too_many_arguments)]
    async fn drive<E, S, O>(
        &self,
        record: &ModelRecord,
        op: String,
        payload: JsonValue,
        producer: GpuProducer,
        make_status: Arc<S>,
        open: O,
        tx: &mpsc::UnboundedSender<Result<E, SidecarError>>,
    ) -> Result<(), SidecarError>
    where
        E: Send + 'static,
        S: Fn(String) -> E + Send + Sync + 'static,
        O: FnOnce(&SidecarSupervisor, &SidecarSpec, JsonValue) -> SidecarStream<E>,
    {
        let _ = tx.send(Ok(make_status(self.descriptor.preparing_status.clone())));

        let status_sink: StatusSink = {
            let tx = tx.clone();
            let make_status = Arc::clone(&make_status);
            Arc::new(move |message: &str| {
                let _ = tx.send(Ok(make_status(message.to_owned())));
            })
        };

        // Race the long pre-stream awaits (env prep, model load) against the
        // consumer leaving, so an abandoned request stops promptly instead of
        // running a multi-minute build or a full load to completion.
        let env_dir = tokio::select! {
            result = (self.descriptor.prepare_environment)(Arc::clone(&status_sink)) => result?,
            _ = tx.closed() => return Ok(()),
        };
        let spec = (self.descriptor.make_spec)(record, env_dir.as_deref())?;

        self.governor.begin_generation(&record.id);
        let _lease = GenerationLease {
            governor: &self.governor,
            model_id: &record.id,
        };

        let load = warm_load_acquire(
            &self.governor,
            &self.supervisor,
            &spec,
            record,
            producer,
            self.descriptor.warm_window,
            &self.descriptor.starting_status,
            &*status_sink,
        );
        // A consumer drop here aborts the load: the future is dropped, so its
        // reservation guard unloads the model and `_lease` ends the generation.
        let gate = tokio::select! {
            result = load => result?,
            _ = tx.closed() => return Ok(()),
        };

        let control = control(&op, payload);
        let mut sub = open(&self.supervisor, &spec, control);
        let outcome = loop {
            tokio::select! {
                event = sub.recv() => match event {
                    Some(Ok(event)) => {
                        if tx.send(Ok(event)).is_err() {
                            break Ok(());
                        }
                    }
                    Some(Err(err)) => break Err(err),
                    None => break Ok(()),
                },
                // The consumer dropped the stream: stop forwarding and drop `sub`,
                // which cascades a cancel into the supervisor stream.
                _ = tx.closed() => break Ok(()),
            }
        };
        drop(gate);
        outcome
    }
}

fn control(op: &str, payload: JsonValue) -> JsonValue {
    let mut fields = BTreeMap::new();
    fields.insert("op".to_owned(), JsonValue::String(op.to_owned()));
    if let JsonValue::Object(payload) = payload {
        fields.extend(payload);
    }
    JsonValue::Object(fields)
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
