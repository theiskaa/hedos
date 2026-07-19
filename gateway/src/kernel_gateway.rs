//! The concrete [`GatewayPort`] over the running kernel: thin delegations to the
//! runtime facade, with admission derived from the governor (streams) and the job
//! queue depth (jobs).

use std::collections::HashSet;
use std::sync::Arc;

use kernel::jobs::{Job, JobEvent};
use kernel::records::{Capability, JsonValue, ModelRecord};
use runtime::Kernel;
use runtime::adapters::ChunkStream;
use runtime::facade::KernelError;
use tokio::sync::mpsc;

use crate::admission::{GatewayAdmissionState, GatewayWorkKind};
use crate::defaults;
use crate::port::{GatewayPort, PortFuture};

/// A [`GatewayPort`] backed by a shared [`Kernel`].
#[derive(Clone)]
pub struct KernelGateway {
    kernel: Arc<Kernel>,
}

impl KernelGateway {
    /// Wrap a shared kernel as a gateway port.
    pub fn new(kernel: Arc<Kernel>) -> Self {
        Self { kernel }
    }
}

impl GatewayPort for KernelGateway {
    fn shelf(&self) -> PortFuture<'_, Vec<ModelRecord>> {
        Box::pin(self.kernel.shelf())
    }

    fn invoke<'a>(
        &'a self,
        model_id: &'a str,
        capability: Capability,
        payload: JsonValue,
    ) -> PortFuture<'a, Result<ChunkStream, KernelError>> {
        Box::pin(self.kernel.invoke(model_id, capability, payload))
    }

    fn submit<'a>(
        &'a self,
        model_id: &'a str,
        capability: Capability,
        payload: JsonValue,
    ) -> PortFuture<'a, Result<String, KernelError>> {
        Box::pin(self.kernel.submit(model_id, capability, payload))
    }

    fn job<'a>(&'a self, id: &'a str) -> PortFuture<'a, Option<Job>> {
        Box::pin(async move { self.kernel.scheduler().job(id) })
    }

    fn job_events<'a>(&'a self, id: &'a str) -> PortFuture<'a, mpsc::UnboundedReceiver<JobEvent>> {
        Box::pin(async move { self.kernel.scheduler().events(id) })
    }

    fn cancel<'a>(&'a self, job_id: &'a str) -> PortFuture<'a, ()> {
        Box::pin(async move { self.kernel.scheduler().cancel(job_id) })
    }

    fn voices<'a>(&'a self, model_id: &'a str) -> PortFuture<'a, Result<Vec<String>, KernelError>> {
        Box::pin(self.kernel.voices(model_id))
    }

    fn supports_tools<'a>(&'a self, model_id: &'a str) -> PortFuture<'a, bool> {
        Box::pin(self.kernel.supports_tools(model_id))
    }

    fn honored_params<'a>(
        &'a self,
        model_id: &'a str,
        capability: Capability,
    ) -> PortFuture<'a, Result<HashSet<String>, KernelError>> {
        Box::pin(self.kernel.honored_params(model_id, capability))
    }

    fn artifact_data<'a>(
        &'a self,
        id: &'a str,
    ) -> PortFuture<'a, Result<Option<Vec<u8>>, KernelError>> {
        Box::pin(self.kernel.artifact_data(id))
    }

    fn admission_state<'a>(
        &'a self,
        model_id: &'a str,
        footprint_mb: Option<i64>,
        kind: GatewayWorkKind,
    ) -> PortFuture<'a, GatewayAdmissionState> {
        Box::pin(async move {
            match kind {
                GatewayWorkKind::Stream => {
                    if self.kernel.governor().would_wait(model_id, footprint_mb) {
                        return GatewayAdmissionState::Saturated {
                            retry_after_seconds: defaults::SATURATED_RETRY_AFTER_SECONDS,
                        };
                    }
                }
                GatewayWorkKind::Job => {
                    if self.kernel.scheduler().queue_depth() >= defaults::INFERENCE_QUEUE_DEPTH_CAP
                    {
                        return GatewayAdmissionState::Saturated {
                            retry_after_seconds: defaults::QUEUED_RETRY_AFTER_SECONDS,
                        };
                    }
                }
            }
            GatewayAdmissionState::Ready
        })
    }
}
