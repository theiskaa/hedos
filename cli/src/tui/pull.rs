//! The pull screen's state: what was typed, which kind of model is shown,
//! what matched, and the plan of the row the cursor rests on, fetched while
//! it rests so the preview is there before `enter`. Pure, like the rest of
//! the app state; the searches and plans it asks for run as tasks.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};

use kernel::install::catalog::InstallCategory;
use kernel::install::installed::{installed_names, is_installed};
use kernel::install::plan::{InstallPlan, InstallSearchHit};
use kernel::install::provider::InstallProviderId;
use kernel::install::recommend::{Ask, Recommendation, Status, recommend};
use kernel::install::reference::{hugging_face_repo, ollama_direct_tag};
use kernel::machine::{Engine, Fit, Machine};
use kernel::profiles::FitVerdict;
use kernel::records::{ModelRecord, ModelState};

use super::edit::LineEdit;
use super::event::Key;
use super::text;
use crate::support::recommend::notes_label;

/// How many quiet ticks after a keystroke before the typed query is searched.
pub(crate) const SEARCH_DEBOUNCE_TICKS: u64 = 2;
/// How many ticks the cursor rests on a row before its plan is asked for:
/// long enough that scrolling past a row asks nothing.
pub(crate) const PLAN_SETTLE_TICKS: u64 = 2;
/// The most matches the list keeps, besides the catalog's models already on
/// the shelf; search hits always keep their places. The catalog's picks can
/// fill it: up to three per category.
pub(crate) const MAX_MATCHES: usize = 12;
/// Hugging Face hits requested per search.
pub(crate) const SEARCH_LIMIT: usize = 8;

/// Which models the list shows: all of them, or one of the catalog's kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Every match.
    All,
    /// The matches of one catalog category.
    Of(InstallCategory),
}

/// The kinds, in the order `tab` steps through them.
pub(crate) const KINDS: [Kind; 5] = [
    Kind::All,
    Kind::Of(InstallCategory::Chat),
    Kind::Of(InstallCategory::Code),
    Kind::Of(InstallCategory::Voice),
    Kind::Of(InstallCategory::Image),
];

impl Kind {
    /// How the kind reads on its chip.
    pub fn label(self) -> &'static str {
        match self {
            Kind::All => "all",
            Kind::Of(InstallCategory::Chat) => "chat",
            Kind::Of(InstallCategory::Code) => "code",
            Kind::Of(InstallCategory::Voice) => "speech",
            Kind::Of(InstallCategory::Image) => "image",
        }
    }

    fn admits(self, offer: &Offer) -> bool {
        match self {
            Kind::All => true,
            Kind::Of(category) => offer.category == Some(category),
        }
    }
}

/// Whether an offer is already on the shelf, and how.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnShelf {
    /// There, with its weights.
    Present,
    /// Its record is there but its weights are gone; pulling again
    /// restores it.
    Gone,
}

/// One installable model the list offers.
#[derive(Debug, Clone, PartialEq)]
pub struct Offer {
    pub provider: InstallProviderId,
    pub reference: String,
    /// What serving it loads, in bytes, when the catalog knows it; search
    /// hits don't.
    pub bytes: Option<i64>,
    /// The engine that would serve it: the catalog's, Ollama for a typed
    /// tag, and unknown for anything from Hugging Face it has not planned.
    pub engine: Engine,
    /// A one-line note: the catalog blurb, or the hit's popularity.
    pub note: String,
    /// The catalog kind; search hits and a typed reference have none.
    pub category: Option<InstallCategory>,
    /// Whether a pull of it is already running.
    pub pulling: bool,
    /// Whether it is on the shelf already.
    pub shelf: Option<OnShelf>,
    /// How many times a search hit was downloaded, when the hub says.
    pub downloads: Option<i64>,
}

impl Offer {
    fn new(provider: InstallProviderId, reference: String, note: String) -> Self {
        Self {
            provider,
            reference,
            bytes: None,
            engine: Engine::Other,
            note,
            category: None,
            pulling: false,
            shelf: None,
            downloads: None,
        }
    }

