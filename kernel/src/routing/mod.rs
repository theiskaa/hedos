//! Auto routing: choosing, per request, between a small and a large model the
//! person configured.
//!
//! The decision runs in three steps. What the request needs (a capability,
//! tools, vision, a wide enough window, parameters its runtime takes) rules a
//! tier out. What the latest user turn looks like picks a tier among the rest.
//! And a conversation that already went large stays large, because switching
//! models mid-conversation throws away the engine's prompt cache and, with one
//! model resident at a time, reloads a model. This module is the pure policy;
//! the gateway holds the memory of each conversation's tier.

mod key;
mod signals;

use std::fmt;

use crate::capabilities::ToolSpec;
use crate::capabilities::chat::{AttachmentKind, ChatMessage};
use crate::profiles::context_budget::Verdict;
use crate::records::{Capability, ModelRecord};

pub use key::conversation_key;
pub use signals::{Signal, Turn};

use signals::classify;

/// The words that mark a turn as demanding when nothing else does. Matched as
/// whole words, case-insensitively.
pub const DEFAULT_HARD_WORDS: &[&str] = &[
    "implement",
    "refactor",
    "architecture",
    "design",
    "migrate",
    "optimize",
    "debug",
    "investigate",
    "prove",
    "step by step",
];

/// The longest latest-user-turn, in characters, still judged simple.
pub const DEFAULT_MAX_SIMPLE_CHARS: usize = 1200;

/// One of the two models a router chooses between. Ordered: `Small < Large`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Tier {
    /// The small, fast model.
    Small,
    /// The large, capable model.
    Large,
}

impl Tier {
    /// The tier's lowercase name.
    pub fn as_str(self) -> &'static str {
        match self {
            Tier::Small => "small",
            Tier::Large => "large",
        }
    }

    fn other(self) -> Self {
        match self {
            Tier::Small => Tier::Large,
            Tier::Large => Tier::Small,
        }
    }
}

impl fmt::Display for Tier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The tunable part of the policy: what counts as a demanding turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteRules {
    /// A latest user turn longer than this many characters goes large.
    pub max_simple_chars: usize,
    /// Words or phrases that send a turn large.
    pub hard_words: Vec<String>,
}

impl Default for RouteRules {
    fn default() -> Self {
        Self {
            max_simple_chars: DEFAULT_MAX_SIMPLE_CHARS,
            hard_words: DEFAULT_HARD_WORDS
                .iter()
                .map(|word| (*word).to_owned())
                .collect(),
        }
    }
}

/// A value for each tier: the two models, their names, what each can do.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ByTier<T> {
    /// The small tier's.
    pub small: T,
    /// The large tier's.
    pub large: T,
}

impl<T> ByTier<T> {
    /// `tier`'s value.
    pub fn get(&self, tier: Tier) -> &T {
        match tier {
            Tier::Small => &self.small,
            Tier::Large => &self.large,
        }
    }

    /// Both values, small first, each with its tier.
    pub fn iter(&self) -> impl Iterator<Item = (Tier, &T)> {
        [(Tier::Small, &self.small), (Tier::Large, &self.large)].into_iter()
    }
}

/// What a request asks of the model that serves it.
#[derive(Debug, Clone, PartialEq)]
pub struct RouteInput {
    /// The capability the request exercises (`chat` or `complete`).
    pub capability: Capability,
    /// Whether tools are attached.
    pub tools: bool,
    /// Whether the last message carries an image. Only the last one is
    /// checked, as the runtime checks it: an image earlier in the history does
    /// not stop a model that cannot see from answering the next turn.
    pub images: bool,
    /// The latest user turn.
    pub turn: Turn,
}

impl RouteInput {
    /// The input for a chat request.
    pub fn chat(messages: &[ChatMessage], tools: &[ToolSpec]) -> Self {
        let images = messages.last().is_some_and(|message| {
            message
                .attachments
                .iter()
                .any(|attachment| attachment.kind == AttachmentKind::Image)
        });
        Self {
            capability: Capability::chat(),
            tools: !tools.is_empty(),
            images,
            turn: Turn::latest(messages),
        }
    }

    /// The input for a bare-prompt completion request.
    pub fn prompt(prompt: &str) -> Self {
        Self {
            capability: Capability::complete(),
            tools: false,
            images: false,
            turn: Turn::prompt(prompt),
        }
    }
}

/// The characters the tools' definitions add to the prompt. The runtime does
/// not count them when it judges a prompt's fit, but the engine reads every
/// one: an agent harness's tool list runs to thousands of tokens.
pub fn tool_chars(tools: &[ToolSpec]) -> i64 {
    tools
        .iter()
        .map(|tool| {
            let parameters = serde_json::to_string(&tool.parameters).map_or(0, |text| text.len());
            (tool.name.len() + tool.description.len() + parameters) as i64
        })
        .sum()
}

/// One tier's model, and what its runtime says of the request.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate<'a> {
    /// The model.
    pub record: &'a ModelRecord,
    /// Whether the prompt fits the window the runtime serves it at.
    pub fit: Verdict,
    /// A request parameter its runtime does not take, if any.
    pub refused_parameter: Option<String>,
}

/// The two tiers' models. A tier is `None` when its model is not ready or the
/// caller cannot reach it.
pub type Tiers<'a> = ByTier<Option<Candidate<'a>>>;

