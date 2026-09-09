//! Driving the bench: clear a model from memory, run it, measure, repeat, and
//! report each step as it happens.
//!
//! The measuring itself is [`kernel::bench`]; this only takes the marks and
//! walks the models. Clearing a model is the one thing it cannot do on its own
//! (a model can be held by this process, by a running gateway, or by the Ollama
//! daemon, and only the caller knows how to ask each), so that is a trait the
//! caller implements.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use kernel::bench::{ColdStart, Figures, Phase, Sample, Status, WallClock, estimate_tokens};
use kernel::capabilities::{CapabilityChunk, GenerationStats};
use kernel::records::{Capability, JsonValue};
use tokio::sync::mpsc::UnboundedSender;

use crate::facade::Kernel;

/// The prompt every benched model answers: prose-shaped, so a model has to
/// write rather than answer in a word, and identical everywhere so two rows
/// mean the same thing.
pub const DEFAULT_PROMPT: &str =
    "Explain how a hash map works, in plain prose, in about two hundred words.";
/// Tokens a run is capped at. Enough to measure a rate, short enough that a
/// model that writes long is cut rather than waited on.
pub const DEFAULT_MAX_TOKENS: i64 = 128;
/// Warm runs per model, after the cold one.
pub const DEFAULT_RUNS: usize = 3;
/// How often a run reports the tokens it has produced. Fast enough to read as
/// moving, slow enough that a fast model does not flood the screen.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(120);
/// How long the driver waits on a silent stream before looking at the cancel
/// flag again. A backend that accepts a request and then produces nothing would
/// otherwise park the walk forever, with no way to stop it.
const CANCEL_POLL: Duration = Duration::from_millis(250);

/// What to bench, and how.
#[derive(Debug, Clone, PartialEq)]
pub struct BenchPlan {
    /// The model ids to bench, in the order they are run.
    pub models: Vec<String>,
    /// The prompt each one answers.
    pub prompt: String,
    /// The cap on each reply.
    pub max_tokens: i64,
    /// Warm runs per model.
    pub runs: usize,
    /// Leave residency alone: no eviction, and so no cold figure.
    pub keep_warm: bool,
}

impl BenchPlan {
    /// A plan over `models` with the standard prompt, cap, and run count.
    pub fn new(models: Vec<String>) -> Self {
        Self {
            models,
            prompt: DEFAULT_PROMPT.to_owned(),
            max_tokens: DEFAULT_MAX_TOKENS,
            runs: DEFAULT_RUNS,
            keep_warm: false,
        }
    }
}

/// What clearing a model from memory came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Eviction {
    /// It is out of memory; what follows is a cold run.
    Cleared,
    /// Something else holds it and would not let go, named here (a running
    /// gateway holds its models in another process).
    Held(String),
}

/// A future returning what an eviction came to.
pub type EvictFuture = Pin<Box<dyn Future<Output = Eviction> + Send>>;

/// Clearing a model from wherever it is loaded, so its next run is cold. The
/// production implementation asks this process's governor, a running gateway,
/// and the Ollama daemon in turn; the seam keeps that out of the driver and
/// lets the tests drive a fake.
pub trait Prepare: Send + Sync {
    /// Clear `model_id` from memory.
    fn evict(&self, model_id: &str) -> EvictFuture;
}

