//! The runtime-bid auction: given the adapters wired into this process, decide
//! which one serves each model. [`ResolutionEngine`] identifies a record (via
//! [`kernel::resolution::identify`]), collects a bid from every adapter, ranks
//! them, and writes the winning runtime — plus the runner-up alternatives and the
//! identified modality/capabilities/params — back onto the record.
//!
//! This is the piece that closes the discovery→serve loop: a model discovered on
//! disk becomes a model with a resolved runtime the gateway can dispatch to.
//!
//! The engine is synchronous — [`RuntimeAdapter::bid`] and `identify` are pure —
//! so it operates directly on a borrowed [`Registry`] rather than through an async
//! actor as the Swift original did.
//!
//! The Swift original also backfilled modality/capabilities from the winner's
//! runtime manifest when identification came up empty; that is deferred until the
//! manifest-backed adapters are ported (none bid today).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use kernel::profiles::ProfileRegistry;
use kernel::records::{
    ModelRecord, ModelState, Resolution, RunTier, RuntimeId, RuntimeRef, SourceKind,
};
use kernel::registry::{Registry, RegistryError};
use kernel::resolution::{IdentificationCache, IdentifiedModel, identify};

use crate::adapters::RuntimeAdapter;

/// One adapter's bid, tagged with which adapter offered it.
struct BidEntry {
    id: RuntimeId,
    tier: RunTier,
    preference: i64,
    alternatives: Vec<RuntimeId>,
}

/// The identification plus ranked bids computed for a record before it is applied.
struct ResolutionPlan {
    identified: IdentifiedModel,
    bids: Vec<BidEntry>,
}

/// One line of a [`ResolutionExplanation`]: an adapter and the bid it offered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterBidReport {
    /// The bidding adapter.
    pub adapter_id: RuntimeId,
    /// The tier it offered to run the model at.
    pub tier: RunTier,
    /// Its tie-break preference (lower wins).
    pub preference: i64,
}

/// A dry-run of resolution for one record: what it was identified as and how each
/// adapter bid, ranked winner-first. Produced by [`ResolutionEngine::explain`]
/// without mutating anything.
#[derive(Debug, Clone)]
pub struct ResolutionExplanation {
    /// The record as it was explained.
    pub record: ModelRecord,
    /// What identification determined.
    pub identified: IdentifiedModel,
    /// Every adapter's bid, ranked winner-first.
    pub bids: Vec<AdapterBidReport>,
}

impl ResolutionExplanation {
    /// The winning adapter, if any adapter bid.
    pub fn winner(&self) -> Option<&RuntimeId> {
        self.bids.first().map(|report| &report.adapter_id)
    }
}

/// Ranks adapter bids and resolves records to their runtimes.
pub struct ResolutionEngine {
    adapters: Vec<Arc<dyn RuntimeAdapter>>,
    profiles: ProfileRegistry,
    cache: Option<Arc<IdentificationCache>>,
}

impl ResolutionEngine {
    /// An engine over `adapters` using the built-in profile set for the parameter
    /// backfill (applied only when identification yields no schema of its own).
    pub fn new(adapters: Vec<Arc<dyn RuntimeAdapter>>) -> Self {
        Self::with_profiles(adapters, ProfileRegistry::builtin())
    }

    /// An engine with an explicit profile registry.
    pub fn with_profiles(
        adapters: Vec<Arc<dyn RuntimeAdapter>>,
        profiles: ProfileRegistry,
    ) -> Self {
        Self {
            adapters,
            profiles,
            cache: None,
        }
    }

    /// The engine with a shared identification cache, so repeated resolution
    /// passes over an unchanged shelf skip re-reading model headers.
    pub fn with_cache(mut self, cache: Arc<IdentificationCache>) -> Self {
        self.cache = Some(cache);
        self
    }

    /// Identify `record`, through the cache when one is wired.
    fn identify(&self, record: &ModelRecord) -> IdentifiedModel {
        match &self.cache {
            Some(cache) => cache.identify(record),
            None => identify(record),
        }
    }

    /// Resolve every record in `registry` (optionally filtered to `kinds`),
    /// writing the winners back. Returns the records that changed.
    pub fn resolve_all(
        &self,
        registry: &mut Registry,
        kinds: Option<&HashSet<SourceKind>>,
    ) -> Result<Vec<ModelRecord>, RegistryError> {
        let plans: HashMap<String, ResolutionPlan> = registry
            .list()
            .into_iter()
            .filter(|record| kinds.is_none_or(|kinds| kinds.contains(&record.source.kind)))
            .filter_map(|record| self.plan(record).map(|plan| (record.id.clone(), plan)))
            .collect();
        if plans.is_empty() {
            return Ok(Vec::new());
        }
        let ids: Vec<String> = plans.keys().cloned().collect();
        let live = self.live_adapter_ids();
        registry.update(&ids, |current| {
            let plan = plans.get(&current.id)?;
            self.applied(plan, current, &live)
        })
    }

    /// Resolve a single `record` against `registry`, returning the updated record
    /// if resolution changed it.
    pub fn resolve(
        &self,
        record: &ModelRecord,
        registry: &mut Registry,
    ) -> Result<Option<ModelRecord>, RegistryError> {
        let Some(plan) = self.plan(record) else {
            return Ok(None);
        };
        let live = self.live_adapter_ids();
        let changed = registry.update(std::slice::from_ref(&record.id), |current| {
            self.applied(&plan, current, &live)
        })?;
        Ok(changed.into_iter().next())
    }

