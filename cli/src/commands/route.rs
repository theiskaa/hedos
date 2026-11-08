//! `hedos route`: the gateway's `auto` model, which serves each request with
//! the small or the large model named here.
//!
//! The policy is the kernel's and the gateway applies it; this command sets the
//! two models, says whether they can stay loaded together, and runs the
//! gateway's own routing on a sample turn without serving it.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use clap::{Args, Subcommand};
use gateway::GatewayScopes;
use gateway::auto_route::{AutoRoute, RouteRequest};
use gateway::kernel_gateway::KernelGateway;
use gateway::resolver;
use kernel::capabilities::ToolSpec;
use kernel::capabilities::chat::{AttachmentKind, ChatAttachment, ChatMessage, ChatRole};
use kernel::records::byte_format::format_bytes;
use kernel::records::{Capability, JsonValue, ModelRecord, ModelState};
use kernel::routing::{Tier, feature};
use runtime::governor::EvictionPolicy;
use runtime::settings::{ROUTING_OFF, SettingsStore};

use crate::error::CliError;
use crate::support::output::Out;
use crate::support::payload;
use crate::support::session::{self, Session};

/// Arguments for `route`.
#[derive(Args)]
pub struct RouteArgs {
    #[command(subcommand)]
    command: Option<RouteCommand>,
}

#[derive(Subcommand)]
enum RouteCommand {
    /// Show the two models and whether they can stay loaded together.
    Show,
    /// Route between a small and a large model.
    Set(SetArgs),
    /// Turn routing off.
    Off,
    /// Show which model a request would go to, and why, without running it.
    Explain(ExplainArgs),
}

#[derive(Args)]
struct SetArgs {
    /// The small, fast model (name, alias, or id).
    #[arg(long)]
    small: String,
    /// The large, capable model (name, alias, or id).
    #[arg(long)]
    large: String,
    /// The model name clients ask for. Omit to keep the current one (auto
    /// unless set).
    #[arg(long)]
    name: Option<String>,
}

#[derive(Args)]
struct ExplainArgs {
    /// The request's latest user turn.
    text: Option<String>,
    /// Read the turn from a file instead.
    #[arg(long, conflicts_with = "text")]
    file: Option<PathBuf>,
    /// The request carries tools, as every agent harness's does.
    #[arg(long)]
    tools: bool,
    /// The request carries an image.
    #[arg(long)]
    image: bool,
    /// Characters of earlier conversation sent with it.
    #[arg(long, default_value_t = 0)]
    history: usize,
}

/// Run the `route` command.
pub async fn run(args: RouteArgs, out: &Out) -> Result<(), CliError> {
    match args.command.unwrap_or(RouteCommand::Show) {
        RouteCommand::Show => show(out).await,
        RouteCommand::Set(args) => set(args, out).await,
        RouteCommand::Off => off(out),
        RouteCommand::Explain(args) => explain(args, out).await,
    }
}

/// The routing `session` serves, or the error saying it is off.
pub(crate) fn auto_route(session: &Session) -> Result<AutoRoute, CliError> {
    AutoRoute::from_settings(&session.settings.routing).ok_or_else(|| CliError::new(ROUTING_OFF))
}

/// Why a small model that cannot call tools takes no agent requests.
pub(crate) fn toolless_note(record: &ModelRecord) -> String {
    format!(
        "{} cannot call tools, so every request from an agent harness goes large",
        record.wire_id()
    )
}

/// One tier as the gateway resolves it, and what the runtime serves it with.
struct Shown {
    tier: Tier,
    configured: String,
    resolved: Result<ModelRecord, String>,
    window: Option<i64>,
}

impl Shown {
    fn of(session: &Session, auto: &AutoRoute, shelf: &[ModelRecord], tier: Tier) -> Self {
        let resolved = auto
            .resolve(tier, shelf, &GatewayScopes::all())
            .map_err(|error| error.message);
        let window = resolved
            .as_ref()
            .ok()
            .and_then(|record| session.kernel.context_window(record, &Capability::chat()));
        Self {
            tier,
            configured: auto.configured(tier).to_owned(),
            resolved,
            window,
        }
    }

    fn record(&self) -> Option<&ModelRecord> {
        self.resolved.as_ref().ok()
    }

    fn features(record: &ModelRecord) -> Vec<String> {
        [Capability::tools(), Capability::see()]
            .iter()
            .filter(|capability| record.can(capability))
            .map(|capability| feature(capability).to_owned())
            .collect()
    }

