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

use crate::governor::{BoxFuture, GateGuard, GpuProducer, MemoryGovernor, RamVerdict, RawUnloader};
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

/// Probe whether the engine currently holds this model loaded.
pub type LoadedProbe = Arc<dyn Fn() -> BoxFuture<bool> + Send + Sync>;
/// The id of the model the engine currently holds, if any (evicted before a load).
pub type PreviousModelProbe = Arc<dyn Fn() -> BoxFuture<Option<String>> + Send + Sync>;
/// Unload whatever the engine currently holds.
pub type UnloadPrevious = Arc<dyn Fn() -> BoxFuture<()> + Send + Sync>;
/// Load the model into the engine, failing with `E`.
pub type EngineLoader<E> = Arc<dyn Fn() -> BoxFuture<Result<(), E>> + Send + Sync>;
/// The engine's learned footprint for the model after loading, if known.
pub type FootprintProbe = Arc<dyn Fn() -> Option<i64> + Send + Sync>;

/// Everything [`acquire_loaded`] needs to bring one in-process engine model into
/// residency: the governor and generation producer, the model's identity and
/// footprint, and the engine callbacks that inspect, load, and evict it.
pub struct EngineLoad<'a, E> {
    /// The memory governor to admit and account the model through.
    pub governor: &'a MemoryGovernor,
    /// The generation producer to hold once the model is loaded.
    pub producer: GpuProducer,
    /// The model's stable id.
    pub model_id: &'a str,
    /// The model's display name.
    pub model_name: &'a str,
    /// The model's footprint estimate, if known.
    pub footprint_mb: Option<i64>,
    /// The status to report when admission is tight.
    pub tight_status: &'a str,
    /// The status reporter for wait reasons and loading steps.
    pub status: StatusFn<'a>,
    /// Whether the engine already holds this model.
    pub is_loaded: LoadedProbe,
    /// The model the engine currently holds, evicted before loading this one.
    pub previous_model_id: PreviousModelProbe,
    /// Unload the currently-held model.
    pub unload_previous: UnloadPrevious,
    /// Load this model.
    pub load: EngineLoader<E>,
    /// The governed unloader run when the governor evicts this model.
    pub evict: RawUnloader,
    /// The model's observed footprint after loading.
    pub observed_footprint_mb: FootprintProbe,
}

/// Ensure an in-process engine holds `load`'s model as a governed resident, then
/// acquire and return the generation producer gate to hold for the generation.
/// Loops so that if the governor evicted the model between the load and the
/// producer acquisition, it reloads.
pub async fn acquire_loaded<E>(load: EngineLoad<'_, E>) -> Result<GateGuard, E> {
    loop {
        ensure_loaded(&load).await?;
        let gate = load.governor.gate().acquire(load.producer.clone()).await;
        if (load.is_loaded)().await {
            return Ok(gate);
        }
        // A concurrent eviction unloaded it between the load and this acquire;
        // drop the gate and try again.
        drop(gate);
    }
}

