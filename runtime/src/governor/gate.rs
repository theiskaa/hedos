//! The GPU gate: a fair, mostly-exclusive lock. Any number of identical
//! `Generation(id)` producers may co-hold it (a model's concurrent generations —
//! serialization to the one engine context is the `GenerationSlot`'s job); every
//! other producer — a different model, any load/unload/job — is exclusive and
//! queues FIFO behind whoever holds it.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use tokio::sync::oneshot;

use crate::governor::lock;
use crate::governor::policy::GpuProducer;

#[derive(Default)]
struct GateState {
    holder: Option<GpuProducer>,
    holders: u32,
    waiters: VecDeque<Waiter>,
}

struct Waiter {
    producer: GpuProducer,
    sender: oneshot::Sender<GateGuard>,
}

/// A fair GPU gate.
#[derive(Clone, Default)]
pub struct GpuGate {
    state: Arc<Mutex<GateState>>,
}

impl GpuGate {
    /// A free gate.
    pub fn new() -> Self {
        Self::default()
    }

    /// Wait for and take the gate for `producer`. The returned guard releases it
    /// on drop. Dropping the future before it resolves cancels the wait and never
    /// leaves the gate held.
    pub async fn acquire(&self, producer: GpuProducer) -> GateGuard {
        let receiver = {
            let mut state = lock(&self.state);
            if state.waiters.is_empty() && admits(&state, &producer) {
                state.holder = Some(producer.clone());
                state.holders += 1;
                return self.guard(producer);
            }
            let (sender, receiver) = oneshot::channel();
            state.waiters.push_back(Waiter {
                producer: producer.clone(),
                sender,
            });
            receiver
        };
        // If this future is dropped while awaiting, `receiver` drops; a later
        // grant then fails to send and the guard is released. The `Err` arm is
        // unreachable in practice (the gate outlives the acquire, and a waiter is
        // only removed to be granted) — an inert guard keeps it panic-free.
        receiver.await.unwrap_or_else(|_| GateGuard {
            gate: Arc::clone(&self.state),
            producer,
            held: false,
        })
    }

    /// Take the gate for `producer`, run `body`, then release it — on both the
    /// success and panic paths.
    pub async fn with_access<F, T>(&self, producer: GpuProducer, body: F) -> T
    where
        F: std::future::Future<Output = T>,
    {
        let _guard = self.acquire(producer).await;
        body.await
    }

    fn guard(&self, producer: GpuProducer) -> GateGuard {
        GateGuard {
            gate: Arc::clone(&self.state),
            producer,
            held: true,
        }
    }
}

/// Proof that the holder currently owns the gate. Releasing on drop is the only
/// way the gate is freed.
pub struct GateGuard {
    gate: Arc<Mutex<GateState>>,
    producer: GpuProducer,
    held: bool,
}

impl GateGuard {
    fn into_producer(mut self) -> GpuProducer {
        self.held = false;
        self.producer.clone()
    }
}

impl Drop for GateGuard {
    fn drop(&mut self) {
        if !self.held {
            return;
        }
        self.held = false;
        release(&self.gate, &self.producer);
    }
}

fn admits(state: &GateState, producer: &GpuProducer) -> bool {
    match &state.holder {
        None => true,
        Some(holder) => holder == producer && producer.shares(),
    }
}

fn release(gate: &Arc<Mutex<GateState>>, producer: &GpuProducer) {
    // A failed grant send (its waiter was cancelled) is re-queued here rather than
    // dropped, so releasing under a cancellation storm iterates instead of
    // recursing through `GateGuard::drop`.
    let mut pending: Vec<GpuProducer> = vec![producer.clone()];
    while let Some(producer) = pending.pop() {
        let grants = {
            let mut state = lock(gate);
            match &state.holder {
                Some(holder) if holder == &producer && state.holders > 0 => {}
                _ => {
                    debug_assert!(false, "unbalanced gate release for {producer:?}");
                    continue;
                }
            }
            state.holders -= 1;
            if state.holders > 0 {
                continue;
            }
            state.holder = None;
            collect_admissions(&mut state, gate)
        };
        for (sender, guard) in grants {
            if let Err(returned) = sender.send(guard) {
                pending.push(returned.into_producer());
            }
        }
    }
}

fn collect_admissions(
    state: &mut GateState,
    gate: &Arc<Mutex<GateState>>,
) -> Vec<(oneshot::Sender<GateGuard>, GateGuard)> {
    let mut grants = Vec::new();
    loop {
        let admissible = match state.waiters.front() {
            Some(front) => admits(state, &front.producer),
            None => false,
        };
        if !admissible {
            break;
        }
        let Some(waiter) = state.waiters.pop_front() else {
            break;
        };
        state.holder = Some(waiter.producer.clone());
        state.holders += 1;
        let guard = GateGuard {
            gate: Arc::clone(gate),
            producer: waiter.producer,
            held: true,
        };
        grants.push((waiter.sender, guard));
    }
    grants
}