    fn line(&self) -> String {
        let record = match &self.resolved {
            Ok(record) => record,
            Err(why) => return format!("{:<5}  {}  {why}", self.tier, self.configured),
        };
        let mut facts = Vec::new();
        if let Some(bytes) = record.serving_size() {
            facts.push(format_bytes(bytes));
        }
        if let Some(window) = self.window {
            facts.push(format!("{window} tokens"));
        }
        facts.extend(Self::features(record));
        format!(
            "{:<5}  {}  {}",
            self.tier,
            record.wire_id(),
            facts.join(" · ")
        )
    }

    fn json(&self) -> serde_json::Value {
        let record = self.record();
        serde_json::json!({
            "configured": self.configured,
            "model": record.map(ModelRecord::wire_id),
            "id": record.map(|record| record.id.clone()),
            "ready": record.is_some(),
            "problem": self.resolved.as_ref().err(),
            "window": self.window,
            "bytes": record.and_then(ModelRecord::serving_size),
            "features": record.map(Self::features),
        })
    }
}

async fn show(out: &Out) -> Result<(), CliError> {
    let session = Session::open()?;
    let Ok(auto) = auto_route(&session) else {
        out.line(ROUTING_OFF);
        out.json(&serde_json::json!({ "on": false }));
        return Ok(());
    };
    let shelf = session.shelf_or_discover().await?;
    let small = Shown::of(&session, &auto, &shelf, Tier::Small);
    let large = Shown::of(&session, &auto, &shelf, Tier::Large);

    out.line(&format!("{} routes between", auto.name()));
    out.line(&format!("  {}", small.line()));
    out.line(&format!("  {}", large.line()));

    let chat = Capability::chat();
    let governed = |record: &ModelRecord| session.kernel.governed(record, &chat);
    let models = small.record().zip(large.record());
    // The eviction policy only reaches models the governor loads; llama.cpp
    // keeps a server per model, and a daemon such as Ollama keeps its own.
    let together = models
        .filter(|(small, large)| governed(small) && governed(large))
        .map(|(small, large)| {
            session
                .kernel
                .governor()
                .hold_together(&[small.footprint_mib(), large.footprint_mib()])
        });
    let eviction = session.settings.models.eviction;
    match together {
        Some(true) => {
            out.line("both stay loaded together, so switching between them costs no reload")
        }
        Some(false) if eviction == EvictionPolicy::StrictSingle => out.line(
            "only one stays loaded at a time (eviction = \"strict_single\"), so each switch \
             between them reloads a model. Set eviction = \"budgeted\" under [models] to keep \
             both when memory allows",
        ),
        Some(false) => out.line(
            "together they exceed the memory budget, so each switch between them reloads a model",
        ),
        None => {}
    }
    for record in [small.record(), large.record()].into_iter().flatten() {
        if !governed(record) {
            out.line(&format!(
                "{} runs on {}, which decides itself what it keeps loaded; \
                 the eviction setting does not apply to it",
                record.wire_id(),
                record
                    .runtime
                    .id
                    .as_ref()
                    .map_or("its runtime", |id| id.as_str())
            ));
        }
    }
    if models.is_none() {
        out.err("while one model is unavailable, the other takes every request it can");
    }
    if let Some(record) = small.record()
        && !record.can(&Capability::tools())
    {
        out.err(&toolless_note(record));
    }
    let shadowed = shelf.iter().find(|record| {
        record.state == ModelState::Ready
            && (auto.answers(&record.name) || auto.answers(record.display_name()))
    });
    if let Some(record) = shadowed {
        out.err(&format!(
            "the model {} is named {} too; while routing is on, reach it by its id {}",
            record.name,
            auto.name(),
            record.id
        ));
    }

    let rules = session.settings.routing.rules();
    out.json(&serde_json::json!({
        "on": true,
        "name": auto.name(),
        "small": small.json(),
        "large": large.json(),
        "hold_together": together,
        "governed": {
            "small": small.record().map(governed),
            "large": large.record().map(governed),
        },
        "eviction": eviction,
        "max_simple_chars": rules.max_simple_chars,
        "hard_words": rules.hard_words,
        "sticky_minutes": session.settings.routing.sticky_minutes,
    }));
    Ok(())
}

async fn set(args: SetArgs, out: &Out) -> Result<(), CliError> {
    let session = Session::open()?;
    let shelf = session.shelf_or_discover().await?;
    let small = ready_chat(&shelf, &args.small)?;
    let large = ready_chat(&shelf, &args.large)?;
    if small.id == large.id {
        return Err(CliError::new(format!(
            "{} is both models; routing needs two different ones",
            small.wire_id()
        )));
    }
    let name = args
        .name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty());

    let saved = SettingsStore::discover()
        .update(|settings| {
            settings.routing.small = stored_name(small, &shelf);
            settings.routing.large = stored_name(large, &shelf);
            if let Some(name) = name {
                settings.routing.name = name.to_owned();
            }
        })
        .map_err(|error| CliError::new(error.to_string()))?;
    let routing = &saved.routing;

    out.line(&format!(
        "{} now routes between {} (small) and {} (large)",
        routing.model_name(),
        small.wire_id(),
        large.wire_id()
    ));
    if !small.can(&Capability::tools()) {
        out.err(&toolless_note(small));
    }
    if let Some(live) = session.live_gateway().await {
        out.err(&format!(
            "a gateway is running on port {}; restart it to route with these models",
            live.port
        ));
    }
    out.json(&serde_json::json!({
        "name": routing.model_name(),
        "small": routing.small(),
        "large": routing.large(),
    }));
    Ok(())
}

