//! The virtual `auto` model: resolving the two configured models for a caller,
//! asking the runtime what each makes of a request, remembering which tier each
//! conversation reached, and handing the choice to the kernel's routing policy.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use kernel::capabilities::ToolSpec;
use kernel::capabilities::chat::ChatMessage;
use kernel::records::{Capability, JsonValue, ModelRecord};
use kernel::routing::{
    ByTier, Candidate, RouteDecision, RouteInput, RouteRefusal, RouteRules, Tier, conversation_key,
    decide, tool_chars,
};
use runtime::settings::RoutingSettings;

use crate::error::{GatewayError, GatewayErrorKind};
use crate::param_guard;
use crate::port::GatewayPort;
use crate::resolver::{no_match, normalized_tag, resolve};
use crate::scopes::GatewayScopes;

/// The most conversations remembered at once; past it the least recently seen
/// is forgotten first.
const MAX_CONVERSATIONS: usize = 512;

/// A request as routing judges it, built from what its handler decoded.
pub struct RouteRequest {
    input: RouteInput,
    conversation: Option<u64>,
    uncounted: i64,
}

impl RouteRequest {
    /// A chat request: its messages and its tools.
    pub fn chat(messages: &[ChatMessage], tools: &[ToolSpec]) -> Self {
        Self {
            input: RouteInput::chat(messages, tools),
            conversation: conversation_key(messages),
            uncounted: tool_chars(tools),
        }
    }

    /// A bare-prompt completion. It has no conversation to remember.
    pub fn prompt(prompt: &str) -> Self {
        Self {
            input: RouteInput::prompt(prompt),
            conversation: None,
            uncounted: 0,
        }
    }

    /// The capability the request exercises.
    pub fn capability(&self) -> &Capability {
        &self.input.capability
    }

    /// Whether tools are attached.
    pub fn has_tools(&self) -> bool {
        self.input.tools
    }
}

/// The model routing chose for a request, and why.
#[derive(Debug, Clone)]
pub struct Routed {
    /// The chosen model.
    pub record: ModelRecord,
    /// The policy's decision.
    pub decision: RouteDecision,
}

/// Routing for one gateway: the virtual name, the two models it chooses
/// between, the rules, and each conversation's tier.
pub struct AutoRoute {
    name: String,
    models: ByTier<String>,
    rules: RouteRules,
    sticky_window: Duration,
    conversations: Mutex<HashMap<u64, Seen>>,
    clock: Box<dyn Fn() -> Instant + Send + Sync>,
}

#[derive(Debug, Clone, Copy)]
struct Seen {
    tier: Tier,
    at: Instant,
}

impl AutoRoute {
    /// Routing under `name` between the models named `small` and `large`.
    pub fn new(
        name: &str,
        small: &str,
        large: &str,
        rules: RouteRules,
        sticky_window: Duration,
    ) -> Self {
        Self {
            name: name.to_owned(),
            models: ByTier {
                small: small.to_owned(),
                large: large.to_owned(),
            },
            rules,
            sticky_window,
            conversations: Mutex::new(HashMap::new()),
            clock: Box::new(Instant::now),
        }
    }

    /// Routing as `[routing]` configures it, or `None` while it is off.
    pub fn from_settings(settings: &RoutingSettings) -> Option<Self> {
        settings.is_on().then(|| {
            Self::new(
                settings.model_name(),
                settings.small(),
                settings.large(),
                settings.rules(),
                settings.sticky_window(),
            )
        })
    }

    #[cfg(test)]
    fn with_clock(mut self, clock: impl Fn() -> Instant + Send + Sync + 'static) -> Self {
        self.clock = Box::new(clock);
        self
    }

    /// The virtual model name clients ask for.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Whether a request for `model` is one this routes, matched as a model
    /// name is: ignoring case and a trailing `:latest`.
    pub fn answers(&self, model: &str) -> bool {
        normalized_tag(model) == normalized_tag(&self.name)
    }

    /// The configured name of `tier`'s model.
    pub fn configured(&self, tier: Tier) -> &str {
        self.models.get(tier)
    }

    /// `tier`'s model for a caller holding `scopes`, resolved as a request
    /// naming it would be.
    pub fn resolve(
        &self,
        tier: Tier,
        shelf: &[ModelRecord],
        scopes: &GatewayScopes,
    ) -> Result<ModelRecord, GatewayError> {
        resolve(self.configured(tier), shelf, scopes)
    }

