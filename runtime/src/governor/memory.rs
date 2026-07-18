//! The admission authority. Decides whether a model may load (evicting others
//! per the policy), tracks the resident set, and pairs generation leases with
//! warm-window residency.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::governor::gate::GpuGate;
use crate::governor::lease::ModelLease;
use crate::governor::policy::{
    EvictionPolicy, GpuProducer, RamVerdict, ResidencyPolicy, ResidentModel,
};
use crate::governor::residency::{ResidencyManager, Unloader};
use crate::governor::{BoxFuture, lock};

const FALLBACK_TOTAL_MB: i64 = 8192;

/// The raw "free the weights" callback a model registers when it loads.
pub type RawUnloader = Arc<dyn Fn() -> BoxFuture<()> + Send + Sync>;

/// A status callback invoked while admission waits to evict a busy model.
pub type OnWait<'a> = &'a (dyn Fn(&str) + Sync);

/// An unloader that does nothing — used to reserve a model before its weights
/// exist.
pub fn noop_unloader() -> RawUnloader {
    Arc::new(|| Box::pin(async {}))
}

/// Tunables for a [`MemoryGovernor`].
#[derive(Debug, Clone)]
pub struct GovernorConfig {
    /// Total system memory in megabytes.
    pub total_memory_mb: i64,
    /// Footprint at or above which a model is "heavy" and serialized.
    pub heavy_threshold_mb: i64,
    /// Fraction of total memory that is the "comfortable" ceiling.
    pub tight_fraction: f64,
    /// Default keep-warm window before an idle unload.
    pub default_warm_window: Duration,
}

impl GovernorConfig {
    /// A config with the given total memory and standard defaults (heavy at
    /// 1 GB, ceiling at 80%, 120 s warm window).
    pub fn with_total_mb(total_memory_mb: i64) -> Self {
        Self {
            total_memory_mb,
            heavy_threshold_mb: 1024,
            tight_fraction: 0.8,
            default_warm_window: Duration::from_secs(120),
        }
    }

    /// A config with the machine's detected total memory.
    pub fn detect() -> Self {
        Self::with_total_mb(detect_total_memory_mb())
    }
}

struct GovernorState {
    residents: HashMap<String, ResidentModel>,
    eviction_policy: EvictionPolicy,
    ram_budget_mb: Option<i64>,
}

struct Inner {
    total_memory_mb: i64,
    heavy_threshold_mb: i64,
    tight_fraction: f64,
    gate: GpuGate,
    leases: Arc<ModelLease>,
    residency: ResidencyManager,
    state: Mutex<GovernorState>,
    admission: tokio::sync::Mutex<()>,
    load_order: AtomicU64,
}

/// The memory/GPU admission authority. Cheap to clone (an `Arc` handle).
#[derive(Clone)]
pub struct MemoryGovernor {
    inner: Arc<Inner>,
}

impl MemoryGovernor {
    /// Build a governor from `config`.
    pub fn new(config: GovernorConfig) -> Self {
        Self {
            inner: Arc::new(Inner {
                total_memory_mb: config.total_memory_mb,
                heavy_threshold_mb: config.heavy_threshold_mb,
                tight_fraction: config.tight_fraction,
                gate: GpuGate::new(),
                leases: Arc::new(ModelLease::new()),
                residency: ResidencyManager::new(config.default_warm_window),
                state: Mutex::new(GovernorState {
                    residents: HashMap::new(),
                    eviction_policy: EvictionPolicy::StrictSingle,
                    ram_budget_mb: None,
                }),
                admission: tokio::sync::Mutex::new(()),
                load_order: AtomicU64::new(0),
            }),
        }
    }

    /// The GPU gate.
    pub fn gate(&self) -> &GpuGate {
        &self.inner.gate
    }

    /// The lease store.
    pub fn leases(&self) -> &ModelLease {
        &self.inner.leases
    }

    /// The residency manager.
    pub fn residency(&self) -> &ResidencyManager {
        &self.inner.residency
    }