/// The ready chat model `query` names.
fn ready_chat<'a>(shelf: &'a [ModelRecord], query: &str) -> Result<&'a ModelRecord, CliError> {
    let record = session::resolve(query, shelf, Some(&Capability::chat()))?;
    if record.state != ModelState::Ready {
        return Err(CliError::new(format!(
            "{} is not ready ({}), so it cannot serve requests",
            record.wire_id(),
            record.state.as_str()
        )));
    }
    Ok(record)
}

/// How `record` is written under `[routing]`: the name the gateway lists it
/// by, unless that name also reaches another model, in which case its id.
fn stored_name(record: &ModelRecord, shelf: &[ModelRecord]) -> String {
    match resolver::resolve(record.wire_id(), shelf, &GatewayScopes::all()) {
        Ok(found) if found.id == record.id => record.wire_id().to_owned(),
        _ => record.id.clone(),
    }
}

fn off(out: &Out) -> Result<(), CliError> {
    SettingsStore::discover()
        .update(|settings| {
            settings.routing.small.clear();
            settings.routing.large.clear();
        })
        .map_err(|error| CliError::new(error.to_string()))?;
    out.line("auto routing is off");
    out.json(&serde_json::json!({ "on": false }));
    Ok(())
}

async fn explain(args: ExplainArgs, out: &Out) -> Result<(), CliError> {
    let session = Session::open()?;
    let auto = auto_route(&session)?;
    let text = match (args.text, &args.file) {
        (_, Some(path)) => std::fs::read_to_string(path).map_err(|error| {
            CliError::new(format!("could not read {}: {error}", path.display()))
        })?,
        (Some(text), None) => text,
        (None, None) => {
            return Err(CliError::new(
                "give the turn to explain, as text or with --file",
            ));
        }
    };

    let mut messages = Vec::new();
    if args.history > 0 {
        messages.push(ChatMessage::new(
            ChatRole::Assistant,
            "x".repeat(args.history),
        ));
    }
    let mut turn = ChatMessage::new(ChatRole::User, text);
    if args.image {
        turn.attachments.push(ChatAttachment {
            kind: AttachmentKind::Image,
            data: Vec::new(),
            mime_type: "image/png".to_owned(),
            name: None,
        });
    }
    messages.push(turn);
    let tools = if args.tools {
        vec![ToolSpec::new("tool", "", JsonValue::Null)]
    } else {
        Vec::new()
    };
    let request_payload = JsonValue::Object(payload::chat(
        messages.iter().map(ChatMessage::payload_value).collect(),
        None,
    ));

    session.shelf_or_discover().await?;
    let port = KernelGateway::new(Arc::clone(&session.kernel));
    let routed = auto
        .route(
            &port,
            &GatewayScopes::all(),
            &RouteRequest::chat(&messages, &tools),
            &BTreeMap::new(),
            &request_payload,
        )
        .await
        .map_err(|error| CliError::new(error.message))?;
    let model = routed.record.wire_id();
    let reason = routed.decision.reason.to_string();
    out.line(&format!(
        "{} → {model} ({}): {reason}",
        auto.name(),
        routed.decision.tier
    ));
    out.json(&serde_json::json!({
        "tier": routed.decision.tier.as_str(),
        "model": model,
        "reason": reason,
    }));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel::records::{Modality, ModelSource, SourceKind};

    fn ready(name: &str, path: &str) -> ModelRecord {
        let mut record = ModelRecord::new(
            name,
            Modality::text(),
            vec![Capability::chat()],
            ModelSource::new(SourceKind::ollama(), path),
        );
        record.state = ModelState::Ready;
        record
    }

    #[test]
    fn a_model_is_stored_by_name_unless_the_name_reaches_another() {
        let alone = ready("qwen3", "a");
        assert_eq!(stored_name(&alone, std::slice::from_ref(&alone)), "qwen3");

        let shelf = vec![ready("dup", "a"), ready("dup", "b")];
        assert_eq!(stored_name(&shelf[1], &shelf), shelf[1].id);
    }
}
