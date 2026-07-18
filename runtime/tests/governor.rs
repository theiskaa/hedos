//! Integration tests for the governor: the GPU gate (exclusion / sharing / FIFO /
//! cancel-safety), lease draining, warm-window residency (driven by paused time),
//! and `MemoryGovernor` admission and eviction. All run on a single-threaded,
//! time-paused runtime so waits and timers are deterministic.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use runtime::governor::{
    EvictionPolicy, GovernorConfig, GpuGate, GpuProducer, KeepWarmPolicy, MemoryGovernor,
    ModelLease, RamVerdict, ResidencyManager, ResidencyPolicy, Unloader, noop_unloader,
};
use tokio::time::{advance, timeout};

const SHORT: Duration = Duration::from_millis(50);

async fn settle() {
    for _ in 0..16 {
        tokio::task::yield_now().await;
    }
}

fn generation(model: &str) -> GpuProducer {
    GpuProducer::Generation(model.to_owned())
}

fn counting_unloader(counter: Arc<AtomicU32>, result: bool) -> Unloader {
    Arc::new(move || {
        let counter = Arc::clone(&counter);
        Box::pin(async move {
            counter.fetch_add(1, Ordering::Relaxed);
            result
        })
    })
}

// ---- gate ------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn gate_is_exclusive_for_different_producers() {
    let gate = GpuGate::new();
    let held = gate.acquire(GpuProducer::Load("a".to_owned())).await;
    assert!(
        timeout(SHORT, gate.acquire(GpuProducer::Job("b".to_owned())))
            .await
            .is_err()
    );
    drop(held);
    assert!(
        timeout(SHORT, gate.acquire(GpuProducer::Job("b".to_owned())))
            .await
            .is_ok()
    );
}

#[tokio::test(start_paused = true)]
async fn gate_shares_only_identical_generations() {
    let gate = GpuGate::new();
    let _a1 = gate.acquire(generation("a")).await;
    assert!(
        timeout(SHORT, gate.acquire(generation("a"))).await.is_ok(),
        "same-model gen shares"
    );
    assert!(
        timeout(SHORT, gate.acquire(generation("b"))).await.is_err(),
        "other-model gen waits"
    );
}

#[tokio::test(start_paused = true)]
async fn gate_is_fifo_and_a_shareable_producer_waits_behind_an_exclusive_one() {
    let gate = GpuGate::new();
    let held = gate.acquire(generation("a")).await;

    let gate2 = gate.clone();
    let waiter =
        tokio::spawn(async move { gate2.acquire(GpuProducer::Load("b".to_owned())).await });
    settle().await;

    assert!(
        timeout(SHORT, gate.acquire(generation("a"))).await.is_err(),
        "a shareable gen must queue behind the exclusive Load waiter"
    );
    drop(held);
    assert!(
        timeout(SHORT, waiter).await.unwrap().is_ok(),
        "the queued exclusive waiter runs next"
    );
}

#[tokio::test(start_paused = true)]
async fn gate_dropped_waiter_never_leaves_it_held() {
    let gate = GpuGate::new();
    let held = gate.acquire(GpuProducer::Load("a".to_owned())).await;
    assert!(
        timeout(SHORT, gate.acquire(GpuProducer::Load("b".to_owned())))
            .await
            .is_err()
    );
    drop(held);
    assert!(
        timeout(SHORT, gate.acquire(GpuProducer::Job("c".to_owned())))
            .await
            .is_ok(),
        "a cancelled waiter must not hold the gate"
    );
}

#[tokio::test(start_paused = true)]
async fn gate_with_access_releases_after_the_body() {
    let gate = GpuGate::new();
    gate.with_access(GpuProducer::Load("a".to_owned()), async {})
        .await;
    assert!(
        timeout(SHORT, gate.acquire(GpuProducer::Load("b".to_owned())))
            .await
            .is_ok()
    );
}