    /// Admit a model to load. Light models under the strict-single policy stack
    /// freely; heavy models (or any model under the budgeted policy) serialize,
    /// evict conflicts (draining their leases first), and then reserve. Returns
    /// the advisory RAM verdict; `Tight` never blocks or evicts.
    pub async fn admit(
        &self,
        model_id: &str,
        name: &str,
        footprint_mb: Option<i64>,
        on_wait: Option<OnWait<'_>>,
    ) -> RamVerdict {
        if !self.is_heavy(footprint_mb) && self.eviction_policy() == EvictionPolicy::StrictSingle {
            let verdict = self.verdict(footprint_mb, Some(model_id));
            self.reserve(model_id, name, footprint_mb);
            return verdict;
        }
        let _admission = self.inner.admission.lock().await;
        self.settle_conflicts(model_id, footprint_mb, on_wait).await;
        let verdict = self.verdict(footprint_mb, Some(model_id));
        self.reserve(model_id, name, footprint_mb);
        verdict
    }

    /// Record that a model's weights are loaded, swapping in its real footprint
    /// and unloader (over the no-op registered at reserve).
    ///
    /// Callers must invoke this (and [`mark_unloaded`](Self::mark_unloaded))
    /// while holding the model's `Load`/`Unload` gate section, so a concurrent
    /// idle unload of the same model cannot interleave with the load.
    pub fn mark_loaded(
        &self,
        model_id: &str,
        name: &str,
        footprint_mb: Option<i64>,
        warm_window: Option<Duration>,
        unloader: RawUnloader,
    ) {
        {
            let mut state = lock(&self.inner.state);
            let loaded_at = self.inner.load_order.fetch_add(1, Ordering::Relaxed);
            state.residents.insert(
                model_id.to_owned(),
                ResidentModel {
                    model_id: model_id.to_owned(),
                    name: name.to_owned(),
                    footprint_mb: footprint_mb.unwrap_or(self.inner.heavy_threshold_mb),
                    loaded_at,
                },
            );
        }
        self.register_unloader(model_id, warm_window, unloader);
    }

    /// Drop a model from the resident set and forget its unloader.
    pub fn mark_unloaded(&self, model_id: &str) {
        lock(&self.inner.state).residents.remove(model_id);
        self.inner.residency.deregister(model_id);
    }

    /// Take a generation lease and cancel any pending idle unload.
    pub fn begin_generation(&self, model_id: &str) {
        self.inner.leases.acquire(model_id);
        self.inner.residency.cancel_idle_unload(model_id);
    }

    /// Release a generation lease; when the last one drops, arm an idle unload.
    pub fn end_generation(&self, model_id: &str) {
        self.inner.leases.release(model_id);
        if self.inner.leases.count(model_id) == 0 && self.is_resident(model_id) {
            self.inner.residency.schedule_idle_unload(model_id);
        }
    }

    /// Update a model's learned footprint. Crossing into "heavy" triggers a
    /// background re-admit to evict any now-conflicting resident.
    pub fn observe_footprint(&self, model_id: &str, footprint_mb: i64) {
        let crossed = {
            let mut state = lock(&self.inner.state);
            let Some(resident) = state.residents.get_mut(model_id) else {
                return;
            };
            let previous = resident.footprint_mb;
            resident.footprint_mb = footprint_mb;
            previous < self.inner.heavy_threshold_mb
                && footprint_mb >= self.inner.heavy_threshold_mb
        };
        if !crossed {
            return;
        }
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let this = self.clone();
        let model_id = model_id.to_owned();
        handle.spawn(async move {
            this.readmit(&model_id, footprint_mb).await;
        });
    }

    /// The advisory RAM verdict for admitting a model of `footprint_mb`.
    pub fn verdict(&self, footprint_mb: Option<i64>, model_id: Option<&str>) -> RamVerdict {
        let incoming = footprint_mb.unwrap_or(self.inner.heavy_threshold_mb);
        let state = lock(&self.inner.state);
        let resident: i64 = state
            .residents
            .values()
            .filter(|resident| Some(resident.model_id.as_str()) != model_id)
            .map(|resident| resident.footprint_mb)
            .sum();
        let ceiling = self.inner.total_memory_mb as f64 * self.inner.tight_fraction;
        if (resident + incoming) as f64 > ceiling {
            RamVerdict::Tight
        } else {
            RamVerdict::Ok
        }
    }

