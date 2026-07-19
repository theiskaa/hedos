//! Load-shedding primitives: the kind of work a request represents, whether the
//! machine can admit it, and the in-flight counter that caps concurrent
//! inference. The async backpressure check that consults the port lands with the
//! port bridge.

use std::sync::atomic::{AtomicUsize, Ordering};

/// The kind of work a request drives, which the governor admits differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GatewayWorkKind {
    /// A streaming inference (chat, completion).
    Stream,
    /// A long-running job that produces an artifact.
    Job,
}

/// Whether the machine can admit a request now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GatewayAdmissionState {
    /// The request can proceed.
    Ready,
    /// The machine is busy; retry after the given number of seconds.
    Saturated {
        /// How long the client should wait before retrying.
        retry_after_seconds: u32,
    },
}

/// A concurrency limiter: a count of in-flight requests capped at a limit.
///
/// `enter` admits a request only while the count is below the limit; each
/// admitted request must be balanced by an `exit`. Lock-free via an atomic
/// compare-and-swap, replacing the Swift lock-guarded counter.
#[derive(Debug, Default)]
pub struct GatewayCounter {
    count: AtomicUsize,
}

impl GatewayCounter {
    /// A counter starting at zero.
    pub fn new() -> Self {
        Self::default()
    }

    /// Admit a request if fewer than `limit` are in flight, incrementing the
    /// count and returning `true`; otherwise leave the count and return `false`.
    pub fn enter(&self, limit: usize) -> bool {
        let mut current = self.count.load(Ordering::Acquire);
        loop {
            if current >= limit {
                return false;
            }
            match self.count.compare_exchange_weak(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(actual) => current = actual,
            }
        }
    }

    /// Release an admitted request, decrementing the count.
    pub fn exit(&self) {
        self.count.fetch_sub(1, Ordering::AcqRel);
    }

    /// The current number of in-flight requests.
    pub fn in_flight(&self) -> usize {
        self.count.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn enter_admits_up_to_the_limit_then_rejects() {
        let counter = GatewayCounter::new();
        assert!(counter.enter(2));
        assert!(counter.enter(2));
        assert!(!counter.enter(2));
        assert_eq!(counter.in_flight(), 2);
    }

    #[test]
    fn exit_frees_a_slot() {
        let counter = GatewayCounter::new();
        assert!(counter.enter(1));
        assert!(!counter.enter(1));
        counter.exit();
        assert!(counter.enter(1));
    }

    #[test]
    fn concurrent_enters_never_exceed_the_limit() {
        let counter = Arc::new(GatewayCounter::new());
        let limit = 4;
        let admitted = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();
        for _ in 0..32 {
            let counter = Arc::clone(&counter);
            let admitted = Arc::clone(&admitted);
            handles.push(std::thread::spawn(move || {
                if counter.enter(limit) {
                    admitted.fetch_add(1, Ordering::AcqRel);
                }
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }
        // Never more than `limit` were admitted, and the count matches.
        assert_eq!(admitted.load(Ordering::Acquire), limit);
        assert_eq!(counter.in_flight(), limit);
    }

    #[test]
    fn the_admission_state_carries_a_retry_hint() {
        let saturated = GatewayAdmissionState::Saturated {
            retry_after_seconds: 5,
        };
        assert_ne!(saturated, GatewayAdmissionState::Ready);
        assert_eq!(GatewayWorkKind::Stream, GatewayWorkKind::Stream);
    }
}