#[tokio::test(start_paused = true)]
async fn gate_frees_only_after_the_last_generation_holder() {
    let gate = GpuGate::new();
    let g1 = gate.acquire(generation("a")).await;
    let g2 = gate.acquire(generation("a")).await;
    let g3 = gate.acquire(generation("a")).await;

    drop(g1);
    assert!(
        timeout(SHORT, gate.acquire(GpuProducer::Load("b".to_owned())))
            .await
            .is_err(),
        "two share-holders remain"
    );
    drop(g2);
    assert!(
        timeout(SHORT, gate.acquire(GpuProducer::Load("b".to_owned())))
            .await
            .is_err(),
        "one share-holder remains"
    );
    drop(g3);
    assert!(
        timeout(SHORT, gate.acquire(GpuProducer::Load("b".to_owned())))
            .await
            .is_ok(),
        "the gate frees only after the last holder"
    );
}

#[tokio::test(start_paused = true)]
async fn gate_cancelled_head_waiter_lets_the_follower_through() {
    let gate = GpuGate::new();
    let held = gate.acquire(GpuProducer::Load("a".to_owned())).await;

    let gate2 = gate.clone();
    let head = tokio::spawn(async move { gate2.acquire(GpuProducer::Load("b".to_owned())).await });
    settle().await;
    let gate3 = gate.clone();
    let follower =
        tokio::spawn(async move { gate3.acquire(GpuProducer::Job("c".to_owned())).await });
    settle().await;

    head.abort();
    settle().await;
    drop(held);
    assert!(
        timeout(SHORT, follower).await.unwrap().is_ok(),
        "a dead head waiter is skipped and the follower is granted"
    );
}

// ---- lease -----------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn lease_counts_and_drains() {
    let lease = ModelLease::new();
    lease.acquire("a");
    lease.acquire("a");
    assert_eq!(lease.count("a"), 2);

    assert!(
        timeout(SHORT, lease.drain("a")).await.is_err(),
        "drain waits while count > 0"
    );
    lease.release("a");
    assert_eq!(lease.count("a"), 1);
    assert!(timeout(SHORT, lease.drain("a")).await.is_err());
    lease.release("a");
    assert_eq!(lease.count("a"), 0);
    assert!(
        timeout(SHORT, lease.drain("a")).await.is_ok(),
        "drain returns at zero"
    );
}

#[tokio::test(start_paused = true)]
async fn lease_release_wakes_a_drainer() {
    let lease = Arc::new(ModelLease::new());
    lease.acquire("a");
    let waiter = {
        let lease = Arc::clone(&lease);
        tokio::spawn(async move { lease.drain("a").await })
    };
    settle().await;
    assert!(!waiter.is_finished(), "drainer waits");
    lease.release("a");
    assert!(timeout(SHORT, waiter).await.unwrap().is_ok());
}

// ---- residency -------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn residency_fires_idle_unload_after_the_window() {
    let residency = ResidencyManager::new(Duration::from_secs(300));
    let fired = Arc::new(AtomicU32::new(0));
    residency.register("a", None, counting_unloader(Arc::clone(&fired), true));
    residency.schedule_idle_unload("a");
    settle().await;

    advance(Duration::from_secs(299)).await;
    settle().await;
    assert_eq!(fired.load(Ordering::Relaxed), 0, "not yet");

    advance(Duration::from_secs(2)).await;
    settle().await;
    assert_eq!(fired.load(Ordering::Relaxed), 1, "fires past the window");
}

#[tokio::test(start_paused = true)]
async fn residency_cancel_stops_the_timer() {
    let residency = ResidencyManager::new(Duration::from_secs(300));
    let fired = Arc::new(AtomicU32::new(0));
    residency.register("a", None, counting_unloader(Arc::clone(&fired), true));
    residency.schedule_idle_unload("a");
    residency.cancel_idle_unload("a");

    advance(Duration::from_secs(600)).await;
    settle().await;
    assert_eq!(fired.load(Ordering::Relaxed), 0);
}

#[tokio::test(start_paused = true)]
async fn residency_restores_a_refused_unloader() {
    let residency = ResidencyManager::new(Duration::from_secs(300));
    let fired = Arc::new(AtomicU32::new(0));
    residency.register("a", None, counting_unloader(Arc::clone(&fired), false));

    residency.unload_now("a").await;
    assert_eq!(fired.load(Ordering::Relaxed), 1, "attempted once");

    residency.unload_now("a").await;
    assert_eq!(
        fired.load(Ordering::Relaxed),
        2,
        "a refused unloader stays registered and retries"
    );
}

