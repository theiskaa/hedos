//! The whole UI state and the reducer over it. Pure: it never touches the
//! kernel or the terminal, so every transition is unit-testable.

use std::time::Duration;

use kernel::bench::Status as BenchStatus;
use kernel::install::pulls::PullState;
use kernel::profiles::FitVerdict;
use kernel::records::{Capability, ModelRecord, ModelState};
use kernel::removal::{ModelDeletionPreview, is_deletable, preview};
use ratatui::widgets::TableState;
use runtime::bench::BenchPlan;
use runtime::install::WorkerError;

use kernel::install::plan::InstallPlan;

use super::bench::BenchScreen;
use super::chat::{ChatPane, Judged};
use super::edit::LineEdit;
use super::effect::{Effect, HandOff};
use super::event::{Event, Key, Planned, Refreshed, Reply, ReplyStep, Searched};
use super::facts::Facts;
use super::jobs::JobRow;
use super::launch::LaunchModal;
use super::layout;
use super::motion::{Eased, EasedSet, Motion};
use super::order::{Sort, order};
use super::palette::{Depth, Ground};
use super::pull::{Enter, PullModal, Search, already_downloading};
use super::pulls::PullsScreen;
use super::state::UiState;
use super::stop::{StopCard, StopChoice};
use super::strip::{HintTargets, TaskStrip};
use super::tasks::{PullAction, TaskEvent, TaskId, TaskKind, TaskLabel, TaskState};
use crate::support::bench_run::{machine_line, rows as bench_rows};
use crate::support::install::find_installed;
use crate::support::judge;
use crate::support::residency::{Holder, warm_request};
use crate::support::shelf_table::verdict;
use crate::tui::bench_view::Board;

/// How often the loop ticks; every cadence below is counted in these.
pub(super) const TICK: Duration = Duration::from_millis(250);
pub(super) const TICKS_PER_SECOND: u64 = 1000 / TICK.as_millis() as u64;
/// Refresh cadence while a task runs, and while idle.
const BUSY_REFRESH_TICKS: u64 = 2 * TICKS_PER_SECOND;
const IDLE_REFRESH_TICKS: u64 = 10 * TICKS_PER_SECOND;
/// How often the job directory is read while a pull is going. The worker
/// rewrites its record about twice a second, so reading faster than this would
/// only re-read the same figures.
const PULL_POLL_TICKS: u64 = TICKS_PER_SECOND / 2;
/// How often it is read while none is, which is how a pull started in a
/// terminal turns up here.
const PULL_IDLE_POLL_TICKS: u64 = 2 * TICKS_PER_SECOND;
/// How far a page key moves the chat transcript, and a wheel notch.
const PAGE_LINES: usize = 10;
const WHEEL_LINES: usize = 3;
/// How often a frame is drawn while something moves continuously: the
/// koala, an intro, a figure easing.
const MOTION_FRAME: Duration = Duration::from_millis(40);
/// How often a frame is drawn for the gateway dot's pulse alone.
const PULSE_FRAME: Duration = Duration::from_millis(100);
/// How long after the selection moves its card is still settling: the
/// name's reveal, the gauge's growth, the sparkline's rise.
const SELECTION_SETTLE_MS: u64 = 400;
/// How often a blinking cursor needs a frame.
const BLINK_FRAME: Duration = Duration::from_millis(500);
/// How long a card takes to open: the backdrop's fade and its grow.
const CARD_OPEN_MS: u64 = 200;
/// How long the launch's movements take, the body's fade the last of them.
const INTRO_MS: u64 = 1200;
/// How long a footer notice stays.
const NOTICE_TICKS: u64 = 2 * TICKS_PER_SECOND;

/// Which surface has the body: the shelf, or the pulls screen in its place.
/// Modals sit in front of either.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Shelf,
    Pulls,
    Bench,
}

/// What can sit in front of the shelf.
#[derive(Debug, Clone, PartialEq)]
pub enum Modal {
    /// Choosing a model to download.
    Pull(Box<PullModal>),
    /// Confirming a removal, with what it would delete.
    Remove(ModelDeletionPreview),
    /// Choosing how to stop a pull, with what each way keeps.
    Stop(StopCard),
    /// The key table.
    Help,
    /// Choosing a harness to launch on the selected model.
    Launch(Box<LaunchModal>),
    /// A conversation with the selected model, in place of the shelf.
    Chat(Box<ChatPane>),
}

/// Why a verb is refused for the selected model: quietly, when the model
/// is busy and the strip already shows with what, or with a reason for the
/// footer.
enum Refusal {
    Busy,
    Because(String),
}

/// A footer message and the tick it expires on.
struct Notice {
    text: String,
    until: u64,
    /// When it was raised, on the animation clock, for its reveal and fade.
    shown_at: u64,
}

/// Everything the screen shows.
pub struct App {
    /// The shelf, in the order it is listed.
    pub records: Vec<ModelRecord>,
    /// The machine facts from the last refresh.
    pub facts: Facts,
    /// The shelf's selection and scroll position, as rows of [`Self::order`];
    /// ratatui keeps the selected row in view through it.
    pub shelf: TableState,
    /// The indices into `records` the shelf shows, filtered and sorted.
    pub order: Vec<usize>,
    /// The fuzzy filter over the shelf.
    pub filter: LineEdit,
    /// Whether keys are typing into the filter.
    pub filtering: bool,
    /// The sort in effect.
    pub sort: Sort,
    /// Background work and what ran in the foreground, oldest first.
    pub tasks: TaskStrip,
    /// The modal in front of the shelf, while one is open.
    pub modal: Option<Modal>,
    /// Which surface has the body.
    pub screen: Screen,
    /// The pulls screen, kept while the shelf shows so that opening it is
    /// instant and its selection survives a visit to the shelf.
    pub pulls: PullsScreen,
    /// The bench screen, kept the same way: a bench goes on while the shelf is
    /// showing, and the screen picks it back up.
    pub bench: BenchScreen,
    /// How many benches have been started, so each one's steps carry a stamp
    /// its predecessor's cannot be mistaken for.
    benches: u64,
    /// Whether the detail pane has the whole body.
    pub expanded: bool,
    /// How many colours the terminal shows; the frame is mapped onto its
    /// palette when that is 256.
    pub depth: Depth,
    /// The terminal's own ground, which the finished frame is handed to.
    pub ground: Ground,
    /// The animation clock; settled unless the loop says otherwise, so a
    /// test always sees a movement's final frame.
    pub motion: Motion,
    /// The header's big figures (models, warm, gone, free GiB), each easing
    /// to its value.
    pub header_figures: [Eased; 4],
    /// The memory bar's segment per loaded model, each easing to its size.
    pub resident_bars: EasedSet,
    /// The conversation last closed while idle, so `t` on the same model
    /// takes it up again.
    kept_chat: Option<Box<ChatPane>>,
    /// A short message in the footer, until the tick it expires on.
    notice: Option<Notice>,
    /// A reference just pulled; the next refresh selects the record it became.
    select_pulled: Option<String>,
    /// Ticks since the loop started; every cadence is counted in these.
    ticks: u64,
    /// The tick of the last refresh request.
    last_refresh: u64,
    /// The tick of the last read of the job directory.
    last_pull_poll: u64,
    /// How many strip rows the last frame actually had room for. A key must act
    /// on the same row the hint was drawn on, and on a terminal too short for
    /// the whole strip that is fewer rows than the layout asked for.
    drawn_task_rows: std::cell::Cell<usize>,
    /// The size the last frame was drawn at.
    drawn_area: std::cell::Cell<ratatui::layout::Rect>,
    /// The model the selection was on when last drawn, and when, on the
    /// animation clock, it arrived there.
    selection_seen: std::cell::RefCell<Option<(Option<String>, u64)>>,
    /// Which kind of card was open when last drawn, and when it opened.
    modal_seen: std::cell::Cell<Option<(std::mem::Discriminant<Modal>, u64)>>,
    /// The sequence of the last refresh applied, so older ones are dropped.
    applied_refresh: u64,
    dirty: bool,
}