    /// Each tier's model for a caller holding `scopes`: ready and in scope, or
    /// `None`.
    pub fn resolved(
        &self,
        shelf: &[ModelRecord],
        scopes: &GatewayScopes,
    ) -> ByTier<Option<ModelRecord>> {
        ByTier {
            small: self.resolve(Tier::Small, shelf, scopes).ok(),
            large: self.resolve(Tier::Large, shelf, scopes).ok(),
        }
    }

    /// How the virtual model is listed to a caller holding `scopes`: the large
    /// model's record (the small one's when only it resolves) under the virtual
    /// name, claiming what either model can do and the wider of their windows,
    /// since a request one cannot serve goes to the other. `None` when neither
    /// resolves.
    pub fn listing(&self, shelf: &[ModelRecord], scopes: &GatewayScopes) -> Option<ModelRecord> {
        let resolved = self.resolved(shelf, scopes);
        let mut listed = resolved.large.clone().or_else(|| resolved.small.clone())?;
        for (_, other) in resolved.iter() {
            let Some(other) = other else { continue };
            for capability in &other.capabilities {
                if !listed.capabilities.contains(capability) {
                    listed.capabilities.push(capability.clone());
                }
            }
            listed.context_length = listed.context_length.max(other.context_length);
        }
        listed.alias = Some(self.name.clone());
        Some(listed)
    }

    /// Choose the model that serves `request` for a caller holding `scopes`.
    /// Each model is judged by what its runtime makes of `payload` and
    /// `params`: whether the prompt fits its window and whether it takes every
    /// parameter. A conversation never goes below the tier it reached; once
    /// the request is served, [`Self::remember`] records the tier.
    pub async fn route(
        &self,
        port: &dyn GatewayPort,
        scopes: &GatewayScopes,
        request: &RouteRequest,
        params: &BTreeMap<String, JsonValue>,
        payload: &JsonValue,
    ) -> Result<Routed, GatewayError> {
        let shelf = port.shelf().await;
        let resolved = self.resolved(&shelf, scopes);
        let floor = request.conversation.and_then(|key| self.floor(key));
        let judge = |uncounted| {
            let resolved = &resolved;
            async move {
                let tiers = ByTier {
                    small: candidate(
                        port,
                        resolved.small.as_ref(),
                        request,
                        params,
                        payload,
                        uncounted,
                    )
                    .await,
                    large: candidate(
                        port,
                        resolved.large.as_ref(),
                        request,
                        params,
                        payload,
                        uncounted,
                    )
                    .await,
                };
                decide(&request.input, &tiers, floor, &self.rules)
            }
        };
        let decision = match judge(request.uncounted).await {
            Ok(decision) => decision,
            // The tool definitions count toward fit to steer a request clear of
            // a window they would overflow, but a named model is served without
            // counting them, so they never make `auto` refuse what one would take.
            Err(refusal) if refusal.only_windows() && request.uncounted > 0 => {
                judge(0).await.map_err(|refusal| self.refused(&refusal))?
            }
            Err(refusal) => return Err(self.refused(&refusal)),
        };
        // `decide` only picks a tier it was handed a model for.
        let record = resolved.get(decision.tier).clone().ok_or_else(|| {
            GatewayError::new(
                GatewayErrorKind::ServerError,
                format!("{} chose a tier with no model", self.name),
            )
        })?;
        Ok(Routed { record, decision })
    }

    /// Record that `request`'s conversation was served at `tier`. The tier
    /// only ever rises: two of its requests finishing in either order leave
    /// the higher one.
    pub fn remember(&self, request: &RouteRequest, tier: Tier) {
        let Some(key) = request.conversation else {
            return;
        };
        let now = (self.clock)();
        let mut conversations = self.conversations();
        let tier = conversations
            .get(&key)
            .filter(|seen| self.is_fresh(seen, now))
            .map_or(tier, |seen| seen.tier.max(tier));
        conversations.insert(key, Seen { tier, at: now });
        if conversations.len() > MAX_CONVERSATIONS
            && let Some(oldest) = conversations
                .iter()
                .min_by_key(|(_, seen)| seen.at)
                .map(|(key, _)| *key)
        {
            conversations.remove(&oldest);
        }
    }

    /// The tier the conversation keyed `key` reached, while it is remembered.
    fn floor(&self, key: u64) -> Option<Tier> {
        let now = (self.clock)();
        let mut conversations = self.conversations();
        conversations.retain(|_, seen| self.is_fresh(seen, now));
        conversations.get(&key).map(|seen| seen.tier)
    }