#[tokio::test(start_paused = true)]
async fn residency_cancelled_unload_restores_the_unloader() {
    let residency = Arc::new(ResidencyManager::new(Duration::from_secs(300)));
    let fired = Arc::new(AtomicU32::new(0));
    let block = Arc::new(tokio::sync::Semaphore::new(0));
    let unloader: Unloader = {
        let fired = Arc::clone(&fired);
        let block = Arc::clone(&block);
        Arc::new(move || {
            let fired = Arc::clone(&fired);
            let block = Arc::clone(&block);
            Box::pin(async move {
                let _permit = block.acquire().await;
                fired.fetch_add(1, Ordering::Relaxed);
                true
            })
        })
    };
    residency.register("a", None, unloader);

    let r2 = Arc::clone(&residency);
    let task = tokio::spawn(async move { r2.unload_now("a").await });
    settle().await;
    assert_eq!(fired.load(Ordering::Relaxed), 0, "the unloader is blocked");

    task.abort();
    settle().await;

    block.add_permits(1);
    timeout(SHORT, residency.unload_now("a")).await.unwrap();
    assert_eq!(
        fired.load(Ordering::Relaxed),
        1,
        "the cancelled unloader was restored and runs on retry"
    );
}

#[tokio::test(start_paused = true)]
async fn residency_concurrent_unload_runs_once() {
    let residency = Arc::new(ResidencyManager::new(Duration::from_secs(300)));
    let fired = Arc::new(AtomicU32::new(0));
    residency.register("a", None, counting_unloader(Arc::clone(&fired), true));

    let r1 = Arc::clone(&residency);
    let r2 = Arc::clone(&residency);
    let t1 = tokio::spawn(async move { r1.unload_now("a").await });
    let t2 = tokio::spawn(async move { r2.unload_now("a").await });
    timeout(SHORT, async {
        let _ = t1.await;
        let _ = t2.await;
    })
    .await
    .unwrap();
    assert_eq!(
        fired.load(Ordering::Relaxed),
        1,
        "serialized unloads: only one actually runs"
    );
}

#[tokio::test(start_paused = true)]
async fn residency_suspend_cancels_all_timers() {
    let residency = ResidencyManager::new(Duration::from_secs(300));
    let fired = Arc::new(AtomicU32::new(0));
    residency.register("a", None, counting_unloader(Arc::clone(&fired), true));
    residency.schedule_idle_unload("a");
    residency.suspend_all();

    advance(Duration::from_secs(600)).await;
    settle().await;
    assert_eq!(fired.load(Ordering::Relaxed), 0);
}

#[tokio::test(start_paused = true)]
async fn residency_per_model_window_overrides_default() {
    let residency = ResidencyManager::new(Duration::from_secs(300));
    let fired = Arc::new(AtomicU32::new(0));
    residency.register(
        "a",
        Some(Duration::from_secs(10)),
        counting_unloader(Arc::clone(&fired), true),
    );
    residency.schedule_idle_unload("a");
    settle().await;

    advance(Duration::from_secs(11)).await;
    settle().await;
    assert_eq!(
        fired.load(Ordering::Relaxed),
        1,
        "the 10s per-model window fires early"
    );
}

// ---- MemoryGovernor --------------------------------------------------------

fn governor() -> MemoryGovernor {
    MemoryGovernor::new(GovernorConfig::with_total_mb(262_144))
}

