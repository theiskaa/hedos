//! Memory and GPU governance: admission against a RAM budget, a fair GPU gate,
//! per-model leases, and warm-window residency. Every engine funnels model
//! loading and generation through here.
//!
//! Each Swift `actor` in the original maps to a struct whose synchronous state
//! lives behind a `std::sync::Mutex` that is **never held across an `.await`** —
//! sub-components (`gate`/`leases`/`residency`) are awaited with the lock
//! dropped, reproducing the reentrancy a Swift actor gets for free.

pub mod gate;
pub mod generation_slot;
pub mod lease;
pub mod memory;
pub mod policy;
pub mod residency;

pub use gate::{GateGuard, GpuGate};
pub use generation_slot::GenerationSlot;
pub use lease::ModelLease;
pub use memory::{GovernorConfig, MemoryGovernor, OnWait, RawUnloader, noop_unloader};
pub use policy::{
    EvictionPolicy, GpuProducer, KeepWarmPolicy, RamVerdict, ResidencyPolicy, ResidentModel,
};
pub use residency::{ResidencyManager, Unloader};

use std::future::Future;
use std::pin::Pin;
use std::sync::{Mutex, MutexGuard, PoisonError};

/// A boxed, sendable future — the shape returned by the governor's async
/// unload callbacks.
pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// Lock a `std::sync::Mutex`, recovering the guard if a previous holder panicked
/// rather than propagating the poison (the governor's critical sections never
/// panic, so poison should not arise; this keeps the no-`unwrap` rule).
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