    /// A snapshot of the resident models.
    pub fn resident(&self) -> Vec<ResidentModel> {
        lock(&self.inner.state)
            .residents
            .values()
            .cloned()
            .collect()
    }

    /// Whether `model_id` is currently resident.
    pub fn is_resident(&self, model_id: &str) -> bool {
        lock(&self.inner.state).residents.contains_key(model_id)
    }

    /// Whether admitting `model_id` would have to wait for a busy conflict.
    pub fn would_wait(&self, model_id: &str, footprint_mb: Option<i64>) -> bool {
        if !self.is_heavy(footprint_mb) && self.eviction_policy() == EvictionPolicy::StrictSingle {
            return false;
        }
        match self.eviction_conflict(model_id, footprint_mb) {
            Some(conflict) => self.inner.leases.count(&conflict.model_id) > 0,
            None => false,
        }
    }

    /// Apply a residency policy (eviction strategy, budget, keep-warm window).
    pub fn apply(&self, policy: &ResidencyPolicy) {
        {
            let mut state = lock(&self.inner.state);
            state.eviction_policy = policy.eviction;
            state.ram_budget_mb = policy.ram_budget_mb;
        }
        self.inner
            .residency
            .set_default_warm_window(policy.keep_warm.warm_window());
    }

    /// Set a per-model keep-warm window.
    pub fn set_warm_window(&self, model_id: &str, window: Duration) {
        self.inner.residency.set_warm_window(model_id, window);
    }

    /// Cancel all idle timers and stop scheduling new ones (on quit).
    pub fn suspend_for_quit(&self) {
        self.inner.residency.suspend_all();
    }

    async fn readmit(&self, model_id: &str, footprint_mb: i64) {
        let _admission = self.inner.admission.lock().await;
        self.settle_conflicts(model_id, Some(footprint_mb), None)
            .await;
    }

    async fn settle_conflicts(
        &self,
        model_id: &str,
        footprint_mb: Option<i64>,
        on_wait: Option<OnWait<'_>>,
    ) {
        while let Some(conflict) = self.eviction_conflict(model_id, footprint_mb) {
            if self.inner.leases.count(&conflict.model_id) > 0 {
                if let Some(callback) = on_wait {
                    callback(&format!("Waiting for {} to finish", conflict.name));
                }
                self.inner.leases.drain(&conflict.model_id).await;
            }
            self.inner.residency.unload_now(&conflict.model_id).await;
            // If the conflict is still resident with no leases, the unload made
            // no progress (a missing/failed unloader); stop rather than spin.
            if self.is_resident(&conflict.model_id)
                && self.inner.leases.count(&conflict.model_id) == 0
            {
                break;
            }
        }
    }

    fn reserve(&self, model_id: &str, name: &str, footprint_mb: Option<i64>) {
        {
            let mut state = lock(&self.inner.state);
            if state.residents.contains_key(model_id) {
                return;
            }
            let loaded_at = self.inner.load_order.fetch_add(1, Ordering::Relaxed);
            state.residents.insert(
                model_id.to_owned(),
                ResidentModel {
                    model_id: model_id.to_owned(),
                    name: name.to_owned(),
                    footprint_mb: footprint_mb.unwrap_or(self.inner.heavy_threshold_mb),
                    loaded_at,
                },
            );
        }
        self.register_unloader(model_id, None, noop_unloader());
    }