    fn from_catalog(rec: &Recommendation) -> Self {
        let entry = &rec.entry;
        let notes = notes_label(&rec.notes);
        let note = if notes.is_empty() {
            entry.blurb.clone()
        } else {
            format!("{} · {notes}", entry.blurb)
        };
        Self {
            bytes: Some(entry.serving_size()),
            engine: entry.engine,
            category: Some(entry.category),
            ..Self::new(entry.provider.clone(), entry.reference.clone(), note)
        }
    }

    fn from_hit(hit: &InstallSearchHit) -> Self {
        let mut note = Vec::new();
        if let Some(downloads) = hit.downloads {
            note.push(format!("↓{}", text::compact(downloads)));
        }
        if let Some(likes) = hit.likes {
            note.push(format!("♥{}", text::compact(likes)));
        }
        Self {
            downloads: hit.downloads,
            ..Self::new(hit.provider.clone(), hit.reference.clone(), note.join("  "))
        }
    }

    /// A row for a reference typed in full: `owner/repo` or `name:tag`. A bare
    /// word is a search, not a tag.
    fn direct(query: &str) -> Option<Self> {
        let (provider, reference, engine) = match hugging_face_repo(query) {
            Some(repo) => (InstallProviderId::huggingface(), repo, Engine::Other),
            None => (
                InstallProviderId::ollama(),
                ollama_direct_tag(query)?,
                Engine::Ollama,
            ),
        };
        Some(Self {
            engine,
            ..Self::new(provider, reference, DIRECT_NOTE.to_owned())
        })
    }

    fn key(&self) -> PlanKey {
        (
            self.provider.as_str().to_owned(),
            self.reference.to_lowercase(),
        )
    }
}

/// A plan as the screen holds it.
#[derive(Debug, Clone, PartialEq)]
pub enum Plan {
    /// Asked for; the number of the ask, so an answer to an older one never
    /// lands.
    Pending(u64),
    /// Back, ready to pull.
    Ready(InstallPlan),
    /// The repository is gated; a token is needed first.
    Gated,
    /// The provider could not plan it, and why.
    Failed(String),
}

/// A plan's key: the provider and the lowercased reference.
type PlanKey = (String, String);

/// Plans are numbered across every screen of the run, so an answer that
/// outlives the screen it was asked from never lands in the next one.
static NEXT_PLAN: AtomicU64 = AtomicU64::new(1);

/// Where a search of the query is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Search {
    /// Nothing typed, or nothing to ask.
    Idle,
    /// Asked for once the query sits still until this tick.
    Due(u64),
    /// Asked; waiting for the answer.
    Asked,
    /// Answered.
    Done,
    /// The provider could not be asked, and why.
    Failed(String),
}

/// The note on the row for the reference exactly as typed.
const DIRECT_NOTE: &str = "as typed";

/// What `enter` did.
#[derive(Debug, Clone, PartialEq)]
pub enum Enter {
    /// The plan is ready: start the pull.
    Start(Box<InstallPlan>),
    /// The plan is on its way; the pull starts when it lands.
    Armed,
    /// The plan had not been asked for; ask it, and start when it lands.
    Plan(InstallProviderId, String, u64),
}

/// The pull screen.
#[derive(Debug, Clone, PartialEq)]
pub struct PullModal {
    /// The query being typed.
    pub input: LineEdit,
    /// The rows shown: the matches of the kind on screen.
    pub matches: Vec<Offer>,
    /// The index of the highlighted match.
    pub selected: usize,
    /// Which of [`KINDS`] is shown.
    pub kind: usize,
    /// The typed reference, normalised, when it names a model already on the
    /// shelf.
    pub direct_installed: Option<String>,
    /// When the selection last moved, on the animation clock, for its
    /// arrival.
    pub selected_at: u64,
    /// When `enter` last started a pull, on the animation clock, for the
    /// button's flash.
    pub pressed_at: Option<u64>,
    /// Every match before the kind narrows it, for the chips' counts.
    all: Vec<Offer>,
    search: Search,
    /// Hits from the last search; dropped on the next edit.
    hits: Vec<Offer>,
    installed: HashSet<String>,
    gone: HashSet<String>,
    /// Lowercased references with a pull already running.
    pulling: HashSet<String>,
    machine: Machine,
    plans: HashMap<PlanKey, Plan>,
    /// The tick the selection last moved on, for when its plan comes due.
    moved_at: u64,
    /// The row `enter` was pressed on while its plan was on its way.
    armed: Option<PlanKey>,
}