impl App {
    /// A UI over `records`, selecting the first.
    pub fn new(records: Vec<ModelRecord>, facts: Facts) -> Self {
        let order = (0..records.len()).collect();
        Self {
            records,
            facts,
            shelf: TableState::new().with_selected(0),
            order,
            filter: LineEdit::default(),
            filtering: false,
            sort: Sort::default(),
            tasks: TaskStrip::default(),
            modal: None,
            screen: Screen::Shelf,
            pulls: PullsScreen::default(),
            bench: BenchScreen::default(),
            benches: 0,
            expanded: false,
            depth: Depth::default(),
            ground: Ground::default(),
            motion: Motion::settled(),
            header_figures: Default::default(),
            resident_bars: EasedSet::default(),
            kept_chat: None,
            notice: None,
            select_pulled: None,
            ticks: 0,
            last_refresh: 0,
            last_pull_poll: 0,
            drawn_task_rows: std::cell::Cell::new(layout::MAX_TASK_ROWS as usize),
            drawn_area: std::cell::Cell::new(ratatui::layout::Rect::default()),
            selection_seen: std::cell::RefCell::new(None),
            modal_seen: std::cell::Cell::new(None),
            applied_refresh: 0,
            dirty: true,
        }
    }

    /// The index of the selected row.
    pub fn selected(&self) -> usize {
        self.shelf.selected().unwrap_or(0)
    }

    /// The selected record, if the shelf shows any.
    pub fn selected_record(&self) -> Option<&ModelRecord> {
        self.order
            .get(self.selected())
            .and_then(|&index| self.records.get(index))
    }

    /// Whether the pull `reference` names the selected record, by the same
    /// matching the pull modal uses to tell what is already on the shelf.
    pub fn selected_is(&self, reference: &str) -> bool {
        match (
            self.selected_record(),
            find_installed(&self.records, reference),
        ) {
            (Some(selected), Some(named)) => selected.id == named.id,
            _ => false,
        }
    }

    /// The records the shelf shows, in order.
    pub fn shown(&self) -> impl Iterator<Item = &ModelRecord> {
        self.order
            .iter()
            .filter_map(|&index| self.records.get(index))
    }

    /// Apply a remembered `state`: its selection, when that model is still on
    /// the shelf.
    pub fn restore(&mut self, state: &UiState) {
        self.reorder(state.selected_id.clone());
        self.tasks
            .remember_dismissed(state.dismissed.iter().cloned());
    }

    /// What to remember for the next run.
    pub fn remembered(&self) -> UiState {
        UiState {
            selected_id: self.selected_record().map(|record| record.id.clone()),
            dismissed: self.tasks.dismissed_jobs(),
        }
    }

    /// Recompute the shown rows around the current selection.
    fn reorder_in_place(&mut self) {
        let keep = self.selected_record().map(|record| record.id.clone());
        self.reorder(keep);
    }

    /// Recompute the shown rows, keeping `keep` selected when it is still
    /// shown.
    fn reorder(&mut self, keep: Option<String>) {
        self.order = order(&self.records, &self.facts, self.filter.as_str(), self.sort);
        let index = keep
            .and_then(|id| {
                self.order
                    .iter()
                    .position(|&index| self.records[index].id == id)
            })
            .unwrap_or(self.selected());
        self.shelf
            .select(Some(index.min(self.order.len().saturating_sub(1))));
        self.dirty = true;
    }

    /// Open the pull modal when the shelf is empty, so a first run lands on
    /// something to do rather than a blank pane.
    pub fn offer_pull_when_empty(&mut self) {
        if self.records.is_empty() {
            self.modal = Some(Modal::Pull(Box::new(PullModal::open(
                &self.records,
                self.facts.memory_bytes,
                &[],
            ))));
            self.dirty = true;
        }
    }

