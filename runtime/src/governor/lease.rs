//! Per-model in-flight lease counting. A model with a live lease cannot be
//! evicted; `drain` waits for every lease on a model to be released.

use std::collections::HashMap;
use std::sync::Mutex;

use tokio::sync::Notify;

use crate::governor::lock;

/// A per-model reference count of in-flight uses. Balancing `acquire`/`release`
/// is the caller's responsibility (the governor pairs them in
/// `begin_generation`/`end_generation`); this is not an RAII handle.
#[derive(Default)]
pub struct ModelLease {
    counts: Mutex<HashMap<String, u32>>,
    zeroed: Notify,
}

impl ModelLease {
    /// A lease store with no active leases.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one more in-flight use of `model_id`.
    pub fn acquire(&self, model_id: &str) {
        *lock(&self.counts).entry(model_id.to_owned()).or_insert(0) += 1;
    }

    /// Release one in-flight use. When a model's count reaches zero its entry is
    /// removed and any `drain` waiters are woken.
    pub fn release(&self, model_id: &str) {
        let reached_zero = {
            let mut counts = lock(&self.counts);
            match counts.get_mut(model_id) {
                Some(count) if *count > 0 => {
                    *count -= 1;
                    if *count == 0 {
                        counts.remove(model_id);
                        true
                    } else {
                        false
                    }
                }
                _ => false,
            }
        };
        if reached_zero {
            self.zeroed.notify_waiters();
        }
    }

    /// The current in-flight count for `model_id`.
    pub fn count(&self, model_id: &str) -> u32 {
        lock(&self.counts).get(model_id).copied().unwrap_or(0)
    }

    /// Wait until `model_id` has no in-flight uses. Returns immediately if it is
    /// already zero. Cancel-safe: dropping the future deregisters the waiter.
    pub async fn drain(&self, model_id: &str) {
        loop {
            let notified = self.zeroed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.count(model_id) == 0 {
                return;
            }
            notified.await;
        }
    }
}