/// A line of the listing: a category heading, a match by its index, or the
/// blank that keeps one category off the next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListingRow {
    Eyebrow(InstallCategory),
    Match(usize),
    Blank,
}

impl PullModal {
    /// A fresh screen offering the machine's recommendations; `pulling`
    /// names the references already downloading.
    pub fn open(shelf: &[ModelRecord], machine: &Machine, pulling: &[String]) -> Self {
        let mut modal = Self {
            input: LineEdit::default(),
            matches: Vec::new(),
            selected: 0,
            kind: 0,
            direct_installed: None,
            selected_at: 0,
            pressed_at: None,
            all: Vec::new(),
            search: Search::Idle,
            hits: Vec::new(),
            installed: installed_names(shelf),
            gone: gone_names(shelf),
            pulling: lowercased(pulling),
            machine: machine.clone(),
            plans: HashMap::new(),
            moved_at: 0,
            armed: None,
        };
        modal.rematch();
        modal
    }

    /// The shelf or the running pulls changed under the open screen.
    pub fn refresh(&mut self, shelf: &[ModelRecord], pulling: &[String]) {
        self.installed = installed_names(shelf);
        self.gone = gone_names(shelf);
        self.pulling = lowercased(pulling);
        self.rematch();
    }

    /// The kind shown.
    pub fn kind(&self) -> Kind {
        KINDS[self.kind % KINDS.len()]
    }

    /// How many matches `kind` has, whatever is shown.
    pub fn count(&self, kind: Kind) -> usize {
        self.all.iter().filter(|offer| kind.admits(offer)).count()
    }

    /// The matches with a heading wherever the category changes, and a
    /// blank before every heading but the first, when the list is the
    /// catalog's recommendations of every kind.
    pub fn rows(&self) -> Vec<ListingRow> {
        let grouped = self.input.trimmed().is_empty() && self.kind() == Kind::All;
        let mut rows = Vec::new();
        let mut current = None;
        for (index, offer) in self.matches.iter().enumerate() {
            if grouped && offer.category.is_some() && offer.category != current {
                if current.is_some() {
                    rows.push(ListingRow::Blank);
                }
                current = offer.category;
                if let Some(category) = offer.category {
                    rows.push(ListingRow::Eyebrow(category));
                }
            }
            rows.push(ListingRow::Match(index));
        }
        rows
    }

    /// The highlighted match.
    pub fn selected_offer(&self) -> Option<&Offer> {
        self.matches.get(self.selected)
    }

    /// The plan held for `offer`, if one was asked for.
    pub fn plan(&self, offer: &Offer) -> Option<&Plan> {
        self.plans.get(&offer.key())
    }

    /// `offer`'s size: the catalog's, or its plan's once that has come back.
    pub fn size(&self, offer: &Offer) -> Option<i64> {
        offer.bytes.or_else(|| match self.plans.get(&offer.key()) {
            Some(Plan::Ready(plan)) => plan.total_bytes,
            _ => None,
        })
    }

    /// How `offer` fits this machine under its engine, by [`Self::size`],
    /// and the memory it was judged against.
    pub fn assessment(&self, offer: &Offer) -> Option<Fit> {
        self.machine.fit_on(self.engine(offer), self.size(offer))
    }

    /// The memory `offer` is judged against, by [`Self::assessment`], or all
    /// of memory while its size is unknown.
    pub fn budget_bytes(&self, offer: &Offer) -> u64 {
        self.assessment(offer)
            .map_or(self.machine.memory_bytes, |fit| fit.budget_bytes)
    }