    /// What can be done with the selected model right now, as keys in footer
    /// order: exactly the keys whose guards would let them through. The
    /// keymap names their verbs.
    pub fn actions(&self) -> Vec<&'static str> {
        let Some(record) = self.selected_record() else {
            return Vec::new();
        };
        let mut actions = Vec::new();
        if self.warmable(record).is_ok() {
            actions.push("w");
        }
        if self.unloadable(record).is_ok() {
            actions.push("u");
        }
        if Self::chat_capable(record).is_ok() {
            actions.extend(["l", "t", "T"]);
        } else if judge::is_judge(record) {
            actions.push("t");
        }
        if self.removable(record).is_ok() {
            actions.push("x");
        }
        if record.primary_weight_path.is_some() {
            actions.push("y");
        }
        actions
    }

    /// Why `record` can't be warmed right now, if it can't.
    fn warmable(&self, record: &ModelRecord) -> Result<(), Refusal> {
        let name = record.display_name();
        if self.tasks.running_on(&record.id) {
            Err(Refusal::Busy)
        } else if self.facts.is_warm(&record.id) {
            Err(Refusal::Because(format!("{name} is already warm")))
        } else if record.state == ModelState::Missing {
            Err(Refusal::Because(format!("{name}'s weights are gone")))
        } else if verdict(record.serving_size(), self.facts.memory_bytes)
            == Some(FitVerdict::TooLarge)
        {
            Err(Refusal::Because(format!(
                "{name} is too big for this machine"
            )))
        } else if warm_request(record).is_none() {
            Err(Refusal::Because(format!("{name} can't be warmed")))
        } else {
            Ok(())
        }
    }

    /// Why `record` can't be unloaded from here, if it can't.
    fn unloadable(&self, record: &ModelRecord) -> Result<(), Refusal> {
        let name = record.display_name();
        if self.tasks.running_on(&record.id) {
            return Err(Refusal::Busy);
        }
        match self
            .facts
            .resident(&record.id)
            .map(|resident| resident.holder)
        {
            None => Err(Refusal::Because(format!("{name} is not warm"))),
            Some(Holder::Gateway) => {
                let port = self.facts.gateway_port.unwrap_or_default();
                Err(Refusal::Because(format!(
                    "{name} is held by the gateway on :{port}; it unloads there after its warm window"
                )))
            }
            Some(Holder::Local | Holder::Daemon) => Ok(()),
        }
    }

    /// Why `record` can't chat, if it can't.
    fn chat_capable(record: &ModelRecord) -> Result<(), Refusal> {
        if record.capabilities.contains(&Capability::chat()) {
            Ok(())
        } else {
            Err(Refusal::Because(format!(
                "{} can't chat",
                record.display_name()
            )))
        }
    }

    /// Answer a refusal: silence for a busy model, the reason otherwise.
    fn refuse(&mut self, refusal: Refusal) -> Vec<Effect> {
        match refusal {
            Refusal::Busy => Vec::new(),
            Refusal::Because(reason) => self.notify(reason),
        }
    }

    /// Put `modal` in front of the shelf.
    fn open(&mut self, modal: Modal) {
        self.modal = Some(modal);
        self.dirty = true;
    }

    /// Take down whatever is in front of the shelf.
    fn close_modal(&mut self) {
        self.modal = None;
        self.dirty = true;
    }

    /// The footer notice, if one is showing.
    pub fn notice(&self) -> Option<&str> {
        self.notice.as_ref().map(|notice| notice.text.as_str())
    }

    /// When the current notice was raised, on the animation clock.
    pub fn notice_at(&self) -> Option<u64> {
        self.notice.as_ref().map(|notice| notice.shown_at)
    }

    /// Whether any task is still running.
    pub fn busy(&self) -> bool {
        self.tasks.busy()
    }

    /// Whether something changed since the last draw; reading it clears it.
    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    /// Record that the loop started `kind` as task `id`.
    pub fn started(&mut self, id: TaskId, kind: TaskKind) {
        self.tasks.start(id, kind);
        self.dirty = true;
    }

    /// Apply `event` and return the effects the loop must perform.
    pub fn reduce(&mut self, event: Event) -> Vec<Effect> {
        match event {
            Event::Key(key) => self.key(key),
            Event::Resize => {
                self.dirty = true;
                Vec::new()
            }
            Event::Tick => self.tick(),
            Event::Frame => {
                self.dirty = true;
                Vec::new()
            }
            Event::Pulls(rows) => self.pulls_polled(rows),
            Event::History { job, lines } => {
                self.history(job, lines);
                Vec::new()
            }
            Event::PullStarted(job) => {
                self.pulls.follow(job);
                Vec::new()
            }
            Event::PullRefused(reason) => self.notify(reason),
            Event::Task(event) => self.task(event),
            Event::Refreshed(refreshed) => {
                self.refreshed(refreshed);
                Vec::new()
            }
            Event::Searched(searched) => {
                self.searched(searched);
                Vec::new()
            }
            Event::Planned(planned) => self.planned(planned),
            Event::Reply(reply) => self.reply(reply),
            Event::Bench { generation, step } => {
                if self.bench.apply(generation, &step) && self.screen == Screen::Bench {
                    self.bench.follow_running();
                    self.dirty = true;
                }
                Vec::new()
            }
            Event::BenchEnded(generation) => {
                // A driver that returned with rows still waiting was stopped;
                // saying so beats leaving them reading as queued forever.
                self.bench.stopped(generation);
                self.dirty = true;
                Vec::new()
            }
            Event::InputClosed => vec![Effect::Quit],
        }
    }

    /// The pull screen, while it is open.
    pub fn pull_screen(&self) -> Option<&PullModal> {
        match &self.modal {
            Some(Modal::Pull(modal)) => Some(modal),
            _ => None,
        }
    }

    /// The chat pane, while it is open.
    pub fn chat_pane(&self) -> Option<&ChatPane> {
        match &self.modal {
            Some(Modal::Chat(pane)) => Some(pane),
            _ => None,
        }
    }

    /// Set the animation clock to `now_ms` since the screen first opened.
    pub fn set_clock(&mut self, now_ms: u64) {
        self.motion.set(now_ms);
    }

    /// Which frame a spinner shows now.
    pub fn spin_frame(&self) -> u64 {
        self.motion.spin_frame(self.ticks)
    }

    /// How soon something on screen changes on its own and needs a frame
    /// drawn, or `None` when nothing moves until an event arrives.
    pub fn frame_due(&self) -> Option<Duration> {
        if !self.motion.is_live() {
            return None;
        }
        let mut due: Option<Duration> = None;
        let mut want = |interval: Duration| {
            due = Some(due.map_or(interval, |due| due.min(interval)));
        };
        // The koala lives in the hero, or beside an empty shelf's copy when
        // the header is one line.
        let hero = layout::Panes::hero(self.drawn_area.get())
            || (self.records.is_empty() && self.screen == Screen::Shelf);
        let intro = self.motion.now_ms() < INTRO_MS;
        // The try screen's name arrives a letter at a time as it opens.
        let arriving = self
            .chat_pane()
            .is_some_and(|pane| self.motion.now_ms() < pane.opened_at + SELECTION_SETTLE_MS);
        let easing = self
            .header_figures
            .iter()
            .any(|figure| figure.moving(crate::tui::ui::FIGURES_MS, &self.motion))
            || self
                .resident_bars
                .moving(crate::tui::ui::SEGMENT_MS, &self.motion);
        let settling = self.motion.now_ms() < self.selected_at() + SELECTION_SETTLE_MS;
        let noticing = self.notice.is_some();
        let opening = self
            .modal_at()
            .is_some_and(|at| self.motion.now_ms() < at + CARD_OPEN_MS);
        if intro || easing || settling || noticing || opening || arriving {
            want(self.motion.scaled(MOTION_FRAME));
        }
        // Past the launch the koala only sways and breathes, which reads
        // as smooth at the pulse's rate for less than half the frames.
        if hero || self.facts.gateway_port.is_some() || self.breathing() {
            want(self.motion.scaled(PULSE_FRAME));
        }
        if self.spinning() || self.chat_pane().is_some_and(ChatPane::streaming) {
            want(self.motion.spin_interval());
        }
        if self.chat_pane().is_some() || self.pull_screen().is_some() {
            // The input's cursor blinks.
            want(self.motion.scaled(BLINK_FRAME));
        }
        // A download's bar shimmers, on the strip or on the pull screen.
        if self.tasks.any_pulling() {
            want(self.motion.scaled(MOTION_FRAME));
        }
        if let Some(modal) = self.pull_screen() {
            let now = self.motion.now_ms();
            let pressed = modal
                .pressed_at
                .is_some_and(|at| now < at + SELECTION_SETTLE_MS);
            if now < modal.selected_at + SELECTION_SETTLE_MS || pressed {
                want(self.motion.scaled(MOTION_FRAME));
            }
        }
        due
    }

    /// When the selection arrived on the model it is on now, on the
    /// animation clock. The selection the screen opens with counts as
    /// arriving at the start, so it plays with the launch rather than after.
    pub fn selected_at(&self) -> u64 {
        let id = self.selected_record().map(|record| record.id.clone());
        let mut seen = self.selection_seen.borrow_mut();
        match &*seen {
            Some((last, at)) if *last == id => *at,
            Some(_) => {
                let at = self.motion.now_ms();
                *seen = Some((id, at));
                at
            }
            None => {
                *seen = Some((id, 0));
                0
            }
        }
    }

    /// When the open card opened, on the animation clock; a card that
    /// changes what it shows inside is still the card that opened.
    pub fn modal_at(&self) -> Option<u64> {
        // The chat pane takes the body rather than opening over it.
        let Some(modal) = self
            .modal
            .as_ref()
            .filter(|modal| !matches!(modal, Modal::Chat(_)))
        else {
            self.modal_seen.set(None);
            return None;
        };
        let kind = std::mem::discriminant(modal);
        match self.modal_seen.get() {
            Some((seen, at)) if seen == kind => Some(at),
            _ => {
                let at = self.motion.now_ms();
                self.modal_seen.set(Some((kind, at)));
                Some(at)
            }
        }
    }

    /// Note the size the last frame was drawn at, which decides whether the
    /// hero and its koala are on screen.
    pub fn note_area(&self, area: ratatui::layout::Rect) {
        self.drawn_area.set(area);
    }

    /// Whether something breathes on screen: a warm model's mark, or the
    /// selected model's share of the fit gauge while it is only what warming
    /// it would take.
    fn breathing(&self) -> bool {
        let warm = self
            .records
            .iter()
            .any(|record| self.facts.is_warm(&record.id));
        let gauge = self.selected_record().is_some_and(|record| {
            record.serving_size().is_some()
                && record.state != ModelState::Missing
                && !self.facts.is_warm(&record.id)
        });
        self.screen == Screen::Shelf
            && self.chat_pane().is_none()
            && self.pull_screen().is_none()
            && (warm || gauge)
    }

    /// Whether a spinner is on screen: a reply waited on, a plan being
    /// made, a task on the strip, a bench running on its screen.
    fn spinning(&self) -> bool {
        self.chat_pane().is_some_and(ChatPane::waiting)
            || self.tasks.busy()
            || self.planning()
            || (self.screen == Screen::Bench && self.bench.running())
    }

    /// The chat pane, mutably, while it is open.
    pub fn chat_pane_mut(&mut self) -> Option<&mut ChatPane> {
        match &mut self.modal {
            Some(Modal::Chat(pane)) => Some(pane),
            _ => None,
        }
    }

    fn key(&mut self, key: Key) -> Vec<Effect> {
        if key == Key::Interrupt && !matches!(self.modal, Some(Modal::Chat(_))) {
            return vec![Effect::Quit];
        }
        match self.modal {
            Some(Modal::Chat(_)) => return self.chat_key(key),
            Some(Modal::Pull(_)) => return self.pull_key(key),
            Some(Modal::Remove(_)) => return self.remove_key(key),
            Some(Modal::Stop(_)) => return self.stop_key(key),
            Some(Modal::Help) => return self.help_key(key),
            Some(Modal::Launch(_)) => return self.launch_key(key),
            None => {}
        }
        if self.screen == Screen::Pulls {
            return self.pulls_key(key);
        }
        if self.screen == Screen::Bench {
            return self.bench_key(key);
        }
        if self.filtering && !matches!(key, Key::Up | Key::Down | Key::ScrollUp | Key::ScrollDown) {
            return self.filter_key(key);
        }
        match key {
            Key::Char('q') => return vec![Effect::Quit],
            Key::Down | Key::ScrollDown | Key::Char('j') => {
                self.select(self.selected().saturating_add(1));
            }
            Key::Up | Key::ScrollUp | Key::Char('k') => {
                self.select(self.selected().saturating_sub(1));
            }
            Key::Top | Key::Char('g') => self.select(0),
            Key::Bottom | Key::Char('G') => self.select(usize::MAX),
            Key::Char('s') if !self.tasks.already_running(&TaskKind::Scan) => {
                return vec![Effect::Spawn(TaskKind::Scan)];
            }
            Key::Char('r') => return self.refresh(),
            Key::Char('w') => return self.warm(),
            Key::Char('u') => return self.unload(),
            Key::Char('p') => self.open_pull_modal(),
            Key::Char('P') => return self.open_pulls(),
            Key::Char('b') => return self.bench_selected(),
            Key::Char('B') => return self.open_bench(),
            Key::Char('x') => return self.remove(),
            Key::Char('l') => return self.launch(),
            Key::Char('T') => return self.hand_off_chat(),
            Key::Char('t') if self.bench.running() => {
                return self.notify("a bench is running; press B, then c to stop it".to_owned());
            }
            Key::Char('t') => return self.open_chat(),
            Key::Char('S') => return self.serve(),
            Key::Char('/') => {
                self.filtering = true;
                self.expanded = false;
                self.dirty = true;
            }
            Key::Escape if self.expanded => {
                self.expanded = false;
                self.dirty = true;
            }
            Key::Escape if !self.filter.is_empty() => {
                self.filter.clear();
                self.reorder_in_place();
            }
            Key::Char('o') => {
                self.sort = self.sort.next();
                self.reorder_in_place();
            }
            Key::Char('y') => return self.copy_path(),
            Key::Char('Y') => return self.copy_id(),
            Key::Char('d') => return self.dismiss(),
            Key::Char('?') => self.open(Modal::Help),
            Key::Enter if self.selected_record().is_some() => {
                self.expanded = !self.expanded;
                self.dirty = true;
            }
            Key::Char('c') => return self.stop_pull(),
            Key::Char('R') => return self.resume_pull(),
            _ => {}
        }
        Vec::new()
    }

    /// The keys of the pull screen: typing searches, the arrows and the
    /// wheel move, `tab` steps through the kinds, `enter` pulls, and `esc`
    /// clears what was typed before it leaves.
    fn pull_key(&mut self, key: Key) -> Vec<Effect> {
        let now = self.ticks;
        let at = self.motion.now_ms();
        let Some(Modal::Pull(modal)) = self.modal.as_mut() else {
            return Vec::new();
        };
        match key {
            Key::Escape if !modal.input.is_empty() => modal.clear_query(now),
            Key::Escape => self.close_modal(),
            Key::Up | Key::ScrollUp => modal.step(-1, now, at),
            Key::Down | Key::ScrollDown => modal.step(1, now, at),
            Key::PageUp => modal.step(-(PAGE_LINES as isize), now, at),
            Key::PageDown => modal.step(PAGE_LINES as isize, now, at),
            Key::Top => modal.step(isize::MIN / 2, now, at),
            Key::Bottom => modal.step(isize::MAX / 2, now, at),
            Key::Tab => modal.cycle_kind(1, now, at),
            Key::BackTab => modal.cycle_kind(-1, now, at),
            Key::Char(_) | Key::Backspace | Key::Edit(_) => modal.edit(key, now),
            Key::Enter => {
                return match modal.enter() {
                    Ok(Enter::Start(plan)) => self.start_pull(*plan),
                    Ok(Enter::Armed) => {
                        self.dirty = true;
                        Vec::new()
                    }
                    Ok(Enter::Plan(provider, reference, ask)) => {
                        self.dirty = true;
                        vec![Effect::Plan(provider, reference, ask)]
                    }
                    Err(reason) => self.notify(reason),
                };
            }
            Key::Clear | Key::Interrupt => return Vec::new(),
        }
        self.dirty = true;
        Vec::new()
    }

    /// Hand `plan` to a worker, unless it is already downloading; the
    /// screen stays, its button following the download.
    fn start_pull(&mut self, plan: InstallPlan) -> Vec<Effect> {
        if self.tasks.is_pulling(&plan.reference) {
            return self.notify(already_downloading(&plan.reference));
        }
        if let Some(Modal::Pull(modal)) = self.modal.as_mut() {
            modal.pressed_at = Some(self.motion.now_ms());
        }
        let mut effects = self.notify(format!(
            "pulling {} · it runs on if you quit",
            plan.reference
        ));
        effects.push(Effect::StartPull(Box::new(plan)));
        effects
    }

    fn filter_key(&mut self, key: Key) -> Vec<Effect> {
        self.dirty = true;
        match key {
            Key::Char(_) | Key::Backspace | Key::Edit(_) => {
                if self.filter.apply(key) {
                    self.reorder_in_place();
                }
            }
            Key::Escape => {
                self.filter.clear();
                self.filtering = false;
                self.reorder_in_place();
            }
            Key::Enter => {
                self.filtering = false;
                self.dirty = true;
            }
            _ => {}
        }
        Vec::new()
    }

    /// Copy the selected model's weights path, or say it has none.
    fn copy_path(&mut self) -> Vec<Effect> {
        let Some(record) = self.selected_record() else {
            return Vec::new();
        };
        match record.primary_weight_path.clone() {
            Some(path) => self.copy(path),
            None => self.notify(format!("{} has no path", record.display_name())),
        }
    }

    /// Copy the selected model's id.
    fn copy_id(&mut self) -> Vec<Effect> {
        match self.selected_record() {
            Some(record) => {
                let id = record.id.clone();
                self.copy(id)
            }
            None => Vec::new(),
        }
    }

    fn copy(&mut self, text: String) -> Vec<Effect> {
        self.notify("copied".to_owned());
        vec![Effect::Copy(text)]
    }

    /// The rows the strip's hints act on: the strip is as tall as its rows
    /// up to the layout's cap, so a target under older rows than that shows
    /// no hint and the key leaves it alone.
    fn hint_targets(&self) -> HintTargets {
        self.tasks
            .hint_targets(self.drawn_task_rows.get(), |reference| {
                self.selected_is(reference)
            })
    }

    /// Record how many strip rows the frame being drawn has room for, so the
    /// keys act on exactly the rows that carry their hints.
    pub(super) fn note_task_rows(&self, rows: usize) {
        self.drawn_task_rows.set(rows);
    }

    /// Drop the newest failed task from the strip, when its row is on screen.
    fn dismiss(&mut self) -> Vec<Effect> {
        if self.hint_targets().failure().is_some() && self.tasks.dismiss_newest_failure() {
            self.dirty = true;
            Vec::new()
        } else {
            self.notify("nothing to dismiss".to_owned())
        }
    }

    /// Open the harness picker for the selected model, or say why not.
    fn launch(&mut self) -> Vec<Effect> {
        match self.chatting_record() {
            Ok(record) => {
                self.open(Modal::Launch(Box::new(LaunchModal::open(&record))));
                Vec::new()
            }
            Err(Refusal::Because(reason)) => {
                self.notify(format!("{reason}, so no harness can use it"))
            }
            Err(refusal) => self.refuse(refusal),
        }
    }

    fn launch_key(&mut self, key: Key) -> Vec<Effect> {
        let Some(Modal::Launch(modal)) = self.modal.as_mut() else {
            return Vec::new();
        };
        match key {
            Key::Escape => self.close_modal(),
            Key::Up | Key::ScrollUp | Key::Char('k') => {
                modal.step(-1);
                self.dirty = true;
            }
            Key::Down | Key::ScrollDown | Key::Char('j') => {
                modal.step(1);
                self.dirty = true;
            }
            Key::Enter => {
                let row = modal.selected_row().clone();
                if let Some(reason) = row.blocked {
                    return self.notify(reason);
                }
                let Some(program) = row.program else {
                    return Vec::new();
                };
                let hand_off = HandOff::Launch {
                    harness: row.spec,
                    program,
                    record: Box::new(modal.record.clone()),
                };
                self.close_modal();
                return vec![Effect::HandOff(Box::new(hand_off))];
            }
            _ => {}
        }
        Vec::new()
    }

    /// The help closes on escape, its own key, or quit; nothing else reaches
    /// the shelf through it.
    fn help_key(&mut self, key: Key) -> Vec<Effect> {
        if matches!(key, Key::Escape | Key::Char('?') | Key::Char('q')) {
            self.close_modal();
        }
        Vec::new()
    }

    /// Hand off to `hedos chat` on the selected model.
    fn hand_off_chat(&mut self) -> Vec<Effect> {
        match self.chatting_record() {
            Ok(record) => vec![Effect::HandOff(Box::new(HandOff::Chat {
                record: Box::new(record),
            }))],
            Err(refusal) => self.refuse(refusal),
        }
    }

    /// Open the try screen on the selected model: a conversation, or the
    /// judge's composer for a model that answers typed questions.
    fn open_chat(&mut self) -> Vec<Effect> {
        let trying = match self.selected_record() {
            Some(record) if judge::is_judge(record) => Ok(record.clone()),
            _ => self.chatting_record(),
        };
        match trying {
            Ok(record) => {
                self.expanded = false;
                let pane = match self.kept_chat.take() {
                    Some(kept) if kept.record.id == record.id => kept,
                    _ => Box::new(ChatPane::open(record).opened(self.motion.now_ms())),
                };
                self.open(Modal::Chat(pane));
                Vec::new()
            }
            Err(refusal) => self.refuse(refusal),
        }
    }

    /// Keys while the chat pane is open: typing, sending, scrolling; escape
    /// or Ctrl-C stops a reply first and closes the pane second.
    fn chat_key(&mut self, key: Key) -> Vec<Effect> {
        let now = self.motion.real_ms();
        let wall = self.motion.wall_ms();
        let Some(Modal::Chat(pane)) = self.modal.as_mut() else {
            return Vec::new();
        };
        match key {
            Key::Escape | Key::Interrupt if pane.streaming() => {
                pane.stop(now);
                self.dirty = true;
                return vec![Effect::StopAsk];
            }
            Key::Escape | Key::Interrupt => {
                // An idle conversation is kept, and so is what was typed and
                // not yet sent, so coming back to the same model takes it up
                // where it was left.
                if let Some(Modal::Chat(pane)) = self.modal.take()
                    && pane.worth_keeping()
                {
                    self.kept_chat = Some(pane);
                }
                self.dirty = true;
            }
            Key::Tab if pane.judging() => pane.next_field(1),
            Key::BackTab if pane.judging() => pane.next_field(-1),
            Key::Tab => {
                pane.suggest();
            }
            Key::Clear => {
                pane.clear();
            }
            Key::BackTab => return Vec::new(),
            Key::Char(_) | Key::Backspace | Key::Edit(_) => {
                pane.edit(key);
                self.dirty = true;
            }
            Key::Up => pane.scroll_up(1),
            Key::Down => pane.scroll_down(1),
            Key::ScrollUp => pane.scroll_up(WHEEL_LINES),
            Key::ScrollDown => pane.scroll_down(WHEEL_LINES),
            Key::PageUp => pane.scroll_up(PAGE_LINES),
            Key::PageDown => pane.scroll_down(PAGE_LINES),
            Key::Top => pane.scroll_to_top(),
            Key::Bottom => pane.scroll_to_bottom(),
            Key::Enter if pane.judging() => {
                let warm = self.facts.is_warm(&pane.record.id);
                let record_id = pane.record.id.clone();
                self.dirty = true;
                return match pane.judge_enter(now, wall, warm) {
                    Judged::Moved => Vec::new(),
                    Judged::Asked(payload, generation) => vec![Effect::Ask {
                        record_id,
                        capability: Capability::judge(),
                        payload,
                        generation,
                    }],
                    Judged::Refused(reason) => self.notify(reason),
                };
            }
            Key::Enter => {
                let warm = self.facts.is_warm(&pane.record.id);
                if let Some((payload, generation)) = pane.submit(now, wall, warm) {
                    self.dirty = true;
                    return vec![Effect::Ask {
                        record_id: pane.record.id.clone(),
                        capability: Capability::chat(),
                        payload,
                        generation,
                    }];
                }
                return Vec::new();
            }
        }
        self.dirty = true;
        Vec::new()
    }

    /// A streamed reply moved; a finished one refreshes, since the model is
    /// warm now.
    fn reply(&mut self, reply: Reply) -> Vec<Effect> {
        let now = self.motion.real_ms();
        let Some(Modal::Chat(pane)) = self.modal.as_mut() else {
            return Vec::new();
        };
        let applied = match reply.step {
            ReplyStep::Text(text) => pane.append(reply.generation, &text, now),
            ReplyStep::Done(stats) => pane.done(reply.generation, stats, now),
            ReplyStep::Failed(reason) => pane.failed(reply.generation, reason, now),
        };
        if !applied {
            return Vec::new();
        }
        let streaming = pane.streaming();
        self.dirty = true;
        if streaming {
            Vec::new()
        } else {
            self.refresh()
        }
    }

    /// Hand off to `hedos serve`, unless a gateway is already up.
    fn serve(&mut self) -> Vec<Effect> {
        match self.facts.gateway_port {
            Some(port) => self.notify(format!("the gateway is already on :{port}")),
            None => vec![Effect::HandOff(Box::new(HandOff::Serve))],
        }
    }

    /// The selected record, cloned for a verb that needs it to chat.
    fn chatting_record(&self) -> Result<ModelRecord, Refusal> {
        let record = self
            .selected_record()
            .ok_or_else(|| Refusal::Because("nothing is selected".to_owned()))?;
        Self::chat_capable(record)?;
        Ok(record.clone())
    }

    /// The UI is back from a hand-off: a fresh shelf and facts stamped with
    /// `sequence` (so a refresh spawned before leaving can't overwrite them),
    /// and a row saying how it went.
    pub fn came_back(&mut self, snapshot: Refreshed, label: TaskLabel, state: TaskState) {
        self.refreshed(snapshot);
        self.tasks.record(label, state, self.ticks);
    }

    /// Open the removal confirmation for the selected model, or say why not.
    fn remove(&mut self) -> Vec<Effect> {
        let Some(record) = self.selected_record() else {
            return Vec::new();
        };
        if let Err(refusal) = self.removable(record) {
            return self.refuse(refusal);
        }
        self.open(Modal::Remove(preview(record)));
        Vec::new()
    }

    /// Why `record` can't be removed right now, if it can't.
    fn removable(&self, record: &ModelRecord) -> Result<(), Refusal> {
        let name = record.display_name();
        if self.tasks.running_on(&record.id) {
            Err(Refusal::Busy)
        } else if self.facts.is_warm(&record.id) {
            Err(Refusal::Because(format!("{name} is warm; unload it first")))
        } else if !is_deletable(record) {
            Err(Refusal::Because(format!(
                "{name} can't be removed from here"
            )))
        } else {
            Ok(())
        }
    }

    /// `y` re-checks everything `x` checked: the shelf and facts refresh
    /// behind the modal while it waits.
    fn remove_key(&mut self, key: Key) -> Vec<Effect> {
        let confirmed = match key {
            Key::Char('y') => true,
            Key::Char('n') | Key::Escape => false,
            _ => return Vec::new(),
        };
        let Some(Modal::Remove(shown)) = self.modal.take() else {
            return Vec::new();
        };
        let model_id = shown.model_id.clone();
        self.dirty = true;
        if !confirmed {
            return Vec::new();
        }
        let Some(record) = self.records.iter().find(|record| record.id == model_id) else {
            return self.notify(format!("{} is no longer on the shelf", shown.name));
        };
        if let Err(refusal) = self.removable(record) {
            return self.refuse(refusal);
        }
        if preview(record) != shown {
            return self.notify(format!("{} changed on disk; look again", shown.name));
        }
        vec![Effect::Spawn(TaskKind::Remove {
            id: model_id,
            name: shown.name,
        })]
    }

    /// Open the stop card over the newest running pull, when its row is on
    /// screen. Nothing stops until the card is answered.
    fn stop_pull(&mut self) -> Vec<Effect> {
        let card = self
            .hint_targets()
            .pull()
            .and_then(|id| self.tasks.row(id))
            .and_then(StopCard::over);
        match card {
            Some(card) => {
                self.open(Modal::Stop(card));
                Vec::new()
            }
            None => self.notify("nothing is downloading".to_owned()),
        }
    }

    /// The card re-checks its pull before acting: it can land, or be stopped
    /// from a terminal, between the poll and the key.
    fn stop_key(&mut self, key: Key) -> Vec<Effect> {
        let Some(choice) = StopCard::choice(key) else {
            return Vec::new();
        };
        let Some(Modal::Stop(mut card)) = self.modal.take() else {
            return Vec::new();
        };
        self.dirty = true;
        let StopChoice::Stop(action) = choice else {
            return Vec::new();
        };
        if !card.follow(self.tasks.pull_row(&card.job)) {
            return self.pull_gone(&card);
        }
        vec![Effect::ControlPull(action, card.job)]
    }

    /// Keep the open stop card on its pull: the figures move under it, and
    /// the pull can end without it.
    fn follow_stop_card(&mut self) {
        let Some(Modal::Stop(card)) = self.modal.as_mut() else {
            return;
        };
        if card.follow(self.tasks.pull_row(&card.job)) {
            return;
        }
        let card = card.clone();
        self.close_modal();
        self.pull_gone(&card);
    }

    /// Say why the card's pull cannot be stopped any more: it landed, or it
    /// stopped without the card.
    fn pull_gone(&mut self, card: &StopCard) -> Vec<Effect> {
        let landed = self
            .tasks
            .pull_row(&card.job)
            .is_some_and(|row| row.pull_state == Some(PullState::Done));
        match landed {
            true => self.notify(format!("{} landed", card.reference)),
            false => self.notify(format!("{} is no longer downloading", card.reference)),
        }
    }

    fn resume_pull(&mut self) -> Vec<Effect> {
        match self.hinted_job(HintTargets::stopped) {
            Some(job) => vec![Effect::ControlPull(PullAction::Resume, job)],
            None => self.notify("no pull is waiting to go on".to_owned()),
        }
    }

    /// The pull job the hint `target` points at, when its row is on screen.
    fn hinted_job(&self, target: impl Fn(&HintTargets) -> Option<TaskId>) -> Option<String> {
        let id = target(&self.hint_targets())?;
        self.tasks.job_of(id).map(str::to_owned)
    }

    /// Put the pulls screen in the shelf's place, on the newest pull still
    /// going the first time, and read the store straight away rather than on
    /// the next cadence.
    fn open_pulls(&mut self) -> Vec<Effect> {
        self.screen = Screen::Pulls;
        self.pulls.entered();
        self.dirty = true;
        self.last_pull_poll = self.ticks;
        let mut effects = vec![Effect::PollPulls];
        effects.extend(self.history_poll());
        effects
    }

    /// Put the shelf back.
    fn close_pulls(&mut self) {
        self.screen = Screen::Shelf;
        self.dirty = true;
    }

    /// Open the bench screen. A bench already going is picked back up rather
    /// than restarted; an empty screen starts one over the whole shelf, since
    /// opening it is the ask.
    fn open_bench(&mut self) -> Vec<Effect> {
        self.screen = Screen::Bench;
        self.dirty = true;
        if self.bench.board().is_none() {
            return self.start_bench(Vec::new(), false);
        }
        Vec::new()
    }

    /// Put the shelf back. The bench itself goes on.
    fn close_bench(&mut self) {
        self.screen = Screen::Shelf;
        self.dirty = true;
    }

    /// Bench the selected model, on the screen the figures land on. The screen
    /// opens only if the bench does: `b` on a model nothing can measure should
    /// say so where the cursor already is.
    fn bench_selected(&mut self) -> Vec<Effect> {
        let Some(record) = self.selected_record() else {
            return self.notify("no model is selected".to_owned());
        };
        let id = record.id.clone();
        let effects = self.start_bench(vec![id], true);
        if !effects.is_empty() {
            self.screen = Screen::Bench;
        }
        effects
    }

    /// Start a bench over `ids`, or over the whole shelf when none are named.
    /// `any_size` benches a model too big for this machine anyway, which is
    /// what naming one is taken to mean.
    fn start_bench(&mut self, ids: Vec<String>, any_size: bool) -> Vec<Effect> {
        if self.bench.running() {
            return self.notify("a bench is already running; c stops it".to_owned());
        }
        let mut rows = bench_rows(&self.records, self.facts.memory_bytes, any_size);
        if !ids.is_empty() {
            rows.retain(|row| ids.contains(&row.id));
        }
        let models: Vec<String> = rows
            .iter()
            .filter(|row| row.status == BenchStatus::Waiting)
            .map(|row| row.id.clone())
            .collect();
        if models.is_empty() {
            return self.notify("nothing here can be benched".to_owned());
        }
        let plan = BenchPlan::new(models);
        self.benches += 1;
        // Measuring one row again keeps the rows it is there to be compared
        // against; a bench over everything starts a fresh table.
        if !ids.is_empty() && self.bench.requeue(&plan.models, self.benches) {
            self.dirty = true;
            return vec![Effect::StartBench(Box::new(plan), self.benches)];
        }
        self.bench.start(
            Board::new(
                rows,
                plan.runs,
                plan.max_tokens,
                machine_line(self.facts.memory_bytes as i64),
            ),
            self.benches,
        );
        self.dirty = true;
        vec![Effect::StartBench(Box::new(plan), self.benches)]
    }

    /// The keys of the bench screen. Its `q` and `?` are the shelf's;
    /// everything else is its own, so no shelf verb fires on a bench row.
    fn bench_key(&mut self, key: Key) -> Vec<Effect> {
        match key {
            Key::Char('q') => return vec![Effect::Quit],
            Key::Escape | Key::Char('B') => self.close_bench(),
            Key::Char('?') => self.open(Modal::Help),
            Key::Char('a') => return self.start_bench(Vec::new(), false),
            Key::Char('b') => return self.bench_again(),
            Key::Char('c') => return self.stop_bench(),
            Key::Down | Key::ScrollDown | Key::Char('j') => return self.move_bench(1),
            Key::Up | Key::ScrollUp | Key::Char('k') => return self.move_bench(-1),
            Key::Top | Key::Char('g') => return self.select_bench(0),
            Key::Bottom | Key::Char('G') => return self.select_bench(usize::MAX),
            _ => {}
        }
        Vec::new()
    }

    /// Measure the selected row again, on its own.
    fn bench_again(&mut self) -> Vec<Effect> {
        let Some(id) = self.bench.selected_row().map(|row| row.id.clone()) else {
            return self.notify("no model is selected".to_owned());
        };
        self.start_bench(vec![id], true)
    }

    /// Ask the bench to stop; what it has measured stands.
    fn stop_bench(&mut self) -> Vec<Effect> {
        if !self.bench.running() {
            return self.notify("no bench is running".to_owned());
        }
        self.dirty = true;
        vec![Effect::StopBench]
    }

    /// Move the bench screen's selection by `delta` rows.
    fn move_bench(&mut self, delta: isize) -> Vec<Effect> {
        if self.bench.step(delta) {
            self.dirty = true;
        }
        Vec::new()
    }

    /// Put the bench screen's selection on `index`, clamped to the last row.
    fn select_bench(&mut self, index: usize) -> Vec<Effect> {
        if self.bench.select(index) {
            self.dirty = true;
        }
        Vec::new()
    }

    /// Open the catalog and search that a pull starts from.
    fn open_pull_modal(&mut self) {
        self.open(Modal::Pull(Box::new(PullModal::open(
            &self.records,
            self.facts.memory_bytes,
            &self.tasks.pulling(),
        ))));
    }

    /// The keys of the pulls screen. The screen's `q`, `?` and `p` are the
    /// shelf's; everything else is its own, so no shelf verb fires by
    /// accident on a pull.
    fn pulls_key(&mut self, key: Key) -> Vec<Effect> {
        match key {
            Key::Char('q') => return vec![Effect::Quit],
            Key::Escape | Key::Char('P') => self.close_pulls(),
            Key::Char('?') => self.open(Modal::Help),
            Key::Char('p') => self.open_pull_modal(),
            Key::Char('x') => return self.forget_selected_pull(),
            Key::Down | Key::ScrollDown | Key::Char('j') => return self.move_pull(1),
            Key::Up | Key::ScrollUp | Key::Char('k') => return self.move_pull(-1),
            Key::Top | Key::Char('g') => return self.select_pull(0),
            Key::Bottom | Key::Char('G') => return self.select_pull(usize::MAX),
            Key::Char('c') => return self.stop_selected_pull(),
            Key::Char('R') => return self.resume_selected_pull(),
            Key::Char('Y') => return self.copy_selected_job(),
            _ => {}
        }
        Vec::new()
    }

    fn move_pull(&mut self, delta: isize) -> Vec<Effect> {
        if !self.pulls.step(delta) {
            return Vec::new();
        }
        self.dirty = true;
        self.history_poll().into_iter().collect()
    }

    fn select_pull(&mut self, index: usize) -> Vec<Effect> {
        if !self.pulls.select(index) {
            return Vec::new();
        }
        self.dirty = true;
        self.history_poll().into_iter().collect()
    }

    /// A read of the selected job's history, when there is one to read.
    fn history_poll(&self) -> Option<Effect> {
        self.pulls.history_wanted().map(Effect::PollHistory)
    }

    /// Open the stop card over the selected pull, or say why not.
    fn stop_selected_pull(&mut self) -> Vec<Effect> {
        let Some(row) = self.pulls.selected_row() else {
            return self.notify("no pull is selected".to_owned());
        };
        match StopCard::over_job(row) {
            Some(card) => {
                self.open(Modal::Stop(card));
                Vec::new()
            }
            // Polled a moment ago, so the seconds left are an upper bound.
            None => match row.status.registering_for_secs(row.polled_at_ms) {
                Some(within_secs) => {
                    let why = WorkerError::PastStopping { within_secs };
                    self.notify(format!("{}: {why}", row.reference))
                }
                None => {
                    let text = format!("{} is {}, not downloading", row.reference, row.pull_state);
                    self.notify(text)
                }
            },
        }
    }

    /// Put the selected pull's job id on the clipboard, which is what a
    /// `hedos pull` command names it by.
    fn copy_selected_job(&mut self) -> Vec<Effect> {
        match self.pulls.selected_row() {
            Some(row) => {
                let id = row.job.clone();
                self.copy(id)
            }
            None => self.notify("no pull is selected".to_owned()),
        }
    }

    /// Take one job's history; the screen shows it only while that job is
    /// the selected one.
    fn history(&mut self, job: String, lines: Vec<String>) {
        if self.pulls.history(job, lines) && self.screen == Screen::Pulls {
            self.dirty = true;
        }
    }

    /// Take the selected pull's record away once it has ended, or say why
    /// not. The rule is the store's; this only spares a round trip for the
    /// answer the screen can already see.
    fn forget_selected_pull(&mut self) -> Vec<Effect> {
        let Some(row) = self.pulls.selected_row() else {
            return self.notify("no pull is selected".to_owned());
        };
        if row.pull_state.is_terminal() {
            return vec![Effect::ControlPull(PullAction::Forget, row.job.clone())];
        }
        let text = format!("{} is {}, not ended", row.reference, row.pull_state);
        self.notify(text)
    }

    /// Put a worker back on the selected pull, or say why not.
    fn resume_selected_pull(&mut self) -> Vec<Effect> {
        let Some(row) = self.pulls.selected_row() else {
            return self.notify("no pull is selected".to_owned());
        };
        if row.pull_state.is_resumable() {
            return vec![Effect::ControlPull(PullAction::Resume, row.job.clone())];
        }
        let text = format!("{} is {}, not stopped", row.reference, row.pull_state);
        self.notify(text)
    }

    fn searched(&mut self, searched: Searched) {
        let Some(Modal::Pull(modal)) = self.modal.as_mut() else {
            return;
        };
        let applied = modal.searched(&searched.query, &searched.hits, searched.note.clone());
        self.dirty = true;
        if applied
            && searched.hits.is_empty()
            && let Some(note) = searched.note
        {
            self.notify(note);
        }
    }

    fn planned(&mut self, planned: Planned) -> Vec<Effect> {
        let Some(Modal::Pull(modal)) = self.modal.as_mut() else {
            return Vec::new();
        };
        let start = modal.planned(
            &planned.provider,
            &planned.reference,
            planned.ask,
            planned.result,
        );
        self.dirty = true;
        match start {
            Some(Ok(plan)) => self.start_pull(plan),
            Some(Err(reason)) => self.notify(reason),
            None => Vec::new(),
        }
    }

    fn warm(&mut self) -> Vec<Effect> {
        let Some(record) = self.selected_record() else {
            return Vec::new();
        };
        if let Err(refusal) = self.warmable(record) {
            return self.refuse(refusal);
        }
        let id = record.id.clone();
        let name = record.display_name().to_owned();
        let kind = match self.facts.gateway_port {
            Some(port) if record.capabilities.contains(&Capability::chat()) => {
                TaskKind::WarmViaGateway { id, name, port }
            }
            _ => TaskKind::Warm { id, name },
        };
        vec![Effect::Spawn(kind)]
    }

    fn unload(&mut self) -> Vec<Effect> {
        let Some(record) = self.selected_record() else {
            return Vec::new();
        };
        if let Err(refusal) = self.unloadable(record) {
            return self.refuse(refusal);
        }
        vec![Effect::Spawn(TaskKind::Unload {
            id: record.id.clone(),
            name: record.display_name().to_owned(),
        })]
    }

    fn notify(&mut self, text: String) -> Vec<Effect> {
        self.notice = Some(Notice {
            text,
            until: self.ticks + NOTICE_TICKS,
            shown_at: self.motion.now_ms(),
        });
        self.dirty = true;
        Vec::new()
    }

    fn refresh(&mut self) -> Vec<Effect> {
        self.last_refresh = self.ticks;
        vec![Effect::Refresh]
    }

    /// Whether the pull screen is waiting on a plan or a search, its
    /// spinner turning.
    fn planning(&self) -> bool {
        matches!(&self.modal, Some(Modal::Pull(modal))
            if modal.planning() || *modal.search() == Search::Asked)
    }

    fn tick(&mut self) -> Vec<Effect> {
        self.ticks += 1;
        let now = self.ticks;
        let mut effects = Vec::new();
        if let Some(Modal::Pull(modal)) = self.modal.as_mut() {
            if let Some(query) = modal.search_due(now) {
                effects.push(Effect::Search(query));
            }
            if let Some((provider, reference, ask)) = modal.plan_due(now) {
                effects.push(Effect::Plan(provider, reference, ask));
                self.dirty = true;
            }
        }
        if self
            .notice
            .as_ref()
            .is_some_and(|notice| now >= notice.until)
        {
            self.notice = None;
            self.dirty = true;
        }
        if self.chat_pane().is_some_and(ChatPane::waiting) || self.planning() {
            self.dirty = true;
        }
        // A bench sends nothing between starting a model and its first token,
        // which on a cold start is the longest part; without a tick of its own
        // the spinner would sit still through exactly the wait it is there for.
        if self.screen == Screen::Bench && self.bench.running() {
            self.dirty = true;
        }
        if self.tasks.expire(now) {
            self.dirty = true;
        }
        // The pulls screen is someone watching, so it reads at the busy
        // cadence whether or not anything is moving.
        let pull_cadence = match self.tasks.any_pulling() || self.screen == Screen::Pulls {
            true => PULL_POLL_TICKS,
            false => PULL_IDLE_POLL_TICKS,
        };
        if now - self.last_pull_poll >= pull_cadence {
            self.last_pull_poll = now;
            effects.push(Effect::PollPulls);
            if self.screen == Screen::Pulls {
                effects.extend(self.history_poll());
            }
        }
        let cadence = if self.busy() {
            BUSY_REFRESH_TICKS
        } else {
            IDLE_REFRESH_TICKS
        };
        if now - self.last_refresh >= cadence {
            effects.extend(self.refresh());
        }
        effects
    }

    fn task(&mut self, event: TaskEvent) -> Vec<Effect> {
        let Some(row) = self.tasks.moved(event, self.ticks) else {
            return Vec::new();
        };
        let finished = !row.running();
        self.dirty = true;
        if finished { self.refresh() } else { Vec::new() }
    }

    /// Fold a poll of the job directory into the strip.
    ///
    /// A model that landed is selected once it reaches the shelf, the same way
    /// a pull started here used to be, whichever process actually fetched it.
    fn pulls_polled(&mut self, rows: Vec<JobRow>) -> Vec<Effect> {
        if self.pulls.sync(&rows) && self.screen == Screen::Pulls {
            self.dirty = true;
        }
        let changes = self.tasks.sync_pulls(rows, self.ticks);
        if !changes.moved {
            return Vec::new();
        }
        self.dirty = true;
        self.follow_stop_card();
        // Two pulls can land in one poll; the newest is the one to select, the
        // same rule a single landing follows.
        match changes.landed.last() {
            Some(reference) => {
                self.select_pulled = Some(reference.clone());
                self.refresh()
            }
            None => Vec::new(),
        }
    }

    fn refreshed(&mut self, refreshed: Refreshed) {
        if refreshed.sequence <= self.applied_refresh {
            return;
        }
        self.applied_refresh = refreshed.sequence;
        let selected_id = self.selected_record().map(|record| record.id.clone());
        self.records = refreshed.records;
        self.facts = refreshed.facts;
        if self.records.is_empty() {
            self.expanded = false;
        }
        // The intent outlives refreshes that predate the record: a cadence
        // refresh can read the shelf before the pull's own scan lands.
        let pulled = self
            .select_pulled
            .as_deref()
            .and_then(|reference| find_installed(&self.records, reference))
            .map(|record| record.id.clone());
        if pulled.is_some() {
            self.select_pulled = None;
        }
        self.reorder(pulled.or(selected_id));
        let pulling = self.tasks.pulling();
        if let Some(Modal::Pull(modal)) = self.modal.as_mut() {
            modal.refresh(&self.records, &pulling);
        }
    }

    fn select(&mut self, index: usize) {
        let last = self.order.len().saturating_sub(1);
        let clamped = index.min(last);
        if clamped != self.selected() {
            self.shelf.select(Some(clamped));
            self.dirty = true;
        }
    }
}

#[cfg(test)]
mod tests;
