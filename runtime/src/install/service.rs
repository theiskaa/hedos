//! The install orchestrator: turns a provider's raw `install()` progress stream
//! into the full [`InstallEvent`] lifecycle (queued → preparing → status/progress
//! → done/failed/cancelled), broadcasts events to any number of subscribers,
//! deduplicates concurrent installs of the same reference, and announces
//! completions so the shelf can refresh.
//!
//! The free-disk probe is injected (`with_disk_probe`); the default reports
//! "unknown" and the headroom check is skipped, because std has no portable
//! free-space API — the composition layer supplies a real probe.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use kernel::install::reference::{hugging_face_repo, is_hugging_face_link, ollama_direct_tag};
use kernel::install::{
    ActiveInstall, InstallAvailability, InstallBrowseResult, InstallError, InstallEvent,
    InstallPlan, InstallProgress, InstallProviderId, InstallSearchHit, InstallStreamEvent,
};
use kernel::records::SourceKind;
use tokio::sync::{Notify, mpsc};

use super::provider::{InstallProvider, InstallProviderStatus};

/// Multiply a pending download by this before checking free disk, for headroom.
const DISK_HEADROOM: f64 = 1.05;
/// How many concluded installs keep a replayable terminal event.
const TERMINAL_HISTORY_LIMIT: usize = 64;

/// A live feed of a single install's lifecycle events.
pub type InstallEventFeed = mpsc::UnboundedReceiver<InstallEvent>;
/// A feed of the source kinds whose installs just completed (shelf-refresh hint).
pub type CompletionFeed = mpsc::UnboundedReceiver<HashSet<SourceKind>>;

type DiskProbe = Box<dyn Fn(&Path) -> Option<i64> + Send + Sync>;
type Clock = Box<dyn Fn() -> i64 + Send + Sync>;

/// Coordinates installs across the registered providers.
#[derive(Clone)]
pub struct InstallService {
    inner: Arc<Inner>,
}

struct Inner {
    providers_by_id: HashMap<String, Arc<dyn InstallProvider>>,
    ordered: Vec<Arc<dyn InstallProvider>>,
    disk_probe_root: PathBuf,
    free_disk_bytes: DiskProbe,
    now_millis: Clock,
    id_counter: AtomicU64,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    installs: HashMap<String, ActiveInstall>,
    phases: HashMap<String, InstallEvent>,
    terminal: HashMap<String, InstallEvent>,
    terminal_order: VecDeque<String>,
    in_flight: HashMap<String, String>,
    subscribers: HashMap<String, HashMap<u64, mpsc::UnboundedSender<InstallEvent>>>,
    completion_subscribers: HashMap<u64, mpsc::UnboundedSender<HashSet<SourceKind>>>,
    cancels: HashMap<String, Arc<Notify>>,
    next_token: u64,
}

/// Assembles an [`InstallService`], letting the disk probe and clock be set
/// before the service is shared (avoids mutating an already-`Arc`d service).
pub struct InstallServiceBuilder {
    providers: Vec<Arc<dyn InstallProvider>>,
    disk_probe_root: PathBuf,
    free_disk_bytes: DiskProbe,
    now_millis: Clock,
}

impl InstallServiceBuilder {
    /// A builder over `providers` with the default (no-op) disk probe and clock.
    pub fn new(providers: Vec<Arc<dyn InstallProvider>>) -> Self {
        Self {
            providers,
            disk_probe_root: PathBuf::from("."),
            free_disk_bytes: Box::new(|_| None),
            now_millis: Box::new(default_now_millis),
        }
    }

    /// Set the free-disk probe and the root it measures (for the headroom check).
    pub fn disk_probe(
        mut self,
        root: impl Into<PathBuf>,
        probe: impl Fn(&Path) -> Option<i64> + Send + Sync + 'static,
    ) -> Self {
        self.disk_probe_root = root.into();
        self.free_disk_bytes = Box::new(probe);
        self
    }

    /// Override the clock used for `started_at` timestamps (tests).
    pub fn clock(mut self, clock: impl Fn() -> i64 + Send + Sync + 'static) -> Self {
        self.now_millis = Box::new(clock);
        self
    }