    /// How `offer` fits this machine, by [`Self::assessment`].
    pub fn fit(&self, offer: &Offer) -> Option<FitVerdict> {
        self.assessment(offer).map(|fit| fit.assessment.verdict)
    }

    /// The engine `offer` would run on: the one it knows, or llama.cpp for
    /// one whose plan fetches a GGUF. Anything else from Hugging Face is
    /// judged against all of memory until it is pulled and resolved.
    fn engine(&self, offer: &Offer) -> Engine {
        if offer.engine != Engine::Other {
            return offer.engine;
        }
        match self.plans.get(&offer.key()) {
            Some(Plan::Ready(plan))
                if plan
                    .files
                    .iter()
                    .any(|file| file.path.to_lowercase().ends_with(".gguf")) =>
            {
                Engine::LlamaCpp
            }
            _ => Engine::Other,
        }
    }

    /// Whether `enter` is waiting on `offer`'s plan.
    pub fn armed(&self, offer: &Offer) -> bool {
        self.armed.as_ref() == Some(&offer.key())
    }

    /// Where the search of the query is.
    pub fn search(&self) -> &Search {
        &self.search
    }

    /// The machine the fits are judged on.
    pub fn machine(&self) -> &Machine {
        &self.machine
    }

    /// Edit the query with `key` on tick `now`; a change re-matches and
    /// re-arms the search.
    pub fn edit(&mut self, key: Key, now: u64) {
        if self.input.apply(key) {
            self.edited(now);
        }
    }

    /// Empty the query.
    pub fn clear_query(&mut self, now: u64) {
        self.input.clear();
        self.edited(now);
    }

    fn edited(&mut self, now: u64) {
        self.armed = None;
        self.hits.clear();
        self.rematch();
        self.moved(now);
        self.search = if self.input.trimmed().is_empty() {
            Search::Idle
        } else {
            Search::Due(now + SEARCH_DEBOUNCE_TICKS)
        };
    }

    /// The query to search on `now`, once it has sat still long enough.
    pub fn search_due(&mut self, now: u64) -> Option<String> {
        match self.search {
            Search::Due(due) if now >= due => {
                self.search = Search::Asked;
                Some(self.input.trimmed().to_owned())
            }
            _ => None,
        }
    }

    /// Fold in the hits for `query`, or why there are none; whether they
    /// applied, which they do not when the query has moved on.
    pub fn searched(
        &mut self,
        query: &str,
        hits: &[InstallSearchHit],
        note: Option<String>,
    ) -> bool {
        if query != self.input.trimmed() {
            return false;
        }
        self.hits = hits.iter().map(Offer::from_hit).collect();
        self.search = match note {
            Some(note) if hits.is_empty() => Search::Failed(note),
            _ => Search::Done,
        };
        self.rematch();
        true
    }

    /// Move the highlight by `delta` rows on tick `now`, `at` on the
    /// animation clock.
    pub fn step(&mut self, delta: isize, now: u64, at: u64) {
        let last = self.matches.len().saturating_sub(1) as isize;
        let target = (self.selected as isize + delta).clamp(0, last) as usize;
        if target != self.selected {
            // Leaving a row lets go of an enter waiting on its plan, so a
            // download never starts for a row the cursor has left.
            self.armed = None;
            self.leave_failed();
            self.selected = target;
            self.selected_at = at;
            self.moved(now);
        }
    }

    /// Show the next kind, or the one before when `delta` is negative.
    pub fn cycle_kind(&mut self, delta: isize, now: u64, at: u64) {
        let kinds = KINDS.len() as isize;
        self.kind = (self.kind as isize + delta).rem_euclid(kinds) as usize;
        self.armed = None;
        self.selected = 0;
        self.selected_at = at;
        self.rematch();
        self.moved(now);
    }

    fn moved(&mut self, now: u64) {
        self.moved_at = now;
    }