/// A flag the driver checks between runs and inside one, so a bench can be
/// stopped without waiting for the model it is on to finish.
#[derive(Debug, Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    /// A flag that is not yet set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Ask the bench to stop.
    pub fn stop(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    /// Whether stopping was asked for.
    pub fn stopped(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// A step of the bench, as it happens.
#[derive(Debug, Clone, PartialEq)]
pub enum BenchEvent {
    /// A model's run began.
    Started { id: String, phase: Phase },
    /// The run has produced this many tokens so far.
    Tokens { id: String, tokens: i64 },
    /// A model is finished with, one way or another.
    Settled { id: String, status: Box<Status> },
}

/// What one run came to.
enum RunOutcome {
    Measured(Box<Sample>),
    Failed(String),
    Stopped,
}

/// Bench every model of `plan` in turn, reporting each step through `events`.
///
/// A model is cleared from memory first (unless the plan keeps it warm), so
/// every row starts from the same state and the first token of that first run
/// is the cold figure; the warm runs follow, and the model is cleared again so
/// the next one has the machine to itself. A failure is that model's row and
/// nothing more: the walk carries on to the next.
pub async fn run(
    kernel: &Kernel,
    plan: &BenchPlan,
    prepare: &dyn Prepare,
    cancel: &Cancel,
    events: &UnboundedSender<BenchEvent>,
) {
    for id in &plan.models {
        if cancel.stopped() {
            settle(events, id, Status::Stopped);
            continue;
        }
        let status = bench_one(kernel, plan, prepare, cancel, events, id).await;
        settle(events, id, status);
    }
}

/// Bench one model, and put it back down however that went: a model left
/// loaded by a run that failed part way would be in memory for every model
/// after it, and none of those figures would mean anything.
async fn bench_one(
    kernel: &Kernel,
    plan: &BenchPlan,
    prepare: &dyn Prepare,
    cancel: &Cancel,
    events: &UnboundedSender<BenchEvent>,
    id: &str,
) -> Status {
    let status = measure(kernel, plan, prepare, cancel, events, id).await;
    if !plan.keep_warm {
        let _ = prepare.evict(id).await;
    }
    status
}

/// The runs themselves: the cold one, then the warm ones.
async fn measure(
    kernel: &Kernel,
    plan: &BenchPlan,
    prepare: &dyn Prepare,
    cancel: &Cancel,
    events: &UnboundedSender<BenchEvent>,
    id: &str,
) -> Status {
    let mut cold = ColdStart::NotMeasured;
    if !plan.keep_warm {
        // Announced before the eviction rather than after it: clearing the
        // model is the first thing a cold start waits on, and a row that reads
        // `waiting` all through it looks like one the bench has not reached.
        let _ = events.send(BenchEvent::Started {
            id: id.to_owned(),
            phase: Phase::ColdStart,
        });
        match prepare.evict(id).await {
            Eviction::Cleared => match one_run(kernel, plan, cancel, events, id).await {
                RunOutcome::Measured(sample) => cold = ColdStart::Measured(sample.ttft_ms),
                RunOutcome::Failed(reason) => return Status::Failed(reason),
                RunOutcome::Stopped => return Status::Stopped,
            },
            // Nothing here was cold, so there is nothing to time; the warm
            // figures below are still real.
            Eviction::Held(holder) => cold = ColdStart::Held(holder),
        }
    }

    let mut samples = Vec::with_capacity(plan.runs);
    for run in 1..=plan.runs {
        let _ = events.send(BenchEvent::Started {
            id: id.to_owned(),
            phase: Phase::Warm { run, of: plan.runs },
        });
        match one_run(kernel, plan, cancel, events, id).await {
            RunOutcome::Measured(sample) => samples.push(*sample),
            // What was measured before it broke still stands, the same way a
            // stop keeps what it had; only a model that measured nothing at
            // all reads as failed.
            RunOutcome::Failed(reason) if samples.is_empty() => return Status::Failed(reason),
            RunOutcome::Stopped if samples.is_empty() => return Status::Stopped,
            RunOutcome::Failed(_) | RunOutcome::Stopped => break,
        }
    }
    if samples.is_empty() {
        return Status::Failed("the plan asked for no runs".to_owned());
    }
    Status::Done(Box::new(Figures::summarize(cold, samples)))
}

/// One run: open the stream, mark the first token and the end, and fold what
/// the runtime reported into a sample.
async fn one_run(
    kernel: &Kernel,
    plan: &BenchPlan,
    cancel: &Cancel,
    events: &UnboundedSender<BenchEvent>,
    id: &str,
) -> RunOutcome {
    // Checked before the request is opened, not only while the reply is read:
    // a request an adapter has started loads the model whether or not anyone
    // reads what comes back, and a stop that landed during the eviction would
    // otherwise leave the model in memory as the bench walked away.
    if cancel.stopped() {
        return RunOutcome::Stopped;
    }
    let started = Instant::now();
    let mut stream = match kernel.invoke(id, Capability::chat(), payload(plan)).await {
        Ok(stream) => stream,
        Err(error) => return RunOutcome::Failed(error.to_string()),
    };

    let mut text = String::new();
    let mut ttft_ms = None;
    let mut stats: Option<GenerationStats> = None;
    let mut reported_at = Instant::now();

    loop {
        if cancel.stopped() {
            return RunOutcome::Stopped;
        }
        // A silent stream is waited on in slices, so a stop is honoured even
        // when the backend never sends another byte.
        let item = match tokio::time::timeout(CANCEL_POLL, stream.recv()).await {
            Ok(Some(item)) => item,
            Ok(None) => break,
            Err(_) => continue,
        };
        match item {
            // Thinking is generated text: a reasoning model's hidden tokens
            // cost the same time as its visible ones and belong in the rate.
            Ok(CapabilityChunk::Text(chunk) | CapabilityChunk::Thinking(chunk)) => {
                if ttft_ms.is_none() {
                    ttft_ms = Some(started.elapsed().as_millis() as i64);
                }
                text.push_str(&chunk);
                if reported_at.elapsed() >= PROGRESS_INTERVAL {
                    reported_at = Instant::now();
                    let _ = events.send(BenchEvent::Tokens {
                        id: id.to_owned(),
                        tokens: estimate_tokens(&text),
                    });
                }
            }
            Ok(CapabilityChunk::Done(done)) => stats = done,
            Ok(_) => {}
            Err(error) => return RunOutcome::Failed(error.to_string()),
        }
    }

    // The count the reply ended on, which the interval above may not have
    // reached.
    let _ = events.send(BenchEvent::Tokens {
        id: id.to_owned(),
        tokens: estimate_tokens(&text),
    });
    let Some(ttft_ms) = ttft_ms else {
        return RunOutcome::Failed("the model produced no tokens".to_owned());
    };
    let wall = WallClock {
        ttft_ms,
        total_ms: started.elapsed().as_millis() as i64,
    };
    RunOutcome::Measured(Box::new(Sample::new(stats.as_ref(), &text, wall)))
}

/// The one-turn chat payload every run sends.
fn payload(plan: &BenchPlan) -> JsonValue {
    let message = JsonValue::object([
        ("role", JsonValue::String("user".to_owned())),
        ("content", JsonValue::String(plan.prompt.clone())),
    ]);
    JsonValue::object([
        ("messages", JsonValue::Array(vec![message])),
        ("max_tokens", JsonValue::Int(plan.max_tokens)),
        // A bench is a measurement, so the same prompt should take the same
        // path through the model every time.
        ("temperature", JsonValue::Double(0.0)),
    ])
}

fn settle(events: &UnboundedSender<BenchEvent>, id: &str, status: Status) {
    let _ = events.send(BenchEvent::Settled {
        id: id.to_owned(),
        status: Box::new(status),
    });
}