    fn register_unloader(&self, model_id: &str, warm_window: Option<Duration>, raw: RawUnloader) {
        let gate = self.inner.gate.clone();
        let leases = Arc::clone(&self.inner.leases);
        let weak = Arc::downgrade(&self.inner);
        let id = model_id.to_owned();
        let governed: Unloader = Arc::new(move || {
            let gate = gate.clone();
            let leases = Arc::clone(&leases);
            let weak = weak.clone();
            let raw = Arc::clone(&raw);
            let id = id.clone();
            Box::pin(async move {
                let unload_id = id;
                gate.with_access(GpuProducer::Unload(unload_id.clone()), async move {
                    if leases.count(&unload_id) > 0 {
                        return false;
                    }
                    raw().await;
                    if let Some(inner) = weak.upgrade() {
                        MemoryGovernor { inner }.mark_unloaded(&unload_id);
                    }
                    true
                })
                .await
            })
        });
        self.inner
            .residency
            .register(model_id, warm_window, governed);
    }

    fn eviction_conflict(
        &self,
        model_id: &str,
        footprint_mb: Option<i64>,
    ) -> Option<ResidentModel> {
        let state = lock(&self.inner.state);
        match state.eviction_policy {
            EvictionPolicy::StrictSingle => state
                .residents
                .values()
                .find(|resident| {
                    resident.model_id != model_id
                        && resident.footprint_mb >= self.inner.heavy_threshold_mb
                })
                .cloned(),
            EvictionPolicy::Budgeted => {
                let budget = state.ram_budget_mb.unwrap_or_else(|| self.default_budget());
                let incoming = footprint_mb.unwrap_or(self.inner.heavy_threshold_mb);
                let occupied: i64 = state
                    .residents
                    .values()
                    .filter(|resident| resident.model_id != model_id)
                    .map(|resident| resident.footprint_mb)
                    .sum();
                if occupied + incoming <= budget {
                    return None;
                }
                state
                    .residents
                    .values()
                    .filter(|resident| resident.model_id != model_id)
                    .min_by(|a, b| (a.loaded_at, &a.model_id).cmp(&(b.loaded_at, &b.model_id)))
                    .cloned()
            }
        }
    }

    fn eviction_policy(&self) -> EvictionPolicy {
        lock(&self.inner.state).eviction_policy
    }

    fn default_budget(&self) -> i64 {
        (self.inner.total_memory_mb as f64 * self.inner.tight_fraction) as i64
    }

    fn is_heavy(&self, footprint_mb: Option<i64>) -> bool {
        footprint_mb.is_some_and(|footprint| footprint >= self.inner.heavy_threshold_mb)
    }
}

fn detect_total_memory_mb() -> i64 {
    #[cfg(target_os = "macos")]
    if let Some(bytes) = macos_memsize() {
        return (bytes / (1 << 20)) as i64;
    }
    #[cfg(target_os = "linux")]
    if let Some(mb) = linux_memtotal_mb() {
        return mb;
    }
    FALLBACK_TOTAL_MB
}

#[cfg(target_os = "macos")]
fn macos_memsize() -> Option<u64> {
    use std::os::raw::{c_char, c_int, c_void};
    unsafe extern "C" {
        fn sysctlbyname(
            name: *const c_char,
            oldp: *mut c_void,
            oldlenp: *mut usize,
            newp: *const c_void,
            newlen: usize,
        ) -> c_int;
    }
    let mut value: u64 = 0;
    let mut length = std::mem::size_of::<u64>();
    let name = c"hw.memsize";
    // SAFETY: `name` is a valid NUL-terminated C string; `value`/`length` are
    // valid, correctly-sized out-parameters; `sysctlbyname` writes at most
    // `length` bytes into `value`. A non-zero return means failure, handled below.
    let result = unsafe {
        sysctlbyname(
            name.as_ptr(),
            std::ptr::from_mut(&mut value).cast(),
            std::ptr::from_mut(&mut length),
            std::ptr::null(),
            0,
        )
    };
    (result == 0).then_some(value)
}

#[cfg(target_os = "linux")]
fn linux_memtotal_mb() -> Option<i64> {
    let contents = std::fs::read_to_string("/proc/meminfo").ok()?;
    let line = contents
        .lines()
        .find(|line| line.starts_with("MemTotal:"))?;
    let kb: i64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb / 1024)
}