/// How a capability reads in a reason: `tools`, `vision`, or its own name.
pub fn feature(capability: &Capability) -> &str {
    if *capability == Capability::see() {
        "vision"
    } else {
        capability.as_str()
    }
}

/// Why a tier cannot serve a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lack {
    /// Its model is not ready, or the caller cannot reach it.
    Unavailable,
    /// Its model lacks a capability the request needs.
    Capability(Capability),
    /// The prompt does not fit its window.
    Window {
        /// The prompt's estimated tokens.
        estimated: i64,
        /// The window.
        window: i64,
    },
    /// Its runtime does not take one of the request's parameters.
    Parameter(String),
}

impl fmt::Display for Lack {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Lack::Unavailable => f.write_str("is not ready"),
            Lack::Capability(capability) => write!(f, "lacks {}", feature(capability)),
            Lack::Window { estimated, window } => {
                write!(
                    f,
                    "holds {window} tokens and the prompt is about {estimated}"
                )
            }
            Lack::Parameter(key) => write!(f, "does not take {key}"),
        }
    }
}

/// Why a request went to the tier it did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteReason {
    /// The latest user turn decided it.
    Signal(Signal),
    /// The conversation already went large.
    Sticky,
    /// The tier the turn asked for cannot serve the request, so the other one
    /// does.
    Cannot {
        /// The tier that could not.
        tier: Tier,
        /// Why.
        lack: Lack,
    },
}

impl fmt::Display for RouteReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RouteReason::Signal(signal) => signal.fmt(f),
            RouteReason::Sticky => f.write_str("conversation already large"),
            RouteReason::Cannot {
                tier: Tier::Small,
                lack,
            } => match lack {
                Lack::Unavailable => f.write_str("small not ready"),
                Lack::Capability(capability) => write!(f, "needs {}", feature(capability)),
                Lack::Window { .. } => f.write_str("needs a wider window"),
                Lack::Parameter(key) => write!(f, "needs {key}"),
            },
            RouteReason::Cannot {
                tier: Tier::Large,
                lack,
            } => write!(f, "large {lack}"),
        }
    }
}

/// The tier a request goes to, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteDecision {
    /// The chosen tier.
    pub tier: Tier,
    /// Why.
    pub reason: RouteReason,
}

/// Neither tier can serve the request.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("neither routed model can serve this request: small {}; large {}", .lacks.small, .lacks.large)]
pub struct RouteRefusal {
    /// Why each tier cannot.
    pub lacks: ByTier<Lack>,
}

impl RouteRefusal {
    /// Whether both tiers are simply not ready, as opposed to present and unfit.
    pub fn nothing_ready(&self) -> bool {
        self.lacks
            .iter()
            .all(|(_, lack)| *lack == Lack::Unavailable)
    }

    /// Whether every tier that is ready fails only on the prompt's fit.
    pub fn only_windows(&self) -> bool {
        !self.nothing_ready()
            && self
                .lacks
                .iter()
                .all(|(_, lack)| matches!(lack, Lack::Window { .. } | Lack::Unavailable))
    }
}

/// Choose the tier that serves `input`.
///
/// `floor` is the tier the conversation already reached; the choice never goes
/// below it unless the tier at the floor cannot serve the request at all.
pub fn decide(
    input: &RouteInput,
    tiers: &Tiers<'_>,
    floor: Option<Tier>,
    rules: &RouteRules,
) -> Result<RouteDecision, RouteRefusal> {
    let signal = classify(&input.turn, rules);
    let (wanted, reason) = match floor {
        Some(floor) if floor > signal.tier() => (floor, RouteReason::Sticky),
        _ => (signal.tier(), RouteReason::Signal(signal)),
    };

    let wanted_lack = match fitness(input, tiers.get(wanted).as_ref()) {
        Ok(()) => {
            return Ok(RouteDecision {
                tier: wanted,
                reason,
            });
        }
        Err(lack) => lack,
    };
    let other = wanted.other();
    match fitness(input, tiers.get(other).as_ref()) {
        Ok(()) => Ok(RouteDecision {
            tier: other,
            reason: RouteReason::Cannot {
                tier: wanted,
                lack: wanted_lack,
            },
        }),
        Err(other_lack) => {
            let (small, large) = match wanted {
                Tier::Small => (wanted_lack, other_lack),
                Tier::Large => (other_lack, wanted_lack),
            };
            Err(RouteRefusal {
                lacks: ByTier { small, large },
            })
        }
    }
}

/// Whether `candidate` can serve `input`: the hard requirements, which no turn
/// can talk past.
fn fitness(input: &RouteInput, candidate: Option<&Candidate<'_>>) -> Result<(), Lack> {
    let candidate = candidate.ok_or(Lack::Unavailable)?;
    let record = candidate.record;
    let mut needed = vec![input.capability.clone()];
    if input.tools {
        needed.push(Capability::tools());
    }
    if input.images {
        needed.push(Capability::see());
    }
    if let Some(missing) = needed
        .into_iter()
        .find(|capability| !record.can(capability))
    {
        return Err(Lack::Capability(missing));
    }
    if let Verdict::Exceeds { estimated, window } = candidate.fit {
        return Err(Lack::Window { estimated, window });
    }
    if let Some(key) = &candidate.refused_parameter {
        return Err(Lack::Parameter(key.clone()));
    }
    Ok(())
}