async fn ensure_loaded<E>(load: &EngineLoad<'_, E>) -> Result<(), E> {
    if (load.is_loaded)().await {
        return Ok(());
    }
    let verdict = load
        .governor
        .admit(
            load.model_id,
            load.model_name,
            load.footprint_mb,
            Some(load.status),
        )
        .await;
    // `admit` reserves the model; if the load fails or the future is dropped
    // before `mark_loaded` swaps in the real resident, this guard unloads the
    // reservation so it does not leak as a phantom that inflates RAM verdicts.
    let mut reservation = ReservationGuard {
        governor: load.governor,
        model_id: load.model_id,
        armed: true,
    };
    if verdict == RamVerdict::Tight {
        (load.status)(load.tight_status);
    }
    // Hold the Load gate across the swap so a concurrent idle unload cannot
    // interleave with the load.
    let load_gate = load
        .governor
        .gate()
        .acquire(GpuProducer::Load(load.model_id.to_owned()))
        .await;
    if let Some(previous) = (load.previous_model_id)().await {
        (load.unload_previous)().await;
        load.governor.mark_unloaded(&previous);
    }
    // On an error the still-armed `reservation` drops (after `load_gate`),
    // unloading the reservation; the Load gate releases on the same unwind.
    (load.load)().await?;
    load.governor.mark_loaded(
        load.model_id,
        load.model_name,
        load.footprint_mb,
        None,
        Arc::clone(&load.evict),
    );
    reservation.disarm();
    drop(load_gate);
    if let Some(observed) = (load.observed_footprint_mb)() {
        load.governor.observe_footprint(load.model_id, observed);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::governor::{GovernorConfig, MemoryGovernor};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    fn governor() -> MemoryGovernor {
        MemoryGovernor::new(GovernorConfig::with_total_mb(262_144))
    }

    fn noop_status() -> StatusFn<'static> {
        &|_: &str| {}
    }

    fn noop_evict() -> RawUnloader {
        Arc::new(|| Box::pin(async {}))
    }

    fn never_previous() -> PreviousModelProbe {
        Arc::new(|| Box::pin(async { None }))
    }

    fn no_unload() -> UnloadPrevious {
        Arc::new(|| Box::pin(async {}))
    }

    fn no_footprint() -> FootprintProbe {
        Arc::new(|| None)
    }

    #[tokio::test]
    async fn an_already_loaded_model_skips_the_load() {
        let governor = governor();
        let loads = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&loads);
        let result = acquire_loaded::<()>(EngineLoad {
            governor: &governor,
            producer: GpuProducer::Generation("m".to_owned()),
            model_id: "m",
            model_name: "M",
            footprint_mb: Some(2048),
            tight_status: "tight",
            status: noop_status(),
            is_loaded: Arc::new(|| Box::pin(async { true })),
            previous_model_id: never_previous(),
            unload_previous: no_unload(),
            load: Arc::new(move || {
                counter.fetch_add(1, Ordering::SeqCst);
                Box::pin(async { Ok(()) })
            }),
            evict: noop_evict(),
            observed_footprint_mb: no_footprint(),
        })
        .await;
        assert!(result.is_ok());
        assert_eq!(loads.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn a_cold_model_is_loaded_and_becomes_resident() {
        let governor = governor();
        let loaded = Arc::new(AtomicBool::new(false));
        let probe = Arc::clone(&loaded);
        let setter = Arc::clone(&loaded);
        let result = acquire_loaded::<()>(EngineLoad {
            governor: &governor,
            producer: GpuProducer::Generation("m".to_owned()),
            model_id: "m",
            model_name: "M",
            footprint_mb: Some(2048),
            tight_status: "tight",
            status: noop_status(),
            is_loaded: Arc::new(move || {
                let probe = Arc::clone(&probe);
                Box::pin(async move { probe.load(Ordering::SeqCst) })
            }),
            previous_model_id: never_previous(),
            unload_previous: no_unload(),
            load: Arc::new(move || {
                let setter = Arc::clone(&setter);
                Box::pin(async move {
                    setter.store(true, Ordering::SeqCst);
                    Ok(())
                })
            }),
            evict: noop_evict(),
            observed_footprint_mb: no_footprint(),
        })
        .await;
        assert!(result.is_ok());
        assert!(governor.is_resident("m"));
    }

    #[tokio::test]
    async fn a_load_failure_leaves_no_phantom_resident() {
        let governor = governor();
        let result = acquire_loaded::<&'static str>(EngineLoad {
            governor: &governor,
            producer: GpuProducer::Generation("m".to_owned()),
            model_id: "m",
            model_name: "M",
            footprint_mb: Some(2048),
            tight_status: "tight",
            status: noop_status(),
            is_loaded: Arc::new(|| Box::pin(async { false })),
            previous_model_id: never_previous(),
            unload_previous: no_unload(),
            load: Arc::new(|| Box::pin(async { Err("boom") })),
            evict: noop_evict(),
            observed_footprint_mb: no_footprint(),
        })
        .await;
        assert_eq!(result.err(), Some("boom"));
        assert!(!governor.is_resident("m"));
    }

    #[tokio::test]
    async fn a_previous_model_is_evicted_before_the_new_one_loads() {
        use std::sync::Mutex;
        let governor = governor();
        // "old" is genuinely resident, so `mark_unloaded(previous)` has an effect.
        governor.mark_loaded("old", "Old", Some(2048), None, noop_evict());
        assert!(governor.is_resident("old"));

        // Record the call order of unload_previous vs load.
        let log: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
        let unload_log = Arc::clone(&log);
        let load_log = Arc::clone(&log);
        let loaded_new = Arc::new(AtomicBool::new(false));
        let probe = Arc::clone(&loaded_new);
        let setter = Arc::clone(&loaded_new);

        let result = acquire_loaded::<()>(EngineLoad {
            governor: &governor,
            producer: GpuProducer::Generation("new".to_owned()),
            model_id: "new",
            model_name: "New",
            footprint_mb: Some(2048),
            tight_status: "tight",
            status: noop_status(),
            is_loaded: Arc::new(move || {
                let probe = Arc::clone(&probe);
                Box::pin(async move { probe.load(Ordering::SeqCst) })
            }),
            previous_model_id: Arc::new(|| Box::pin(async { Some("old".to_owned()) })),
            unload_previous: Arc::new(move || {
                let unload_log = Arc::clone(&unload_log);
                Box::pin(async move {
                    unload_log.lock().unwrap().push("unload");
                })
            }),
            load: Arc::new(move || {
                let load_log = Arc::clone(&load_log);
                let setter = Arc::clone(&setter);
                Box::pin(async move {
                    load_log.lock().unwrap().push("load");
                    setter.store(true, Ordering::SeqCst);
                    Ok(())
                })
            }),
            evict: noop_evict(),
            observed_footprint_mb: no_footprint(),
        })
        .await;

        assert!(result.is_ok());
        // "old" was evicted from residency, "new" took its place.
        assert!(!governor.is_resident("old"));
        assert!(governor.is_resident("new"));
        // The previous model was unloaded before the new one loaded.
        assert_eq!(*log.lock().unwrap(), vec!["unload", "load"]);
    }
}
