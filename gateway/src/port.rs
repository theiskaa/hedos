//! The seam between the gateway and the kernel: everything a handler needs from
//! the running kernel, behind a trait so handlers can be tested against a double.
//! The concrete implementation over the runtime facade lands with the bridge.

use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;

use kernel::jobs::{Job, JobEvent};
use kernel::records::{Capability, JsonValue, ModelRecord};
use runtime::adapters::ChunkStream;
use runtime::facade::KernelError;
use tokio::sync::mpsc;

use crate::admission::{GatewayAdmissionState, GatewayWorkKind};
use crate::error::{GatewayError, GatewayErrorKind};

/// A boxed future returned by a [`GatewayPort`] method, borrowing the port (and
/// its arguments) for the duration.
pub type PortFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Everything a handler needs from the running kernel: the shelf, streaming and
/// job dispatch, job control, and the metadata (tool support, honored params,
/// voices, admission) that shapes a request.
pub trait GatewayPort: Send + Sync {
    /// The models currently on the shelf.
    fn shelf(&self) -> PortFuture<'_, Vec<ModelRecord>>;

    /// Serve a streaming request, yielding capability chunks.
    fn invoke<'a>(
        &'a self,
        model_id: &'a str,
        capability: Capability,
        payload: JsonValue,
    ) -> PortFuture<'a, Result<ChunkStream, KernelError>>;

    /// Submit a job (image generation, etc.), returning its id.
    fn submit<'a>(
        &'a self,
        model_id: &'a str,
        capability: Capability,
        payload: JsonValue,
    ) -> PortFuture<'a, Result<String, KernelError>>;

    /// The current state of a job, if it exists.
    fn job<'a>(&'a self, id: &'a str) -> PortFuture<'a, Option<Job>>;

    /// A stream of a job's runtime events.
    fn job_events<'a>(&'a self, id: &'a str) -> PortFuture<'a, mpsc::UnboundedReceiver<JobEvent>>;

    /// Cancel a running job.
    fn cancel<'a>(&'a self, job_id: &'a str) -> PortFuture<'a, ()>;

    /// The voices a speech model offers.
    fn voices<'a>(&'a self, model_id: &'a str) -> PortFuture<'a, Result<Vec<String>, KernelError>>;

    /// The parameter keys the model's runtime honors for a capability.
    fn honored_params<'a>(
        &'a self,
        model_id: &'a str,
        capability: Capability,
    ) -> PortFuture<'a, Result<HashSet<String>, KernelError>>;

    /// The raw bytes of a produced artifact, if it exists.
    fn artifact_data<'a>(
        &'a self,
        id: &'a str,
    ) -> PortFuture<'a, Result<Option<Vec<u8>>, KernelError>>;

    /// Whether the machine can admit a request for a model of the given
    /// footprint and work kind right now.
    fn admission_state<'a>(
        &'a self,
        model_id: &'a str,
        footprint_mb: Option<i64>,
        kind: GatewayWorkKind,
    ) -> PortFuture<'a, GatewayAdmissionState>;
}

/// Reject a request up front if the machine can't admit the model right now,
/// surfacing the governor's retry hint as an overloaded error.
pub async fn require_admission(
    port: &dyn GatewayPort,
    record: &ModelRecord,
    kind: GatewayWorkKind,
) -> Result<(), GatewayError> {
    let state = port
        .admission_state(&record.id, record.footprint_mb, kind)
        .await;
    if let GatewayAdmissionState::Saturated {
        retry_after_seconds,
    } = state
    {
        return Err(GatewayError::new(
            GatewayErrorKind::Overloaded,
            "the machine is busy with another model — retry shortly",
        )
        .with_retry_after(retry_after_seconds));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel::records::{Modality, ModelSource, SourceKind};

    /// A minimal port that reports a fixed admission state and stubs the rest.
    struct StubPort {
        admission: GatewayAdmissionState,
    }

    impl GatewayPort for StubPort {
        fn shelf(&self) -> PortFuture<'_, Vec<ModelRecord>> {
            Box::pin(async { Vec::new() })
        }
        fn invoke<'a>(
            &'a self,
            _model_id: &'a str,
            _capability: Capability,
            _payload: JsonValue,
        ) -> PortFuture<'a, Result<ChunkStream, KernelError>> {
            Box::pin(async {
                let (_tx, stream) = ChunkStream::channel();
                Ok(stream)
            })
        }
        fn submit<'a>(
            &'a self,
            _model_id: &'a str,
            _capability: Capability,
            _payload: JsonValue,
        ) -> PortFuture<'a, Result<String, KernelError>> {
            Box::pin(async { Ok(String::new()) })
        }
        fn job<'a>(&'a self, _id: &'a str) -> PortFuture<'a, Option<Job>> {
            Box::pin(async { None })
        }
        fn job_events<'a>(
            &'a self,
            _id: &'a str,
        ) -> PortFuture<'a, mpsc::UnboundedReceiver<JobEvent>> {
            Box::pin(async { mpsc::unbounded_channel().1 })
        }
        fn cancel<'a>(&'a self, _job_id: &'a str) -> PortFuture<'a, ()> {
            Box::pin(async {})
        }
        fn voices<'a>(
            &'a self,
            _model_id: &'a str,
        ) -> PortFuture<'a, Result<Vec<String>, KernelError>> {
            Box::pin(async { Ok(Vec::new()) })
        }
        fn honored_params<'a>(
            &'a self,
            _model_id: &'a str,
            _capability: Capability,
        ) -> PortFuture<'a, Result<HashSet<String>, KernelError>> {
            Box::pin(async { Ok(HashSet::new()) })
        }
        fn artifact_data<'a>(
            &'a self,
            _id: &'a str,
        ) -> PortFuture<'a, Result<Option<Vec<u8>>, KernelError>> {
            Box::pin(async { Ok(None) })
        }
        fn admission_state<'a>(
            &'a self,
            _model_id: &'a str,
            _footprint_mb: Option<i64>,
            _kind: GatewayWorkKind,
        ) -> PortFuture<'a, GatewayAdmissionState> {
            let admission = self.admission;
            Box::pin(async move { admission })
        }
    }

    fn record() -> ModelRecord {
        ModelRecord::new(
            "m",
            Modality::text(),
            Vec::new(),
            ModelSource::new(SourceKind::ollama(), "m"),
        )
    }

    #[tokio::test]
    async fn a_ready_machine_admits_the_request() {
        let port = StubPort {
            admission: GatewayAdmissionState::Ready,
        };
        assert!(
            require_admission(&port, &record(), GatewayWorkKind::Stream)
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn a_saturated_machine_is_overloaded_with_a_retry_hint() {
        let port = StubPort {
            admission: GatewayAdmissionState::Saturated {
                retry_after_seconds: 3,
            },
        };
        let error = require_admission(&port, &record(), GatewayWorkKind::Stream)
            .await
            .unwrap_err();
        assert_eq!(error.kind, GatewayErrorKind::Overloaded);
        assert_eq!(error.retry_after_seconds, Some(3));
    }
}