    /// A failed plan is asked again only once the cursor has left the row
    /// and come back, so a row that cannot be planned is not asked about on
    /// every tick.
    fn leave_failed(&mut self) {
        if let Some(offer) = self.selected_offer()
            && let key = offer.key()
            && matches!(self.plans.get(&key), Some(Plan::Failed(_)))
        {
            self.plans.remove(&key);
        }
    }

    /// The plan to ask for on tick `now`: the selected row's, once the
    /// cursor has rested on it, unless it is on the shelf, downloading,
    /// too big for the machine, or already asked for.
    pub fn plan_due(&mut self, now: u64) -> Option<(InstallProviderId, String, u64)> {
        if now < self.moved_at + PLAN_SETTLE_TICKS {
            return None;
        }
        let offer = self.selected_offer()?;
        // A reference still being typed is planned once its search has
        // settled, not at every pause on the way to the whole name.
        let typing = matches!(self.search, Search::Due(_) | Search::Asked);
        if (typing && offer.note == DIRECT_NOTE)
            || offer.shelf == Some(OnShelf::Present)
            || offer.pulling
            || self.fit(offer) == Some(FitVerdict::TooLarge)
            || self.plans.contains_key(&offer.key())
        {
            return None;
        }
        let (provider, reference, key) =
            (offer.provider.clone(), offer.reference.clone(), offer.key());
        let ask = NEXT_PLAN.fetch_add(1, Ordering::Relaxed);
        self.plans.insert(key, Plan::Pending(ask));
        Some((provider, reference, ask))
    }

    /// Whether a plan is on its way for the selected row.
    pub fn planning(&self) -> bool {
        self.selected_offer()
            .and_then(|offer| self.plans.get(&offer.key()))
            .is_some_and(|plan| matches!(plan, Plan::Pending(_)))
    }

    /// The plan for `reference` from `provider`, asked as `ask`, came back.
    /// An answer to an older ask never lands. When `enter` was waiting on
    /// it, what enter would have said had the plan been there already comes
    /// back: the plan to start, or why it can't be.
    pub fn planned(
        &mut self,
        provider: &InstallProviderId,
        reference: &str,
        ask: u64,
        result: Result<InstallPlan, String>,
    ) -> Option<Result<InstallPlan, String>> {
        let key = (provider.as_str().to_owned(), reference.to_lowercase());
        if self.plans.get(&key) != Some(&Plan::Pending(ask)) {
            return None;
        }
        let plan = match result {
            Ok(plan) if plan.requires_auth => Plan::Gated,
            Ok(plan) => Plan::Ready(plan),
            Err(reason) => Plan::Failed(reason),
        };
        self.plans.insert(key.clone(), plan);
        if self.armed.as_ref() != Some(&key) {
            return None;
        }
        self.armed = None;
        if self.selected_offer().map(Offer::key) != Some(key) {
            return None;
        }
        Some(match self.enter() {
            Ok(Enter::Start(plan)) => Ok(*plan),
            Ok(_) => Err(format!("{reference} has no plan yet")),
            Err(reason) => Err(reason),
        })
    }

    /// `enter` on the selected row: start it, arm it for its plan, ask for
    /// its plan and arm it, or say why not.
    pub fn enter(&mut self) -> Result<Enter, String> {
        let offer = self
            .selected_offer()
            .ok_or_else(|| "nothing to pull".to_owned())?
            .clone();
        if offer.shelf == Some(OnShelf::Present) {
            return Err(format!("{} is already on the shelf", offer.reference));
        }
        if offer.pulling {
            return Err(already_downloading(&offer.reference));
        }
        if self.fit(&offer) == Some(FitVerdict::TooLarge) {
            return Err(format!(
                "{} needs {} GiB; this machine can give a model {}",
                offer.reference,
                text::gib(self.size(&offer).unwrap_or(0)),
                text::gib(self.budget_bytes(&offer) as i64)
            ));
        }
        let key = offer.key();
        match self.plans.get(&key) {
            Some(Plan::Ready(plan)) => Ok(Enter::Start(Box::new(plan.clone()))),
            Some(Plan::Gated) => Err(format!(
                "{} is gated; add a Hugging Face token first",
                offer.reference
            )),
            Some(Plan::Failed(reason)) => Err(reason.clone()),
            Some(Plan::Pending(_)) => {
                self.armed = Some(key);
                Ok(Enter::Armed)
            }
            None => {
                let ask = NEXT_PLAN.fetch_add(1, Ordering::Relaxed);
                self.plans.insert(key.clone(), Plan::Pending(ask));
                self.armed = Some(key);
                Ok(Enter::Plan(offer.provider, offer.reference, ask))
            }
        }
    }

