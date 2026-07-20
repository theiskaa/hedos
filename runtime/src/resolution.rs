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
//! actor.
//!
//! Backfilling modality/capabilities from the winner's runtime manifest when
//! identification comes up empty is deferred until the manifest-backed adapters
//! are ported (none bid today).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use kernel::profiles::ProfileRegistry;
use kernel::records::{
    Capability, ModelRecord, ModelState, Resolution, RunTier, RuntimeId, RuntimeRef, SourceKind,
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
        let wires_tools = self.adapter_wires_tools(updated.runtime.id.as_ref());
        merge(&plan.identified, &mut updated, wires_tools);
        if plan.identified.params.is_empty() {
            updated = self.profiles.refreshed(&updated);
        }
        (updated != *current).then_some(updated)
    }

    /// Whether the adapter behind `id` forwards tools. `false` when no adapter in
    /// this engine matches — the winner was just assigned from this same set, so
    /// a miss means the record resolved to nothing.
    fn adapter_wires_tools(&self, id: Option<&RuntimeId>) -> bool {
        id.and_then(|id| self.adapters.iter().find(|adapter| adapter.id() == id))
            .is_some_and(|adapter| adapter.wires_tools())
    }

    /// Re-apply the `tools` capability fold to every record using its
    /// already-resolved runtime, without re-running the auction. A registry
    /// written before the capability existed never had the fold applied, and
    /// nothing re-resolves an existing shelf on its own — without this, such a
    /// shelf serves no tools until a manual rescan. Idempotent and cheap (no
    /// identification, no disk reads): an already-folded record comes out
    /// unchanged and nothing is rewritten. Applied to pinned records too — the
    /// fold adjusts capabilities, never the runtime choice a pin protects.
    pub fn refold_tool_capability(
        &self,
        registry: &mut Registry,
    ) -> Result<Vec<ModelRecord>, RegistryError> {
        let ids: Vec<String> = registry
            .list()
            .into_iter()
            .map(|record| record.id.clone())
            .collect();
        registry.update(&ids, |current| {
            let mut updated = current.clone();
            let wires_tools = self.adapter_wires_tools(updated.runtime.id.as_ref());
            fold_tool_capability(
                wires_tools,
                updated.supports_tools,
                &mut updated.capabilities,
            );
            (updated != *current).then_some(updated)
        })
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
fn merge(identified: &IdentifiedModel, updated: &mut ModelRecord, runtime_wires_tools: bool) {
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
    // Re-apply the tool signal after capabilities were overwritten. Tool support
    // is the resolved runtime AND the model together: the runtime has to forward
    // tools and parse tool calls ([`RuntimeAdapter::wires_tools`]), and the
    // model's template has to declare them. This is what keeps a model served by
    // a non-wiring runtime (the Python sidecars) out of the `hedos launch`
    // picker rather than letting it be picked and then refused.
    fold_tool_capability(
        runtime_wires_tools,
        updated.supports_tools,
        &mut updated.capabilities,
    );
}

/// Fold the `tools` capability into a conversational model when both its runtime
/// wires tools and its template doesn't authoritatively lack them (`Some(false)`).
/// An undetermined template (`None`) on a tool-wiring runtime is assumed capable
/// and gated by an actual request rather than hidden.
fn fold_tool_capability(
    runtime_wires_tools: bool,
    tool_capable: Option<bool>,
    capabilities: &mut Vec<Capability>,
) {
    let conversational = capabilities
        .iter()
        .any(|cap| *cap == Capability::chat() || *cap == Capability::complete());
    let has_tools = capabilities.iter().any(|cap| *cap == Capability::tools());
    let keep = conversational && runtime_wires_tools && tool_capable != Some(false);
    match (keep, has_tools) {
        (true, false) => capabilities.push(Capability::tools()),
        (false, true) => capabilities.retain(|cap| *cap != Capability::tools()),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::fold_tool_capability;
    use kernel::records::Capability;

    /// Fold assuming a tool-wiring runtime, isolating the template logic.
    fn fold(caps: Vec<Capability>, tool_capable: Option<bool>) -> Vec<Capability> {
        let mut caps = caps;
        fold_tool_capability(true, tool_capable, &mut caps);
        caps
    }

    #[test]
    fn an_undetermined_chat_model_is_assumed_tool_capable() {
        // A template we couldn't read (safetensors, endpoints) keeps tools, so a
        // model that may well support them isn't hidden from the launch picker.
        let caps = fold(vec![Capability::chat(), Capability::complete()], None);
        assert!(caps.contains(&Capability::tools()));
    }

    #[test]
    fn a_template_with_tool_markers_gets_tools() {
        let caps = fold(vec![Capability::chat()], Some(true));
        assert!(caps.contains(&Capability::tools()));
    }

    #[test]
    fn a_template_without_tool_markers_withholds_tools() {
        // The authoritative negative: deepseek-coder-v2's template lacks them.
        let caps = fold(
            vec![Capability::chat(), Capability::complete()],
            Some(false),
        );
        assert!(!caps.contains(&Capability::tools()));
    }

    #[test]
    fn a_negative_removes_a_previously_added_tools_capability() {
        let caps = fold(vec![Capability::chat(), Capability::tools()], Some(false));
        assert!(!caps.contains(&Capability::tools()));
    }

    #[test]
    fn a_non_conversational_model_is_never_given_tools() {
        let caps = fold(vec![Capability::embed()], Some(true));
        assert!(!caps.contains(&Capability::tools()));
    }

    #[test]
    fn tools_is_not_duplicated() {
        let caps = fold(vec![Capability::chat(), Capability::tools()], None);
        let count = caps.iter().filter(|c| **c == Capability::tools()).count();
        assert_eq!(count, 1);
    }

    #[test]
    fn a_runtime_that_does_not_wire_tools_withholds_them() {
        // The mlx-lm sidecar renders only the message list into the prompt, so a
        // model it serves can't do tool calls however capable the weights are.
        let mut caps = vec![Capability::chat(), Capability::tools()];
        fold_tool_capability(false, None, &mut caps);
        assert!(!caps.contains(&Capability::tools()));

        let mut caps = vec![Capability::chat()];
        fold_tool_capability(false, Some(true), &mut caps);
        assert!(!caps.contains(&Capability::tools()));
    }
}
