//! Warm-window residency: after a model goes idle it is kept loaded for a window,
//! then an idle-unload timer fires. Unloads are serialized per model and an
//! unloader that refuses (because the model is still leased) is restored to retry.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::task::AbortHandle;

use crate::governor::{BoxFuture, lock};

/// An unload callback. Returns `true` if it actually unloaded, `false` if it
/// refused (e.g. the model is still in use) — a refused unloader is restored.
pub type Unloader = Arc<dyn Fn() -> BoxFuture<bool> + Send + Sync>;

struct State {
    default_warm_window: Duration,
    warm_windows: HashMap<String, Duration>,
    unloaders: HashMap<String, Unloader>,
    idle_tasks: HashMap<String, AbortHandle>,
    unload_locks: HashMap<String, Arc<tokio::sync::Mutex<()>>>,
    suspended: bool,
}

struct Inner {
    state: Mutex<State>,
}

/// Manages per-model idle-unload timers and warm windows. Cheap to clone (an
/// `Arc` handle), so timer tasks can hold their own reference.
#[derive(Clone)]
pub struct ResidencyManager {
    inner: Arc<Inner>,
}

impl ResidencyManager {
    /// A manager whose models default to `default_warm_window` of keep-warm.
    pub fn new(default_warm_window: Duration) -> Self {
        Self {
            inner: Arc::new(Inner {
                state: Mutex::new(State {
                    default_warm_window,
                    warm_windows: HashMap::new(),
                    unloaders: HashMap::new(),
                    idle_tasks: HashMap::new(),
                    unload_locks: HashMap::new(),
                    suspended: false,
                }),
            }),
        }
    }

    /// Register `model_id`'s unloader (and optional per-model warm window),
    /// cancelling any pending idle timer for it.
    pub fn register(&self, model_id: &str, warm_window: Option<Duration>, unloader: Unloader) {
        let mut state = lock(&self.inner.state);
        state.unloaders.insert(model_id.to_owned(), unloader);
        if let Some(window) = warm_window {
            state.warm_windows.insert(model_id.to_owned(), window);
        }
        if let Some(handle) = state.idle_tasks.remove(model_id) {
            handle.abort();
        }
    }

    /// Forget `model_id`: drop its unloader and warm window and cancel its timer.
    pub fn deregister(&self, model_id: &str) {
        let mut state = lock(&self.inner.state);
        state.unloaders.remove(model_id);
        state.warm_windows.remove(model_id);
        if let Some(handle) = state.idle_tasks.remove(model_id) {
            handle.abort();
        }
    }

    /// Arm an idle-unload timer for `model_id` that fires after its warm window.
    pub fn schedule_idle_unload(&self, model_id: &str) {
        // spawn + insert happen under the one lock (spawn is synchronous) so a
        // concurrent suspend/cancel/reschedule can't slip into a gap.
        let mut state = lock(&self.inner.state);
        if state.suspended || !state.unloaders.contains_key(model_id) {
            return;
        }
        if let Some(handle) = state.idle_tasks.remove(model_id) {
            handle.abort();
        }
        let window = warm_window(&state, model_id);
        let this = self.clone();
        let id = model_id.to_owned();
        let handle = tokio::spawn(async move {
            tokio::time::sleep(window).await;
            this.unload_now(&id).await;
        });
        state
            .idle_tasks
            .insert(model_id.to_owned(), handle.abort_handle());
    }

    /// Cancel `model_id`'s pending idle-unload timer, if any.
    pub fn cancel_idle_unload(&self, model_id: &str) {
        if let Some(handle) = lock(&self.inner.state).idle_tasks.remove(model_id) {
            handle.abort();
        }
    }

    /// Unload `model_id` now, serialized against any concurrent unload of the
    /// same model. If the unloader refuses (returns `false`), it is restored.
    pub async fn unload_now(&self, model_id: &str) {
        self.cancel_idle_unload(model_id);
        let per_model = {
            let mut state = lock(&self.inner.state);
            Arc::clone(state.unload_locks.entry(model_id.to_owned()).or_default())
        };
        let _permit = per_model.lock_owned().await;

        let unloader = {
            let mut state = lock(&self.inner.state);
            match state.unloaders.remove(model_id) {
                Some(unloader) => unloader,
                None => return,
            }
        };
        // If this future is dropped (or the unloader panics) mid-await, the guard
        // restores the unloader so the model is not left resident with no way to
        // unload it. On normal completion we disarm it and apply the real policy.
        let mut restore = RestoreUnloader {
            state: &self.inner.state,
            model_id,
            unloader: Some(Arc::clone(&unloader)),
        };
        let unloaded = unloader().await;
        restore.unloader = None;
        if !unloaded {
            let mut state = lock(&self.inner.state);
            state
                .unloaders
                .entry(model_id.to_owned())
                .or_insert(unloader);
        }
    }

    /// Set `model_id`'s warm window, rescheduling a pending timer to use it.
    pub fn set_warm_window(&self, model_id: &str, window: Duration) {
        let pending = {
            let mut state = lock(&self.inner.state);
            state.warm_windows.insert(model_id.to_owned(), window);
            state.idle_tasks.contains_key(model_id)
        };
        if pending {
            self.schedule_idle_unload(model_id);
        }
    }

    /// Change the default warm window, rescheduling pending timers that use it.
    pub fn set_default_warm_window(&self, window: Duration) {
        let to_reschedule: Vec<String> = {
            let mut state = lock(&self.inner.state);
            state.default_warm_window = window;
            state
                .idle_tasks
                .keys()
                .filter(|id| !state.warm_windows.contains_key(*id))
                .cloned()
                .collect()
        };
        for id in to_reschedule {
            self.schedule_idle_unload(&id);
        }
    }

    /// Cancel every idle timer and stop scheduling new ones (used on quit).
    pub fn suspend_all(&self) {
        let mut state = lock(&self.inner.state);
        state.suspended = true;
        for (_, handle) in state.idle_tasks.drain() {
            handle.abort();
        }
    }
}

struct RestoreUnloader<'a> {
    state: &'a Mutex<State>,
    model_id: &'a str,
    unloader: Option<Unloader>,
}

impl Drop for RestoreUnloader<'_> {
    fn drop(&mut self) {
        if let Some(unloader) = self.unloader.take() {
            lock(self.state)
                .unloaders
                .entry(self.model_id.to_owned())
                .or_insert(unloader);
        }
    }
}

fn warm_window(state: &State, model_id: &str) -> Duration {
    state
        .warm_windows
        .get(model_id)
        .copied()
        .unwrap_or(state.default_warm_window)
}
