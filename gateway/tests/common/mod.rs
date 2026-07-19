//! A configurable [`GatewayPort`] double for exercising the resolver gate and the
//! handlers without a running kernel.

#![allow(dead_code)]

use std::collections::HashSet;

use gateway::admission::{GatewayAdmissionState, GatewayWorkKind};
use gateway::port::{GatewayPort, PortFuture};
use kernel::capabilities::CapabilityChunk;
use kernel::jobs::{Job, JobEvent};
use kernel::records::{
    Capability, JsonValue, Modality, ModelRecord, ModelSource, ModelState, SourceKind,
};
use runtime::adapters::ChunkStream;
use runtime::facade::KernelError;
use tokio::sync::mpsc;

/// A port whose behavior is fixed at construction.
pub struct MockPort {
    pub shelf: Vec<ModelRecord>,
    pub chunks: Vec<CapabilityChunk>,
    pub supports_tools: bool,
    pub honored: HashSet<String>,
    pub admission: GatewayAdmissionState,
}

impl Default for MockPort {
    fn default() -> Self {
        Self {
            shelf: Vec::new(),
            chunks: Vec::new(),
            supports_tools: false,
            honored: HashSet::new(),
            admission: GatewayAdmissionState::Ready,
        }
    }
}

impl MockPort {
    /// A port serving a single ready model with the given id/name.
    pub fn with_ready_model(name: &str) -> (Self, String) {
        let record = ready_model(name);
        let id = record.id.clone();
        (
            Self {
                shelf: vec![record],
                ..Self::default()
            },
            id,
        )
    }
}

/// A ready text model registered from an ollama tag.
pub fn ready_model(name: &str) -> ModelRecord {
    let mut record = ModelRecord::new(
        name,
        Modality::text(),
        Vec::new(),
        ModelSource::new(SourceKind::ollama(), name),
    );
    record.state = ModelState::Ready;
    record
}

impl GatewayPort for MockPort {
    fn shelf(&self) -> PortFuture<'_, Vec<ModelRecord>> {
        let shelf = self.shelf.clone();
        Box::pin(async move { shelf })
    }

    fn invoke<'a>(
        &'a self,
        _model_id: &'a str,
        _capability: Capability,
        _payload: JsonValue,
    ) -> PortFuture<'a, Result<ChunkStream, KernelError>> {
        let chunks = self.chunks.clone();
        Box::pin(async move {
            let (tx, stream) = ChunkStream::channel();
            for chunk in chunks {
                let _ = tx.send(Ok(chunk));
            }
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

    fn job_events<'a>(&'a self, _id: &'a str) -> PortFuture<'a, mpsc::UnboundedReceiver<JobEvent>> {
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

    fn supports_tools<'a>(&'a self, _model_id: &'a str) -> PortFuture<'a, bool> {
        let supports = self.supports_tools;
        Box::pin(async move { supports })
    }

    fn honored_params<'a>(
        &'a self,
        _model_id: &'a str,
        _capability: Capability,
    ) -> PortFuture<'a, Result<HashSet<String>, KernelError>> {
        let honored = self.honored.clone();
        Box::pin(async move { Ok(honored) })
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
