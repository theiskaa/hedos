//! Governed load helpers: the orchestration between the memory governor and the
//! sidecar supervisor. `warm_load_acquire` admits a model, holds the GPU gate
//! while it loads, marks it a governed resident, and hands back the producer
//! lease held for the generation; `governed_one_shot` brackets a one-off body in
//! a generation lease and the producer gate. The governor unit deferred these
//! until the sidecar existed.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use kernel::records::ModelRecord;

use crate::governor::{GateGuard, GpuProducer, MemoryGovernor, RamVerdict, RawUnloader};
use crate::sidecar::{SidecarError, SidecarSpec, SidecarSupervisor};

/// A status reporter for a governed load — a wait reason or a loading step. The
/// same shape as the governor's `OnWait` (`Send + Sync` so a holder can be
/// spawned).
pub type StatusFn<'a> = &'a (dyn Fn(&str) + Send + Sync);

/// Run `body` as a one-shot governed generation: take a generation lease, admit
/// the model, hold the producer gate for the body, then release the gate and end
/// the lease. The gate release and lease end happen on every path — normal
/// return, error, or the future being dropped mid-flight.
pub async fn governed_one_shot<T, E, F>(
    governor: &MemoryGovernor,
    record: &ModelRecord,
    producer: GpuProducer,
    status: StatusFn<'_>,
    body: F,
) -> Result<T, E>
where
    F: Future<Output = Result<T, E>>,
{
    governor.begin_generation(&record.id);
    let _lease = GenerationLease {
        governor,
        model_id: &record.id,
    };
    let _ = governor
        .admit(&record.id, &record.name, record.footprint_mb, Some(status))
        .await;
    let _gate = governor.gate().acquire(producer).await;
    body.await
}

/// Ensure `spec`'s sidecar is running as a governed resident and return the
/// producer gate lease to hold for the generation. Loops so that if residency
/// shut the sidecar down between the load and the producer acquisition, it
/// respawns. On a spawn failure the reservation is released so admission
/// accounting does not leak.
#[allow(clippy::too_many_arguments)]
pub async fn warm_load_acquire(
    governor: &MemoryGovernor,
    supervisor: &SidecarSupervisor,
    spec: &SidecarSpec,
    record: &ModelRecord,
    producer: GpuProducer,
    warm_window: Option<Duration>,
    starting_status: &str,
    status: StatusFn<'_>,
) -> Result<GateGuard, SidecarError> {
    loop {
        if !supervisor.is_running(&spec.runtime_id) {
            let verdict = governor
                .admit(&record.id, &record.name, record.footprint_mb, Some(status))
                .await;
            // `admit` reserves the model. If the future is dropped (cancelled) or
            // the spawn fails before `mark_loaded` swaps in the real resident, the
            // guard unloads the reservation so it doesn't leak as a phantom that
            // inflates every future RAM verdict.
            let mut reservation = ReservationGuard {
                governor,
                model_id: &record.id,
                armed: true,
            };
            if verdict == RamVerdict::Tight {
                status("Memory is tight — loading anyway");
            }
            status(starting_status);

            let load_gate = governor
                .gate()
                .acquire(GpuProducer::Load(record.id.clone()))
                .await;
            // On failure (or a `?` early return) the still-armed `reservation`
            // drops and unloads the reservation.
            supervisor.ensure_running(spec).await?;
            // `mark_loaded` must run while the Load gate is held so a concurrent
            // idle unload of the same model cannot interleave with the load.
            governor.mark_loaded(
                &record.id,
                &record.name,
                record.footprint_mb,
                warm_window,
                shutdown_unloader(supervisor, &spec.runtime_id),
            );
            reservation.disarm();
            drop(load_gate);
        }

        let producer_gate = governor.gate().acquire(producer.clone()).await;
        if supervisor.is_running(&spec.runtime_id) {
            return Ok(producer_gate);
        }
        drop(producer_gate);
    }
}

fn shutdown_unloader(supervisor: &SidecarSupervisor, runtime_id: &str) -> RawUnloader {
    let supervisor = supervisor.clone();
    let runtime_id = runtime_id.to_owned();
    Arc::new(move || {
        let supervisor = supervisor.clone();
        let runtime_id = runtime_id.clone();
        Box::pin(async move { supervisor.shutdown(&runtime_id).await })
    })
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

struct ReservationGuard<'a> {
    governor: &'a MemoryGovernor,
    model_id: &'a str,
    armed: bool,
}

impl ReservationGuard<'_> {
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for ReservationGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.governor.mark_unloaded(self.model_id);
        }
    }
}