    /// Build the service (first registration wins on a duplicate provider id).
    pub fn build(self) -> InstallService {
        let mut providers_by_id = HashMap::new();
        for provider in &self.providers {
            providers_by_id
                .entry(provider.id().as_str().to_owned())
                .or_insert_with(|| Arc::clone(provider));
        }
        InstallService {
            inner: Arc::new(Inner {
                providers_by_id,
                ordered: self.providers,
                disk_probe_root: self.disk_probe_root,
                free_disk_bytes: self.free_disk_bytes,
                now_millis: self.now_millis,
                id_counter: AtomicU64::new(0),
                state: Mutex::new(State::default()),
            }),
        }
    }
}

impl InstallService {
    /// A service over `providers` with default disk/clock behavior. Use
    /// [`InstallServiceBuilder`] to set a real disk probe or a custom clock.
    pub fn new(providers: Vec<Arc<dyn InstallProvider>>) -> Self {
        InstallServiceBuilder::new(providers).build()
    }

    /// A builder for a service with a custom disk probe or clock.
    pub fn builder(providers: Vec<Arc<dyn InstallProvider>>) -> InstallServiceBuilder {
        InstallServiceBuilder::new(providers)
    }

    /// Each provider's identity and current availability, in registration order.
    pub async fn providers(&self) -> Vec<InstallProviderStatus> {
        let mut statuses = Vec::with_capacity(self.inner.ordered.len());
        for provider in &self.inner.ordered {
            statuses.push(provider.status().await);
        }
        statuses
    }

    /// Search one provider, erroring if it isn't available.
    pub async fn search(
        &self,
        provider: &InstallProviderId,
        query: &str,
        limit: usize,
    ) -> Result<Vec<InstallSearchHit>, InstallError> {
        self.require_available(provider)
            .await?
            .search(query, limit)
            .await
    }

    /// Browse for a model: a Hugging Face search, with the typed repo surfaced as
    /// an exact hit when it's an HF link or the search came back without it.
    pub async fn browse(&self, raw_query: &str, limit: usize) -> InstallBrowseResult {
        let query = raw_query.trim();
        if query.is_empty() || ollama_direct_tag(query).is_some() {
            return InstallBrowseResult::default();
        }
        let repo = hugging_face_repo(query);
        let search_term = repo.as_deref().unwrap_or(query);
        match self
            .search(&InstallProviderId::huggingface(), search_term, limit)
            .await
        {
            Ok(mut hits) => {
                if let Some(repo) = &repo {
                    let missing = !hits
                        .iter()
                        .any(|hit| hit.reference.eq_ignore_ascii_case(repo));
                    if (is_hugging_face_link(query) || hits.is_empty()) && missing {
                        hits.insert(0, exact_hit(repo));
                    }
                }
                InstallBrowseResult::with_hits(hits)
            }
            Err(error) => match &repo {
                Some(repo) => InstallBrowseResult::with_hits(vec![exact_hit(repo)]),
                None => InstallBrowseResult::failure(error.to_string()),
            },
        }
    }

    /// Resolve a plan through one provider, erroring if it isn't available.
    pub async fn plan(
        &self,
        provider: &InstallProviderId,
        reference: &str,
    ) -> Result<InstallPlan, InstallError> {
        self.require_available(provider)
            .await?
            .plan(reference)
            .await
    }