#[tokio::test(start_paused = true)]
async fn light_models_stack_without_eviction() {
    let gov = governor();
    gov.admit("a", "A", Some(256), None).await;
    gov.mark_loaded("a", "A", Some(256), None, noop_unloader());
    gov.admit("b", "B", Some(256), None).await;
    gov.mark_loaded("b", "B", Some(256), None, noop_unloader());
    assert!(gov.is_resident("a") && gov.is_resident("b"));
    assert_eq!(gov.resident().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn strict_single_evicts_the_prior_heavy_model() {
    let gov = governor();
    gov.admit("a", "A", Some(4096), None).await;
    gov.mark_loaded("a", "A", Some(4096), None, noop_unloader());
    assert!(gov.is_resident("a"));

    gov.admit("b", "B", Some(4096), None).await;
    assert!(!gov.is_resident("a"), "the first heavy model is evicted");
    assert!(gov.is_resident("b"));
}

#[tokio::test(start_paused = true)]
async fn admission_waits_for_a_busy_conflict_to_drain() {
    let gov = governor();
    gov.admit("a", "A", Some(4096), None).await;
    gov.mark_loaded("a", "A", Some(4096), None, noop_unloader());
    gov.begin_generation("a");

    let gov2 = gov.clone();
    let admit_b = tokio::spawn(async move { gov2.admit("b", "B", Some(4096), None).await });
    settle().await;
    assert!(
        !admit_b.is_finished(),
        "admission blocks while the conflict is leased"
    );
    assert!(gov.is_resident("a"));

    gov.end_generation("a");
    assert!(timeout(SHORT, admit_b).await.unwrap().is_ok());
    assert!(!gov.is_resident("a"), "the drained model is then evicted");
    assert!(gov.is_resident("b"));
}

#[tokio::test(start_paused = true)]
async fn concurrent_admit_of_the_same_model_stays_single() {
    let gov = governor();
    let g1 = gov.clone();
    let g2 = gov.clone();
    let a = tokio::spawn(async move { g1.admit("a", "A", Some(4096), None).await });
    let b = tokio::spawn(async move { g2.admit("a", "A", Some(4096), None).await });
    timeout(SHORT, async {
        let _ = a.await;
        let _ = b.await;
    })
    .await
    .unwrap();

    assert!(gov.is_resident("a"));
    assert_eq!(
        gov.resident().len(),
        1,
        "reserving the same model twice never self-evicts"
    );
}

#[tokio::test(start_paused = true)]
async fn tight_verdict_never_blocks() {
    let gov = MemoryGovernor::new(GovernorConfig::with_total_mb(1000));
    let verdict = gov.admit("a", "A", Some(900), None).await;
    assert_eq!(verdict, RamVerdict::Tight);
    assert!(gov.is_resident("a"), "a tight model loads anyway");
}

#[tokio::test(start_paused = true)]
async fn budgeted_policy_evicts_the_oldest() {
    let gov = governor();
    gov.apply(&ResidencyPolicy {
        keep_warm: KeepWarmPolicy::FiveMinutes,
        eviction: EvictionPolicy::Budgeted,
        ram_budget_mb: Some(1000),
    });
    gov.admit("a", "A", Some(400), None).await;
    gov.mark_loaded("a", "A", Some(400), None, noop_unloader());
    gov.admit("b", "B", Some(400), None).await;
    gov.mark_loaded("b", "B", Some(400), None, noop_unloader());
    gov.admit("c", "C", Some(400), None).await;

    assert!(
        !gov.is_resident("a"),
        "the oldest is evicted to fit the budget"
    );
    assert!(gov.is_resident("b") && gov.is_resident("c"));
}

#[tokio::test(start_paused = true)]
async fn idle_unload_fires_after_a_generation_ends() {
    let gov = governor();
    let unloaded = Arc::new(AtomicU32::new(0));
    let raw = {
        let unloaded = Arc::clone(&unloaded);
        Arc::new(move || {
            let unloaded = Arc::clone(&unloaded);
            Box::pin(async move {
                unloaded.fetch_add(1, Ordering::Relaxed);
            }) as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        })
    };
    gov.admit("a", "A", Some(256), None).await;
    gov.mark_loaded("a", "A", Some(256), None, raw);
    gov.begin_generation("a");
    gov.end_generation("a");
    settle().await;

    advance(Duration::from_secs(121)).await;
    settle().await;
    assert_eq!(unloaded.load(Ordering::Relaxed), 1);
    assert!(!gov.is_resident("a"));
}

#[tokio::test(start_paused = true)]
async fn observe_footprint_crossing_into_heavy_evicts_a_conflict() {
    let gov = governor();
    gov.admit("a", "A", Some(500), None).await;
    gov.mark_loaded("a", "A", Some(500), None, noop_unloader());
    gov.admit("b", "B", Some(500), None).await;
    gov.mark_loaded("b", "B", Some(500), None, noop_unloader());

    gov.observe_footprint("a", 4096);
    settle().await;
    assert!(
        gov.is_resident("a") && gov.is_resident("b"),
        "one heavy model, no conflict yet"
    );

    gov.observe_footprint("b", 4096);
    settle().await;
    assert!(
        !gov.is_resident("a"),
        "the older model that turned heavy is evicted"
    );
    assert!(gov.is_resident("b"));
}
