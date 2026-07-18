//! Value types for the governor: who wants the GPU, the RAM verdict, a resident
//! model record, and the residency/eviction policy.

use std::time::Duration;

/// Who is asking to use the GPU. Equality is by variant *and* model id, so
/// `Generation("a")` and `Generation("b")` are distinct producers.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum GpuProducer {
    /// A token-generation pass. The only producer that can share the gate — two
    /// concurrent generations of the *same* model may co-hold it.
    Generation(String),
    /// Loading a model's weights.
    Load(String),
    /// Unloading a model's weights.
    Unload(String),
    /// A one-shot media job.
    Job(String),
}

impl GpuProducer {
    /// Whether two identical copies of this producer may hold the gate at once.
    pub fn shares(&self) -> bool {
        matches!(self, GpuProducer::Generation(_))
    }

    /// The model this producer acts on.
    pub fn model_id(&self) -> &str {
        match self {
            GpuProducer::Generation(id)
            | GpuProducer::Load(id)
            | GpuProducer::Unload(id)
            | GpuProducer::Job(id) => id,
        }
    }
}

/// The advisory RAM pressure of admitting a model. Never blocks a load — callers
/// surface `Tight` as a status line and load anyway.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RamVerdict {
    /// The load fits comfortably under the budget ceiling.
    Ok,
    /// The load pushes resident memory over the ceiling.
    Tight,
}

/// A model the governor is tracking as resident (weights loaded, or reserved and
/// about to load).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResidentModel {
    /// The model's id.
    pub model_id: String,
    /// Its display name.
    pub name: String,
    /// Its estimated footprint in megabytes.
    pub footprint_mb: i64,
    /// A monotonic tick recording load order (lower is older), used to evict the
    /// oldest under the budgeted policy.
    pub loaded_at: u64,
}

/// How long a model's weights are kept warm after its last use before an idle
/// unload is scheduled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeepWarmPolicy {
    /// Keep warm for five minutes.
    FiveMinutes,
    /// Keep warm for fifteen minutes.
    FifteenMinutes,
    /// Keep warm for one hour.
    OneHour,
    /// Unload as soon as idle.
    Never,
}

impl KeepWarmPolicy {
    /// The warm window this policy corresponds to.
    pub fn warm_window(self) -> Duration {
        match self {
            KeepWarmPolicy::FiveMinutes => Duration::from_secs(300),
            KeepWarmPolicy::FifteenMinutes => Duration::from_secs(900),
            KeepWarmPolicy::OneHour => Duration::from_secs(3600),
            KeepWarmPolicy::Never => Duration::ZERO,
        }
    }
}

/// How the governor makes room when a model needs to load.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvictionPolicy {
    /// At most one heavy model resident at a time.
    StrictSingle,
    /// Evict the oldest resident(s) until the RAM budget is satisfied.
    Budgeted,
}

/// The residency configuration: how long to keep models warm, how to evict, and
/// an optional explicit RAM budget for the budgeted policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResidencyPolicy {
    /// The keep-warm window.
    pub keep_warm: KeepWarmPolicy,
    /// The eviction strategy.
    pub eviction: EvictionPolicy,
    /// An explicit RAM budget in megabytes for `Budgeted`; defaults to the
    /// governor's ceiling when `None`.
    pub ram_budget_mb: Option<i64>,
}

impl Default for ResidencyPolicy {
    fn default() -> Self {
        Self {
            keep_warm: KeepWarmPolicy::FiveMinutes,
            eviction: EvictionPolicy::StrictSingle,
            ram_budget_mb: None,
        }
    }
}
