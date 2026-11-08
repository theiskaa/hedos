//! The concrete [`GatewayPort`] over the running kernel: thin delegations to the
//! runtime facade, with admission derived from the governor (streams) and the job
//! queue depth (jobs).

use std::collections::HashSet;
use std::sync::Arc;

use kernel::jobs::{Job, JobEvent};
use kernel::profiles::Verdict;
use kernel::records::{Capability, JsonValue, ModelRecord};
use runtime::Kernel;
use runtime::adapters::ChunkStream;
use runtime::facade::KernelError;
use tokio::sync::mpsc;

use crate::admission::{GatewayAdmissionState, GatewayWorkKind};
use crate::auto_route::AutoRoute;
use crate::defaults;
use crate::port::{GatewayPort, GatewayResident, PortFuture};

/// A [`GatewayPort`] backed by a shared [`Kernel`].
#[derive(Clone)]
pub struct KernelGateway {
    kernel: Arc<Kernel>,
    auto_route: Option<Arc<AutoRoute>>,
}

impl KernelGateway {
    /// Wrap a shared kernel as a gateway port.
    pub fn new(kernel: Arc<Kernel>) -> Self {
        Self {
            kernel,
            auto_route: None,
        }
    }

    /// The same port, serving `auto_route` (or no routing, for `None`).
    pub fn with_auto_route(mut self, auto_route: Option<AutoRoute>) -> Self {
        self.auto_route = auto_route.map(Arc::new);
        self
    }
}

impl GatewayPort for KernelGateway {
    fn shelf(&self) -> PortFuture<'_, Arc<[ModelRecord]>> {
        Box::pin(self.kernel.shelf())
    }

    fn resident(&self) -> PortFuture<'_, Vec<GatewayResident>> {
        Box::pin(async move {
            // A resident without a record id can't be matched to the shelf.
            self.kernel
                .resident_models()
                .into_iter()
                .filter_map(|entry| {
                    Some(GatewayResident {
                        model_id: entry.model_id?,
                        name: entry.name,
                        footprint_mb: entry.footprint_mb,
                        expires_at_millis: entry.expires_at_millis,
                    })
                })
                .collect()
        })
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

    fn abandon(&self, job_id: &str) {
        self.kernel.scheduler().cancel(job_id);
    }

    fn voices<'a>(&'a self, model_id: &'a str) -> PortFuture<'a, Result<Vec<String>, KernelError>> {
        Box::pin(self.kernel.voices(model_id))
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

    fn served_window(&self, record: &ModelRecord) -> Option<i64> {
        self.kernel
            .embedding_window(record)
            .or(record.context_length)
    }

    fn prompt_fit(
        &self,
        record: &ModelRecord,
        capability: &Capability,
        payload: &JsonValue,
        uncounted: i64,
    ) -> Verdict {
        self.kernel
            .assess_prompt(record, capability, payload, uncounted)
    }

    fn auto_route(&self) -> Option<&AutoRoute> {
        self.auto_route.as_deref()
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