    /// The typed reference itself, the catalog's recommendations (narrowed
    /// by the query when there is one), and the search hits, each marked
    /// when it is on the shelf already, without repeats; then the kind
    /// shown narrows them.
    fn rematch(&mut self) {
        let typed = self.input.trimmed();
        let query = typed.to_lowercase();
        let catalog_room = MAX_MATCHES.saturating_sub(self.hits.len());
        let mut matches: Vec<Offer> = Vec::new();
        let direct = Offer::direct(typed);
        self.direct_installed = direct
            .as_ref()
            .filter(|row| is_installed(&row.reference, &self.installed))
            .map(|row| row.reference.clone());
        matches.extend(direct);
        let ask = Ask {
            categories: &[],
            installed: &self.installed,
            all: false,
        };
        let mut room = catalog_room;
        for rec in recommend(&self.machine, &ask).iter().filter(|rec| {
            query.is_empty()
                || rec.entry.reference.to_lowercase().contains(&query)
                || rec.entry.name.to_lowercase().contains(&query)
        }) {
            if rec.status == Status::Pick {
                if room == 0 {
                    continue;
                }
                room -= 1;
            }
            matches.push(Offer::from_catalog(rec));
        }
        matches.extend(self.hits.iter().cloned());
        let mut seen = HashSet::new();
        matches
            .retain(|offer| seen.insert((offer.provider.clone(), offer.reference.to_lowercase())));
        for offer in &mut matches {
            offer.pulling = self.pulling.contains(&offer.reference.to_lowercase());
            offer.shelf = if is_installed(&offer.reference, &self.installed) {
                Some(OnShelf::Present)
            } else if is_installed(&offer.reference, &self.gone) {
                Some(OnShelf::Gone)
            } else {
                None
            };
        }
        let kind = self.kind();
        self.matches = matches
            .iter()
            .filter(|offer| kind.admits(offer))
            .cloned()
            .collect();
        self.all = matches;
        self.selected = self.selected.min(self.matches.len().saturating_sub(1));
    }

    /// A screen whose one row is `plan`'s reference, already planned, for a
    /// test that starts from a ready preview.
    #[cfg(test)]
    pub fn ready(plan: InstallPlan) -> Self {
        let mut modal = Self::open(&[], &Machine::default(), &[]);
        let offer = Offer::new(plan.provider.clone(), plan.reference.clone(), String::new());
        modal.plans.insert(offer.key(), Plan::Ready(plan));
        modal.all = vec![offer.clone()];
        modal.matches = vec![offer];
        modal.selected = 0;
        modal
    }
}

/// The names the shelf's gone records go by, the way [`installed_names`]
/// names the present ones.
fn gone_names(shelf: &[ModelRecord]) -> HashSet<String> {
    shelf
        .iter()
        .filter(|record| record.state == ModelState::Missing)
        .flat_map(|record| {
            [
                record.id.to_lowercase(),
                record.name.to_lowercase(),
                record.display_name().to_lowercase(),
            ]
        })
        .collect()
}

/// The notice for a reference that is already being pulled.
pub fn already_downloading(reference: &str) -> String {
    format!("{reference} is already downloading")
}

fn lowercased(references: &[String]) -> HashSet<String> {
    references
        .iter()
        .map(|reference| reference.to_lowercase())
        .collect()
}

#[cfg(test)]
mod tests;
