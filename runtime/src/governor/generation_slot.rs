//! A per-engine binary lock that serializes two concurrent generations of the
//! same model. They share the GPU gate (both are `Generation(id)`) but must not
//! share the single engine context, so the second waits here.

use std::sync::Arc;

use tokio::sync::{Mutex, OwnedMutexGuard};

/// A fair one-at-a-time slot. Acquire it only while already holding the model's
/// `Generation` gate; drop the returned guard before releasing the gate.
#[derive(Clone, Default)]
pub struct GenerationSlot {
    lock: Arc<Mutex<()>>,
}

impl GenerationSlot {
    /// A fresh, unheld slot.
    pub fn new() -> Self {
        Self::default()
    }

    /// Wait for and take the slot. The returned guard releases it when dropped.
    pub async fn acquire(&self) -> OwnedMutexGuard<()> {
        Arc::clone(&self.lock).lock_owned().await
    }
}