    /// Begin installing `plan`, returning the install id. A matching install
    /// already in flight returns the existing id; an over-budget disk check errors.
    pub fn begin(&self, plan: InstallPlan) -> Result<String, InstallError> {
        let provider = self
            .inner
            .providers_by_id
            .get(plan.provider.as_str())
            .cloned()
            .ok_or_else(|| InstallError::ProviderUnknown(plan.provider.clone()))?;
        let flight_key = flight_key(&plan);

        let cancel = Arc::new(Notify::new());
        let id = {
            let mut state = self.inner.lock();
            // Dedup before the disk check: an already-running install returns its
            // id without re-probing disk.
            if let Some(existing) = state.in_flight.get(&flight_key) {
                return Ok(existing.clone());
            }
            self.inner.check_disk(&plan)?;
            let id = self.inner.next_id();
            let started_at = (self.inner.now_millis)();
            state.installs.insert(
                id.clone(),
                ActiveInstall::new(
                    &id,
                    plan.provider.clone(),
                    &plan.reference,
                    &plan.display_name,
                    plan.total_bytes,
                    started_at,
                ),
            );
            state.phases.insert(id.clone(), InstallEvent::Queued);
            state.in_flight.insert(flight_key.clone(), id.clone());
            state.cancels.insert(id.clone(), Arc::clone(&cancel));
            Inner::emit(&mut state, &id, &InstallEvent::Queued);
            id
        };

        let inner = Arc::clone(&self.inner);
        tokio::spawn(run(inner, id.clone(), plan, flight_key, provider, cancel));
        Ok(id)
    }

    /// A live feed of one install's events, replaying its current state (or the
    /// terminal event for a concluded one).
    pub fn events(&self, id: &str) -> InstallEventFeed {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut state = self.inner.lock();
        if let Some(terminal) = state.terminal.get(id) {
            let _ = tx.send(terminal.clone());
            return rx;
        }
        let Some(install) = state.installs.get(id) else {
            return rx;
        };
        if let Some(phase) = state.phases.get(id) {
            let _ = tx.send(phase.clone());
        }
        if install.progress.bytes_downloaded > 0 || install.progress.total_bytes.is_some() {
            let _ = tx.send(InstallEvent::Progress(install.progress.clone()));
        }
        let token = state.next_token;
        state.next_token += 1;
        state
            .subscribers
            .entry(id.to_owned())
            .or_default()
            .insert(token, tx);
        rx
    }

    /// The in-progress installs, oldest first.
    pub fn active(&self) -> Vec<ActiveInstall> {
        let state = self.inner.lock();
        let mut installs: Vec<ActiveInstall> = state.installs.values().cloned().collect();
        installs.sort_by(|a, b| (a.started_at, &a.id).cmp(&(b.started_at, &b.id)));
        installs
    }

    /// Request cancellation of an install (a no-op for an unknown/concluded id).
    pub fn cancel(&self, id: &str) {
        let state = self.inner.lock();
        if let Some(cancel) = state.cancels.get(id) {
            cancel.notify_one();
        }
    }

    /// A feed of the source kinds whose installs complete, for shelf refreshes.
    pub fn completions(&self) -> CompletionFeed {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut state = self.inner.lock();
        let token = state.next_token;
        state.next_token += 1;
        state.completion_subscribers.insert(token, tx);
        rx
    }

    async fn require_available(
        &self,
        id: &InstallProviderId,
    ) -> Result<Arc<dyn InstallProvider>, InstallError> {
        let provider = self
            .inner
            .providers_by_id
            .get(id.as_str())
            .cloned()
            .ok_or_else(|| InstallError::ProviderUnknown(id.clone()))?;
        if let InstallAvailability::Unavailable { hint } = provider.availability().await {
            return Err(InstallError::ProviderUnavailable(hint));
        }
        Ok(provider)
    }
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn next_id(&self) -> String {
        format!("in-{:012}", self.id_counter.fetch_add(1, Ordering::Relaxed))
    }

    fn check_disk(&self, plan: &InstallPlan) -> Result<(), InstallError> {
        let Some(pending) = plan.remaining_bytes.or(plan.total_bytes) else {
            return Ok(());
        };
        let scaled = (pending.max(0) as f64) * DISK_HEADROOM;
        let required = if scaled >= i64::MAX as f64 {
            i64::MAX
        } else {
            scaled as i64
        };
        let available = (self.free_disk_bytes)(&self.disk_probe_root).unwrap_or(i64::MAX);
        if available < required {
            return Err(InstallError::InsufficientDisk {
                required_bytes: required,
                available_bytes: available,
            });
        }
        Ok(())
    }