    /// Explain how `record` would resolve without changing anything.
    pub fn explain(&self, record: &ModelRecord) -> ResolutionExplanation {
        let identified = self.identify(record);
        let bids = self
            .collect_bids(record, &identified)
            .into_iter()
            .map(|entry| AdapterBidReport {
                adapter_id: entry.id,
                tier: entry.tier,
                preference: entry.preference,
            })
            .collect();
        ResolutionExplanation {
            record: record.clone(),
            identified,
            bids,
        }
    }

    /// Explain every record in `registry`.
    pub fn explain_all(&self, registry: &Registry) -> Vec<ResolutionExplanation> {
        registry
            .list()
            .into_iter()
            .map(|record| self.explain(record))
            .collect()
    }

    fn live_adapter_ids(&self) -> HashSet<RuntimeId> {
        self.adapters
            .iter()
            .map(|adapter| adapter.id().clone())
            .collect()
    }

    /// Identify `record` and collect its ranked bids — unless it is user-pinned to
    /// a live adapter (leave that choice alone) or its weights are missing.
    fn plan(&self, record: &ModelRecord) -> Option<ResolutionPlan> {
        if self.is_pinned_to_live_adapter(record) {
            return None;
        }
        if record.state == ModelState::Missing {
            return None;
        }
        let identified = self.identify(record);
        let bids = self.collect_bids(record, &identified);
        Some(ResolutionPlan { identified, bids })
    }

    fn is_pinned_to_live_adapter(&self, record: &ModelRecord) -> bool {
        record.runtime.resolved == Resolution::User
            && record
                .runtime
                .id
                .as_ref()
                .is_some_and(|pinned| self.adapters.iter().any(|adapter| adapter.id() == pinned))
    }

    fn collect_bids(&self, record: &ModelRecord, identified: &IdentifiedModel) -> Vec<BidEntry> {
        let mut entries: Vec<BidEntry> = self
            .adapters
            .iter()
            .filter_map(|adapter| {
                adapter.bid(record, identified).map(|bid| BidEntry {
                    id: adapter.id().clone(),
                    tier: bid.tier,
                    preference: bid.preference,
                    alternatives: bid.alternatives,
                })
            })
            .collect();
        // Lower preference wins; ties break on the runtime id for a stable order.
        entries
            .sort_by(|left, right| (left.preference, &left.id).cmp(&(right.preference, &right.id)));
        entries
    }

    /// Apply `plan` to `current`, returning the updated record if it differs. Re-runs
    /// the pin/missing guards against the live registry state (a record can change
    /// between planning and the write). A still-downloading record is reset to
    /// unresolved rather than resolved against partial weights.
    fn applied(
        &self,
        plan: &ResolutionPlan,
        current: &ModelRecord,
        live: &HashSet<RuntimeId>,
    ) -> Option<ModelRecord> {
        if current.runtime.resolved == Resolution::User
            && current
                .runtime
                .id
                .as_ref()
                .is_some_and(|pinned| live.contains(pinned))
        {
            return None;
        }
        if current.state == ModelState::Missing {
            return None;
        }
        if current.downloading {
            let mut updated = current.clone();
            updated.runtime = RuntimeRef::default();
            updated.state = ModelState::Unresolved;
            return (updated != *current).then_some(updated);
        }

        let mut updated = current.clone();
        apply_winner(&plan.bids, &mut updated, &current.runtime);
        merge(&plan.identified, &mut updated);
        if plan.identified.params.is_empty() {
            updated = self.profiles.refreshed(&updated);
        }
        (updated != *current).then_some(updated)
    }
}

/// Set `updated`'s runtime to the winning bid, recording the runner-ups (plus the
/// winner's own declared alternatives) as fallbacks. With no bids the record is
/// reset to unresolved.
fn apply_winner(bids: &[BidEntry], updated: &mut ModelRecord, previous: &RuntimeRef) {
    let Some(winner) = bids.first() else {
        updated.runtime = RuntimeRef::default();
        updated.state = ModelState::Unresolved;
        return;
    };
    let mut alternatives: Vec<RuntimeId> = bids[1..].iter().map(|entry| entry.id.clone()).collect();
    for declared in &winner.alternatives {
        if *declared != winner.id && !alternatives.contains(declared) {
            alternatives.push(declared.clone());
        }
    }
    updated.runtime = RuntimeRef {
        id: Some(winner.id.clone()),
        resolved: Resolution::Auto,
        tier: winner.tier,
        alternatives,
        // Keep the confirmation timestamp only if the winner is unchanged.
        confirmed_at: if previous.id.as_ref() == Some(&winner.id) {
            previous.confirmed_at
        } else {
            None
        },
    };
    updated.state = ModelState::Ready;
}

/// Fold the identified facts onto `updated`, each field only when identification
/// actually determined it (an empty modality/capability/param set never clobbers
/// what the record already carries).
fn merge(identified: &IdentifiedModel, updated: &mut ModelRecord) {
    if let Some(modality) = &identified.modality {
        updated.modality = modality.clone();
    }
    if !identified.capabilities.is_empty() {
        updated.capabilities = identified.capabilities.clone();
    }
    if !identified.params.is_empty() {
        updated.params = identified.params.clone();
    }
    if let Some(context_length) = identified.context_length {
        updated.context_length = Some(context_length);
    }
    if let Some(has_chat_template) = identified.has_chat_template {
        updated.has_chat_template = Some(has_chat_template);
    }
    updated.execution = identified.execution;
}