    fn is_fresh(&self, seen: &Seen, now: Instant) -> bool {
        now.saturating_duration_since(seen.at) < self.sticky_window
    }

    fn refused(&self, refusal: &RouteRefusal) -> GatewayError {
        let both = refusal
            .lacks
            .iter()
            .map(|(tier, lack)| format!("{} ({tier}) {lack}", self.configured(tier)))
            .collect::<Vec<_>>()
            .join("; ");
        if refusal.nothing_ready() {
            let error = no_match(&self.name);
            GatewayError::new(error.kind, format!("{}: {both}", error.message))
        } else {
            GatewayError::new(
                GatewayErrorKind::BadRequest,
                format!("{} cannot serve this request: {both}", self.name),
            )
        }
    }

    fn conversations(&self) -> MutexGuard<'_, HashMap<u64, Seen>> {
        // A panic while the map was held leaves it whole: every write is a
        // single insert or remove.
        self.conversations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[cfg(test)]
    fn remembered(&self) -> usize {
        self.conversations().len()
    }
}

/// `record` as a candidate for `request`, with what its runtime says of the
/// prompt's fit and the request's parameters.
async fn candidate<'a>(
    port: &dyn GatewayPort,
    record: Option<&'a ModelRecord>,
    request: &RouteRequest,
    params: &BTreeMap<String, JsonValue>,
    payload: &JsonValue,
    uncounted: i64,
) -> Option<Candidate<'a>> {
    let record = record?;
    let capability = request.capability();
    // A model whose honored parameters cannot be read stays a candidate; the
    // guard run on the chosen model reports the failure.
    let refused_parameter = port
        .honored_params(&record.id, capability.clone())
        .await
        .ok()
        .and_then(|honored| param_guard::unhonored(params, &honored).map(str::to_owned));
    Some(Candidate {
        record,
        fit: port.prompt_fit(record, capability, payload, uncounted),
        refused_parameter,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    use kernel::capabilities::chat::{ChatMessage, ChatRole};
    use kernel::jobs::{Job, JobEvent};
    use kernel::records::{Modality, ModelSource, ModelState, SourceKind};
    use kernel::routing::RouteReason;
    use runtime::adapters::ChunkStream;
    use runtime::facade::KernelError;
    use tokio::sync::mpsc;

    use super::*;
    use crate::admission::{GatewayAdmissionState, GatewayWorkKind};
    use crate::port::{GatewayResident, PortFuture};

    /// A shelf of ready models, each taking only `temperature`.
    struct Shelf(Vec<ModelRecord>);

    impl GatewayPort for Shelf {
        fn shelf(&self) -> PortFuture<'_, Arc<[ModelRecord]>> {
            let shelf: Arc<[ModelRecord]> = self.0.clone().into();
            Box::pin(async move { shelf })
        }
        fn resident(&self) -> PortFuture<'_, Vec<GatewayResident>> {
            Box::pin(async { Vec::new() })
        }
        fn invoke<'a>(
            &'a self,
            _: &'a str,
            _: Capability,
            _: JsonValue,
        ) -> PortFuture<'a, Result<ChunkStream, KernelError>> {
            Box::pin(async { Ok(ChunkStream::channel().1) })
        }
        fn submit<'a>(
            &'a self,
            _: &'a str,
            _: Capability,
            _: JsonValue,
        ) -> PortFuture<'a, Result<String, KernelError>> {
            Box::pin(async { Ok(String::new()) })
        }
        fn job<'a>(&'a self, _: &'a str) -> PortFuture<'a, Option<Job>> {
            Box::pin(async { None })
        }
        fn job_events<'a>(
            &'a self,
            _: &'a str,
        ) -> PortFuture<'a, mpsc::UnboundedReceiver<JobEvent>> {
            Box::pin(async { mpsc::unbounded_channel().1 })
        }
        fn cancel<'a>(&'a self, _: &'a str) -> PortFuture<'a, ()> {
            Box::pin(async {})
        }
        fn abandon(&self, _: &str) {}
        fn voices<'a>(&'a self, _: &'a str) -> PortFuture<'a, Result<Vec<String>, KernelError>> {
            Box::pin(async { Ok(Vec::new()) })
        }
        fn honored_params<'a>(
            &'a self,
            _: &'a str,
            _: Capability,
        ) -> PortFuture<'a, Result<HashSet<String>, KernelError>> {
            Box::pin(async { Ok(HashSet::from(["temperature".to_owned()])) })
        }
        fn artifact_data<'a>(
            &'a self,
            _: &'a str,
        ) -> PortFuture<'a, Result<Option<Vec<u8>>, KernelError>> {
            Box::pin(async { Ok(None) })
        }
        fn admission_state<'a>(
            &'a self,
            _: &'a str,
            _: Option<i64>,
            _: GatewayWorkKind,
        ) -> PortFuture<'a, GatewayAdmissionState> {
            Box::pin(async { GatewayAdmissionState::Ready })
        }
    }

    fn ready(name: &str) -> ModelRecord {
        let mut record = ModelRecord::new(
            name,
            Modality::text(),
            vec![Capability::chat(), Capability::tools()],
            ModelSource::new(SourceKind::ollama(), name),
        );
        record.state = ModelState::Ready;
        record
    }

    fn shelf() -> Shelf {
        Shelf(vec![ready("tiny"), ready("big")])
    }

    fn routing() -> AutoRoute {
        AutoRoute::new(
            "auto",
            "tiny",
            "big",
            RouteRules::default(),
            Duration::from_secs(60),
        )
    }

    fn conversation(first: &str, latest: &str) -> RouteRequest {
        RouteRequest::chat(
            &[
                ChatMessage::new(ChatRole::User, first),
                ChatMessage::new(ChatRole::Assistant, "ok"),
                ChatMessage::new(ChatRole::User, latest),
            ],
            &[],
        )
    }

    fn opening(text: &str) -> RouteRequest {
        RouteRequest::chat(&[ChatMessage::new(ChatRole::User, text)], &[])
    }

    async fn route_with(
        routing: &AutoRoute,
        port: &Shelf,
        scopes: &GatewayScopes,
        request: &RouteRequest,
        params: &BTreeMap<String, JsonValue>,
    ) -> Result<Routed, GatewayError> {
        routing
            .route(port, scopes, request, params, &JsonValue::Null)
            .await
    }

    /// Route `request` and remember its tier, as a served request is.
    async fn route(routing: &AutoRoute, port: &Shelf, request: &RouteRequest) -> Routed {
        let routed = route_with(
            routing,
            port,
            &GatewayScopes::all(),
            request,
            &BTreeMap::new(),
        )
        .await
        .unwrap();
        routing.remember(request, routed.decision.tier);
        routed
    }

    #[tokio::test]
    async fn a_conversation_that_went_large_stays_large() {
        let (routing, port) = (routing(), shelf());
        let routed = route(&routing, &port, &opening("refactor the parser")).await;
        assert_eq!(routed.record.name, "big");

        let later = conversation("refactor the parser", "thanks");
        let routed = route(&routing, &port, &later).await;
        assert_eq!(routed.record.name, "big");
        assert_eq!(routed.decision.reason, RouteReason::Sticky);

        let routed = route(&routing, &port, &conversation("hello", "thanks")).await;
        assert_eq!(
            routed.record.name, "tiny",
            "another conversation starts afresh"
        );
    }

    #[tokio::test]
    async fn a_request_that_was_not_served_leaves_no_tier_behind() {
        let (routing, port) = (routing(), shelf());
        let opened = opening("debug this");
        route_with(
            &routing,
            &port,
            &GatewayScopes::all(),
            &opened,
            &BTreeMap::new(),
        )
        .await
        .unwrap();
        assert_eq!(routing.remembered(), 0);
        let routed = route(&routing, &port, &conversation("debug this", "ok")).await;
        assert_eq!(routed.record.name, "tiny");
    }

    #[test]
    fn a_tier_only_rises_whichever_request_finishes_last() {
        let routing = routing();
        let hard = opening("debug this");
        routing.remember(&hard, Tier::Large);
        routing.remember(&conversation("debug this", "ok"), Tier::Small);
        assert_eq!(
            routing.floor(
                conversation_key(&[ChatMessage::new(ChatRole::User, "debug this")]).unwrap()
            ),
            Some(Tier::Large)
        );
    }

    #[test]
    fn the_name_matches_as_a_model_name_does() {
        let routing = routing();
        for asked in ["auto", "Auto", "auto:latest", "AUTO:latest"] {
            assert!(routing.answers(asked), "{asked}");
        }
        assert!(!routing.answers("autobot"));
    }

    #[tokio::test]
    async fn tool_definitions_never_make_it_refuse_what_a_named_model_takes() {
        // Each model's window holds the prompt alone, not with the tools.
        let tight = |name| {
            let mut record = ready(name);
            record.context_length = Some(1000);
            record
        };
        let port = Shelf(vec![tight("tiny"), tight("big")]);
        let tools = [ToolSpec::new("t", "x".repeat(4000), JsonValue::Null)];
        let request = RouteRequest::chat(&[ChatMessage::new(ChatRole::User, "hi")], &tools);
        let routed = route_with(
            &routing(),
            &port,
            &GatewayScopes::all(),
            &request,
            &BTreeMap::new(),
        )
        .await
        .unwrap();
        assert_eq!(routed.record.name, "tiny");
    }

    #[tokio::test]
    async fn a_conversation_forgets_its_tier_after_the_window() {
        let elapsed = Arc::new(AtomicU64::new(0));
        let start = Instant::now();
        let clock = Arc::clone(&elapsed);
        let routing =
            routing().with_clock(move || start + Duration::from_secs(clock.load(Ordering::SeqCst)));
        let port = shelf();
        route(&routing, &port, &opening("debug this")).await;

        let later = conversation("debug this", "ok");
        elapsed.store(59, Ordering::SeqCst);
        assert_eq!(route(&routing, &port, &later).await.record.name, "big");

        elapsed.store(59 + 61, Ordering::SeqCst);
        assert_eq!(route(&routing, &port, &later).await.record.name, "tiny");
    }

    #[tokio::test]
    async fn the_memory_is_capped() {
        let ticks = Arc::new(AtomicU64::new(0));
        let start = Instant::now();
        let clock = Arc::clone(&ticks);
        let routing = routing().with_clock(move || {
            start + Duration::from_millis(clock.fetch_add(1, Ordering::SeqCst))
        });
        let port = shelf();
        for n in 0..(MAX_CONVERSATIONS + 10) {
            route(
                &routing,
                &port,
                &conversation(&format!("conversation {n}"), "hi"),
            )
            .await;
        }
        assert_eq!(routing.remembered(), MAX_CONVERSATIONS);
    }

    #[tokio::test]
    async fn a_model_out_of_the_callers_scope_is_not_routed_to() {
        let (routing, port) = (routing(), shelf());
        let scopes = GatewayScopes {
            models: Some(vec![port.0[1].id.clone()]),
            capabilities: None,
        };
        let routed = route_with(&routing, &port, &scopes, &opening("hi"), &BTreeMap::new())
            .await
            .unwrap();
        assert_eq!(routed.record.name, "big");
        assert_eq!(routed.decision.reason.to_string(), "small not ready");
    }

    #[tokio::test]
    async fn a_parameter_the_runtime_does_not_take_is_a_hard_requirement() {
        let (routing, port) = (routing(), Shelf(vec![ready("tiny")]));
        let params = BTreeMap::from([("presence_penalty".to_owned(), JsonValue::Int(1))]);
        let error = route_with(
            &routing,
            &port,
            &GatewayScopes::all(),
            &opening("hi"),
            &params,
        )
        .await
        .unwrap_err();
        assert_eq!(error.kind, GatewayErrorKind::BadRequest);
        assert!(
            error
                .message
                .contains("tiny (small) does not take presence_penalty"),
            "{}",
            error.message
        );
    }

    #[tokio::test]
    async fn nothing_to_route_to_is_not_found_naming_both_models() {
        let error = route_with(
            &routing(),
            &Shelf(Vec::new()),
            &GatewayScopes::all(),
            &opening("hi"),
            &BTreeMap::new(),
        )
        .await
        .unwrap_err();
        assert_eq!(error.kind, GatewayErrorKind::NotFound);
        assert!(error.message.contains("tiny (small) is not ready"));
        assert!(error.message.contains("big (large) is not ready"));
    }

    #[test]
    fn it_is_off_until_settings_name_two_models() {
        assert!(AutoRoute::from_settings(&RoutingSettings::default()).is_none());
        let settings = RoutingSettings {
            small: "tiny".to_owned(),
            large: "big".to_owned(),
            ..RoutingSettings::default()
        };
        let routing = AutoRoute::from_settings(&settings).expect("on");
        assert!(routing.answers("auto"));
        assert!(!routing.answers("tiny"));
    }
}