    fn transition(&self, id: &str, event: InstallEvent) {
        let mut state = self.lock();
        state.phases.insert(id.to_owned(), event.clone());
        Self::emit(&mut state, id, &event);
    }

    fn apply_progress(&self, id: &str, progress: InstallProgress) {
        let mut state = self.lock();
        let Some(install) = state.installs.get_mut(id) else {
            return;
        };
        install.progress = progress.clone();
        Self::emit(&mut state, id, &InstallEvent::Progress(progress));
    }

    fn conclude(&self, id: &str, flight_key: &str, source_kind: &SourceKind, event: InstallEvent) {
        let mut state = self.lock();
        if !state.installs.contains_key(id) {
            return;
        }
        Self::emit(&mut state, id, &event);
        state.terminal.insert(id.to_owned(), event);
        state.terminal_order.push_back(id.to_owned());
        if state.terminal_order.len() > TERMINAL_HISTORY_LIMIT
            && let Some(oldest) = state.terminal_order.pop_front()
        {
            state.terminal.remove(&oldest);
        }
        // Dropping the subscriber senders ends each feed with a clean close.
        state.subscribers.remove(id);
        state.installs.remove(id);
        state.phases.remove(id);
        state.cancels.remove(id);
        if state.in_flight.get(flight_key) == Some(&id.to_owned()) {
            state.in_flight.remove(flight_key);
        }
        let kinds: HashSet<SourceKind> = std::iter::once(source_kind.clone()).collect();
        state
            .completion_subscribers
            .retain(|_, tx| tx.send(kinds.clone()).is_ok());
    }

    fn emit(state: &mut State, id: &str, event: &InstallEvent) {
        if let Some(subscribers) = state.subscribers.get_mut(id) {
            // Send to each live feed and prune any whose receiver has been dropped,
            // so a long install with churny subscribers can't accumulate dead
            // senders.
            subscribers.retain(|_, tx| {
                let _ = tx.send(event.clone());
                !tx.is_closed()
            });
        }
    }
}

/// Consume the provider's install stream, mapping it to the lifecycle and
/// concluding. Cancellation drops the stream (triggering the provider's cleanup).
async fn run(
    inner: Arc<Inner>,
    id: String,
    plan: InstallPlan,
    flight_key: String,
    provider: Arc<dyn InstallProvider>,
    cancel: Arc<Notify>,
) {
    let source_kind = provider.source_kind();
    inner.transition(&id, InstallEvent::Preparing);
    let mut stream = provider.install(plan);

    // One `Notified` future, pinned for the whole loop, so cancellation is a
    // clean one-shot rather than a fresh future per iteration.
    let cancelled = cancel.notified();
    tokio::pin!(cancelled);
    let terminal = loop {
        tokio::select! {
            biased;
            _ = &mut cancelled => break InstallEvent::Cancelled,
            item = stream.recv() => match item {
                None => break InstallEvent::Done,
                Some(Ok(InstallStreamEvent::Status(message))) => {
                    inner.transition(&id, InstallEvent::Status(message));
                }
                Some(Ok(InstallStreamEvent::Progress(progress))) => {
                    inner.apply_progress(&id, progress);
                }
                Some(Err(error)) => break InstallEvent::Failed {
                    message: error.to_string(),
                },
            },
        }
    };

    // Drop the stream before concluding so a cancel reaches the provider's
    // cleanup (its sender sees the receiver gone) before we mark the install done.
    drop(stream);
    inner.conclude(&id, &flight_key, &source_kind, terminal);
}

fn flight_key(plan: &InstallPlan) -> String {
    format!("{}|{}", plan.provider.as_str(), plan.reference)
}

fn exact_hit(repo: &str) -> InstallSearchHit {
    InstallSearchHit {
        provider: InstallProviderId::huggingface(),
        reference: repo.to_owned(),
        name: repo
            .rsplit('/')
            .find(|part| !part.is_empty())
            .unwrap_or(repo)
            .to_owned(),
        downloads: None,
        likes: None,
        updated_at: None,
    }
}

fn default_now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}
