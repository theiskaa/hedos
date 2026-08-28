//! The whole UI state and the reducer over it. Pure: it never touches the
//! kernel or the terminal, so every transition is unit-testable.

use std::time::Duration;

use kernel::profiles::FitVerdict;
use kernel::records::{Capability, ModelRecord, ModelState};
use kernel::removal::{ModelDeletionPreview, is_deletable, preview};
use ratatui::widgets::TableState;

use super::chat::ChatPane;
use super::edit::LineEdit;
use super::effect::{Effect, HandOff};
use super::event::{Event, Key, Planned, Refreshed, Reply, ReplyStep, Searched};
use super::facts::Facts;
use super::launch::LaunchModal;
use super::layout;
use super::order::{Sort, order};
use super::pull::{PullModal, Stage, already_downloading};
use super::state::UiState;
use super::strip::TaskStrip;
use super::tasks::{TaskEvent, TaskId, TaskKind, TaskLabel, TaskState};
use crate::support::install::find_installed;
use crate::support::residency::{Holder, warm_request};
use crate::support::shelf_table::verdict;

/// How often the loop ticks; every cadence below is counted in these.
pub(super) const TICK: Duration = Duration::from_millis(250);
pub(super) const TICKS_PER_SECOND: u64 = 1000 / TICK.as_millis() as u64;
/// Refresh cadence while a task runs, and while idle.
const BUSY_REFRESH_TICKS: u64 = 2 * TICKS_PER_SECOND;
const IDLE_REFRESH_TICKS: u64 = 10 * TICKS_PER_SECOND;
/// How far a page key moves the chat transcript, and a wheel notch.
const PAGE_LINES: usize = 10;
const WHEEL_LINES: usize = 3;
/// How long a footer notice stays.
const NOTICE_TICKS: u64 = 2 * TICKS_PER_SECOND;

/// What can sit in front of the shelf.
#[derive(Debug, Clone, PartialEq)]
pub enum Modal {
    /// Choosing a model to download.
    Pull(Box<PullModal>),
    /// Confirming a removal, with what it would delete.
    Remove(ModelDeletionPreview),
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
    /// Whether the detail pane has the whole body.
    pub expanded: bool,
    /// A short message in the footer, until the tick it expires on.
    notice: Option<Notice>,
    /// A reference just pulled; the next refresh selects the record it became.
    select_pulled: Option<String>,
    /// Ticks since the loop started; every cadence is counted in these.
    ticks: u64,
    /// The tick of the last refresh request.
    last_refresh: u64,
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
            expanded: false,
            notice: None,
            select_pulled: None,
            ticks: 0,
            last_refresh: 0,
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
    }

    /// What to remember for the next run.
    pub fn remembered(&self) -> UiState {
        UiState {
            selected_id: self.selected_record().map(|record| record.id.clone()),
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
        } else if verdict(record.footprint_mb, self.facts.memory_bytes)
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
            Event::Task(event) => self.task(event),
            Event::Refreshed(refreshed) => {
                self.refreshed(refreshed);
                Vec::new()
            }
            Event::Searched(searched) => {
                self.searched(searched);
                Vec::new()
            }
            Event::Planned(planned) => {
                self.planned(planned);
                Vec::new()
            }
            Event::Reply(reply) => self.reply(reply),
            Event::InputClosed => vec![Effect::Quit],
        }
    }

    /// The chat pane, while it is open.
    pub fn chat_pane(&self) -> Option<&ChatPane> {
        match &self.modal {
            Some(Modal::Chat(pane)) => Some(pane),
            _ => None,
        }
    }

    /// Ticks since the loop started, for what animates on them.
    pub fn ticks(&self) -> u64 {
        self.ticks
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
            Some(Modal::Help) => {
                if matches!(key, Key::Escape | Key::Char('?') | Key::Char('q')) {
                    self.close_modal();
                }
                return Vec::new();
            }
            Some(Modal::Launch(_)) => return self.launch_key(key),
            None => {}
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
            Key::Char('p') => self.open(Modal::Pull(Box::new(PullModal::open(
                &self.records,
                self.facts.memory_bytes,
                &self.tasks.pulling(),
            )))),
            Key::Char('x') => return self.remove(),
            Key::Char('l') => return self.launch(),
            Key::Char('T') => return self.hand_off_chat(),
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
            Key::Char('c') => return self.cancel_pull(),
            _ => {}
        }
        Vec::new()
    }

    fn pull_key(&mut self, key: Key) -> Vec<Effect> {
        let now = self.ticks;
        let Some(Modal::Pull(modal)) = self.modal.as_mut() else {
            return Vec::new();
        };
        self.dirty = true;
        match (&modal.stage, key) {
            (Stage::Listing, Key::Escape) => self.modal = None,
            (Stage::Listing, Key::Up | Key::ScrollUp) => modal.step(-1),
            (Stage::Listing, Key::Down | Key::ScrollDown) => modal.step(1),
            (Stage::Listing, Key::Char(_) | Key::Backspace | Key::Edit(_)) => {
                modal.edit(key, now);
            }
            (Stage::Listing, Key::Enter) => match modal.choose() {
                Ok((provider, reference, ask)) => {
                    return vec![Effect::Plan(provider, reference, ask)];
                }
                Err(reason) => return self.notify(reason),
            },
            (Stage::Preview(plan), Key::Enter) => {
                let kind = TaskKind::Pull(plan.clone());
                if self.tasks.already_running(&kind) {
                    return self.notify(already_downloading(kind.subject()));
                }
                self.modal = None;
                return vec![Effect::Spawn(kind)];
            }
            (Stage::Preview(_) | Stage::Note(_), Key::Escape | Key::Backspace) => modal.back(),
            (Stage::Planning(_), Key::Escape) => modal.back(),
            _ => {}
        }
        Vec::new()
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

    /// Drop the newest failed task from the strip, when its row is on
    /// screen: the strip is as tall as its rows up to the layout's cap, so a
    /// failure under older rows than that shows no hint and does not go.
    fn dismiss(&mut self) -> Vec<Effect> {
        let visible = self
            .tasks
            .visible_failure(layout::MAX_TASK_ROWS as usize)
            .is_some();
        if visible && self.tasks.dismiss_newest_failure() {
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
        self.dirty = true;
        match key {
            Key::Escape => self.modal = None,
            Key::Up | Key::ScrollUp | Key::Char('k') => modal.step(-1),
            Key::Down | Key::ScrollDown | Key::Char('j') => modal.step(1),
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
                self.modal = None;
                return vec![Effect::HandOff(Box::new(hand_off))];
            }
            _ => {}
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

    /// Open the chat pane on the selected model.
    fn open_chat(&mut self) -> Vec<Effect> {
        match self.chatting_record() {
            Ok(record) => {
                self.expanded = false;
                self.open(Modal::Chat(Box::new(ChatPane::open(record))));
                Vec::new()
            }
            Err(refusal) => self.refuse(refusal),
        }
    }

    /// Keys while the chat pane is open: typing, sending, scrolling; escape
    /// or Ctrl-C stops a reply first and closes the pane second.
    fn chat_key(&mut self, key: Key) -> Vec<Effect> {
        let Some(Modal::Chat(pane)) = self.modal.as_mut() else {
            return Vec::new();
        };
        self.dirty = true;
        match key {
            Key::Escape | Key::Interrupt if pane.streaming() => {
                pane.stop();
                return vec![Effect::StopAsk];
            }
            Key::Escape | Key::Interrupt => self.modal = None,
            Key::Char(_) | Key::Backspace | Key::Edit(_) => pane.edit(key),
            Key::Up => pane.scroll_up(1),
            Key::Down => pane.scroll_down(1),
            Key::ScrollUp => pane.scroll_up(WHEEL_LINES),
            Key::ScrollDown => pane.scroll_down(WHEEL_LINES),
            Key::PageUp => pane.scroll_up(PAGE_LINES),
            Key::PageDown => pane.scroll_down(PAGE_LINES),
            Key::Top => pane.scroll_to_top(),
            Key::Bottom => pane.scroll_to_bottom(),
            Key::Enter => {
                if let Some((payload, generation)) = pane.submit() {
                    return vec![Effect::Ask {
                        record_id: pane.record.id.clone(),
                        payload,
                        generation,
                    }];
                }
            }
        }
        Vec::new()
    }

    /// A streamed reply moved; a finished one refreshes, since the model is
    /// warm now.
    fn reply(&mut self, reply: Reply) -> Vec<Effect> {
        let Some(Modal::Chat(pane)) = self.modal.as_mut() else {
            return Vec::new();
        };
        let applied = match reply.step {
            ReplyStep::Text(text) => pane.append(reply.generation, &text),
            ReplyStep::Done(stats) => pane.done(reply.generation, stats),
            ReplyStep::Failed(reason) => pane.failed(reply.generation, reason),
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

    /// Cancel the newest pull still downloading.
    fn cancel_pull(&mut self) -> Vec<Effect> {
        match self.tasks.newest_running_pull() {
            Some(id) => vec![Effect::Cancel(id)],
            None => self.notify("nothing is downloading".to_owned()),
        }
    }

    fn searched(&mut self, searched: Searched) {
        let Some(Modal::Pull(modal)) = self.modal.as_mut() else {
            return;
        };
        let applied = modal.searched(&searched.query, &searched.hits);
        self.dirty = true;
        if applied
            && searched.hits.is_empty()
            && let Some(note) = searched.note
        {
            self.notify(note);
        }
    }

    fn planned(&mut self, planned: Planned) {
        if let Some(Modal::Pull(modal)) = self.modal.as_mut() {
            modal.planned(planned.ask, planned.result);
            self.dirty = true;
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
        });
        self.dirty = true;
        Vec::new()
    }

    fn refresh(&mut self) -> Vec<Effect> {
        self.last_refresh = self.ticks;
        vec![Effect::Refresh]
    }

    /// Whether the pull modal is waiting on a plan, its spinner turning.
    fn planning(&self) -> bool {
        matches!(&self.modal, Some(Modal::Pull(modal)) if matches!(modal.stage, Stage::Planning(_)))
    }

    fn tick(&mut self) -> Vec<Effect> {
        self.ticks += 1;
        let now = self.ticks;
        let mut effects = Vec::new();
        if let Some(Modal::Pull(modal)) = self.modal.as_mut()
            && let Some(query) = modal.search_due(now)
        {
            effects.push(Effect::Search(query));
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
        if self.tasks.expire(now) {
            self.dirty = true;
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
        if let (Some(TaskKind::Pull(plan)), TaskState::Done(_)) = (&row.kind, &row.state) {
            self.select_pulled = Some(plan.reference.clone());
        }
        self.dirty = true;
        if finished { self.refresh() } else { Vec::new() }
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
mod tests {
    use super::*;
    use crate::tui::keymap;
    use crate::tui::strip::{DONE_LINGER_TICKS, FAILED_LINGER_TICKS};
    use crate::tui::testing::{plan, resident};
    use kernel::records::{Capability, Modality, ModelSource, SourceKind};

    fn record(index: usize) -> ModelRecord {
        crate::tui::testing::record(&format!("model-{index}"))
    }

    fn app(count: usize) -> App {
        app_from((0..count).map(record).collect())
    }

    fn app_from(records: Vec<ModelRecord>) -> App {
        App::new(records, Facts::default())
    }

    fn press(app: &mut App, key: Key) -> Vec<Effect> {
        app.reduce(Event::Key(key))
    }

    fn pull(app: &App) -> &PullModal {
        match &app.modal {
            Some(Modal::Pull(modal)) => modal,
            _ => panic!("no pull modal"),
        }
    }

    fn ticks(app: &mut App, count: u64) -> Vec<Effect> {
        (0..count).flat_map(|_| app.reduce(Event::Tick)).collect()
    }

    #[test]
    fn movement_clamps_at_both_ends() {
        let mut app = app(3);
        press(&mut app, Key::Up);
        assert_eq!(app.selected(), 0);
        for _ in 0..5 {
            press(&mut app, Key::Char('j'));
        }
        assert_eq!(app.selected(), 2);
    }

    #[test]
    fn top_and_bottom_jump() {
        let mut app = app(4);
        press(&mut app, Key::Char('G'));
        assert_eq!(app.selected(), 3);
        press(&mut app, Key::Char('g'));
        assert_eq!(app.selected(), 0);
    }

    #[test]
    fn selected_is_matches_by_the_pull_modal_rule() {
        let mut app = app(2);
        press(&mut app, Key::Down);
        assert_eq!(
            app.selected_record().map(|r| r.name.as_str()),
            Some("model-1")
        );
        assert!(app.selected_is("model-1"));
        assert!(app.selected_is("owner/model-1"));
        assert!(app.selected_is("MODEL-1"));
        assert!(!app.selected_is("model-0"));
        assert!(!app.selected_is("model-2"));
    }

    #[test]
    fn an_empty_shelf_never_selects() {
        let mut app = app(0);
        press(&mut app, Key::Down);
        assert_eq!(app.selected(), 0);
        assert!(app.selected_record().is_none());
        assert!(press(&mut app, Key::Char('w')).is_empty());
    }

    #[test]
    fn quit_keys_yield_quit() {
        let mut app = app(1);
        assert_eq!(press(&mut app, Key::Char('q')), vec![Effect::Quit]);
        assert_eq!(press(&mut app, Key::Interrupt), vec![Effect::Quit]);
    }

    #[test]
    fn only_changes_mark_the_screen_dirty() {
        let mut app = app(2);
        assert!(app.take_dirty());
        press(&mut app, Key::Up);
        assert!(!app.take_dirty());
        press(&mut app, Key::Down);
        assert!(app.take_dirty());
        app.reduce(Event::Resize);
        assert!(app.take_dirty());
    }

    #[test]
    fn scan_and_refresh_are_effects() {
        let mut app = app(1);
        assert_eq!(
            press(&mut app, Key::Char('s')),
            vec![Effect::Spawn(TaskKind::Scan)]
        );
        assert_eq!(press(&mut app, Key::Char('r')), vec![Effect::Refresh]);
    }

    #[test]
    fn warm_goes_local_without_a_gateway_and_through_it_with_one() {
        let mut app = app(1);
        let id = app.records[0].id.clone();
        assert_eq!(
            press(&mut app, Key::Char('w')),
            vec![Effect::Spawn(TaskKind::Warm {
                id,
                name: "model-0".to_owned()
            })]
        );
        app.facts.gateway_port = Some(4321);
        assert!(matches!(
            press(&mut app, Key::Char('w')).as_slice(),
            [Effect::Spawn(TaskKind::WarmViaGateway { port: 4321, .. })]
        ));
        app.started(
            TaskId::next(),
            TaskKind::WarmViaGateway {
                id: app.records[0].id.clone(),
                name: "model-0".to_owned(),
                port: 4321,
            },
        );
        assert!(press(&mut app, Key::Char('w')).is_empty());
    }

    #[test]
    fn warming_a_warm_model_only_notifies() {
        let mut app = app(1);
        app.facts
            .residents
            .push(resident(&app.records[0].id, Holder::Local));
        assert!(press(&mut app, Key::Char('w')).is_empty());
        assert_eq!(app.notice(), Some("model-0 is already warm"));
        ticks(&mut app, NOTICE_TICKS);
        assert_eq!(app.notice(), None);
    }

    #[test]
    fn unload_needs_a_locally_warm_model() {
        let mut app = app(1);
        let id = app.records[0].id.clone();
        assert!(press(&mut app, Key::Char('u')).is_empty());
        assert_eq!(app.notice(), Some("model-0 is not warm"));

        app.facts.residents.push(resident(&id, Holder::Gateway));
        assert!(press(&mut app, Key::Char('u')).is_empty());
        assert!(app.notice().unwrap().contains("held by the gateway"));

        app.facts.residents[0].holder = Holder::Local;
        assert_eq!(
            press(&mut app, Key::Char('u')),
            vec![Effect::Spawn(TaskKind::Unload {
                id,
                name: "model-0".to_owned()
            })]
        );
    }

    #[test]
    fn a_running_task_blocks_a_duplicate_on_the_same_model() {
        let mut app = app(1);
        let id = app.records[0].id.clone();
        app.started(
            TaskId::next(),
            TaskKind::Warm {
                id,
                name: "model-0".to_owned(),
            },
        );
        assert!(press(&mut app, Key::Char('w')).is_empty());
        assert!(app.busy());
    }

    #[test]
    fn finished_tasks_request_a_refresh_and_results_fade() {
        let mut app = app(1);
        let id = TaskId::next();
        app.started(id, TaskKind::Scan);
        app.take_dirty();
        let effects = app.reduce(Event::Task(TaskEvent {
            id,
            state: TaskState::Done("found 2".to_owned()),
        }));
        assert_eq!(effects, vec![Effect::Refresh]);
        assert!(app.take_dirty());
        assert_eq!(app.tasks.rows().len(), 1);
        ticks(&mut app, DONE_LINGER_TICKS + 1);
        assert!(app.tasks.rows().is_empty());
    }

    #[test]
    fn failures_stay_much_longer_than_results() {
        let mut app = app(1);
        let id = TaskId::next();
        app.started(id, TaskKind::Scan);
        app.reduce(Event::Task(TaskEvent {
            id,
            state: TaskState::Failed("no".to_owned()),
        }));
        ticks(&mut app, DONE_LINGER_TICKS * 2);
        assert_eq!(app.tasks.rows().len(), 1);
        ticks(&mut app, FAILED_LINGER_TICKS);
        assert!(app.tasks.rows().is_empty());
    }

    #[test]
    fn idle_refresh_is_slower_than_busy_refresh() {
        let mut app = app(1);
        let idle = ticks(&mut app, IDLE_REFRESH_TICKS);
        assert_eq!(idle, vec![Effect::Refresh]);
        app.started(TaskId::next(), TaskKind::Scan);
        let busy = ticks(&mut app, BUSY_REFRESH_TICKS);
        assert_eq!(busy, vec![Effect::Refresh]);
    }

    #[test]
    fn a_refresh_keeps_the_selected_model() {
        let mut app = app(3);
        press(&mut app, Key::Char('G'));
        let kept = app.records[2].clone();
        app.reduce(Event::Refreshed(Refreshed {
            sequence: 2,
            records: vec![kept.clone(), record(9)],
            facts: Facts::default(),
        }));
        assert_eq!(app.selected_record().map(|r| &r.id), Some(&kept.id));

        app.reduce(Event::Refreshed(Refreshed {
            sequence: 3,
            records: Vec::new(),
            facts: Facts::default(),
        }));
        assert!(app.selected_record().is_none());
    }

    #[test]
    fn the_pull_modal_captures_keys_until_it_closes() {
        let mut app = app(1);
        press(&mut app, Key::Char('p'));
        assert!(app.modal.is_some());
        assert!(press(&mut app, Key::Char('q')).is_empty());
        assert_eq!(pull(&app).input.as_str(), "q");
        press(&mut app, Key::Backspace);
        let effects = press(&mut app, Key::Enter);
        assert!(matches!(effects.as_slice(), [Effect::Plan(_, _, _)]));
        press(&mut app, Key::Escape);
        assert_eq!(pull(&app).stage, Stage::Listing);
        press(&mut app, Key::Escape);
        assert!(app.modal.is_none());
        assert_eq!(press(&mut app, Key::Interrupt), vec![Effect::Quit]);
    }

    #[test]
    fn ticks_turn_the_planning_spinner_and_nothing_else() {
        let mut app = app(1);
        press(&mut app, Key::Char('p'));
        app.take_dirty();
        assert!(ticks(&mut app, 1).is_empty());
        assert!(!app.take_dirty());
        press(&mut app, Key::Enter);
        assert!(matches!(pull(&app).stage, Stage::Planning(_)));
        app.take_dirty();
        assert!(ticks(&mut app, 1).is_empty());
        assert!(app.take_dirty());
        press(&mut app, Key::Escape);
        app.take_dirty();
        ticks(&mut app, 1);
        assert!(!app.take_dirty());
    }

    #[test]
    fn a_typed_query_is_searched_after_the_debounce() {
        let mut app = app(1);
        press(&mut app, Key::Char('p'));
        press(&mut app, Key::Char('x'));
        assert!(ticks(&mut app, 1).is_empty());
        assert_eq!(ticks(&mut app, 1), vec![Effect::Search("x".to_owned())]);
    }

    #[test]
    fn an_abandoned_partial_download_can_be_removed() {
        // `downloading` is discovery's reading of incomplete blobs on disk, not
        // a live pull; with no task running there is nothing to cancel, so
        // removal is the only way out.
        let mut partial = record(0);
        partial.downloading = true;
        let mut app = app_from(vec![partial]);
        assert!(press(&mut app, Key::Char('x')).is_empty());
        assert!(app.notice().is_none());
        assert!(matches!(app.modal, Some(Modal::Remove(_))));
    }

    #[test]
    fn cancel_targets_the_newest_running_pull() {
        let mut app = app(1);
        assert!(press(&mut app, Key::Char('c')).is_empty());
        assert_eq!(app.notice(), Some("nothing is downloading"));
        let id = TaskId::next();
        let plan = plan("x");
        app.started(id, TaskKind::Pull(plan.clone()));
        assert_eq!(press(&mut app, Key::Char('c')), vec![Effect::Cancel(id)]);

        let mut modal = PullModal::open(&[], 0, &[]);
        let mut replanned = plan;
        replanned.remaining_bytes = Some(5);
        modal.stage = Stage::Preview(replanned);
        app.modal = Some(Modal::Pull(Box::new(modal)));
        assert!(press(&mut app, Key::Enter).is_empty());
        assert_eq!(app.notice(), Some("x is already downloading"));
    }

    #[test]
    fn remove_asks_first_and_refuses_warm_models() {
        let mut app = app(1);
        let id = app.records[0].id.clone();
        app.facts.residents.push(resident(&id, Holder::Local));
        assert!(press(&mut app, Key::Char('x')).is_empty());
        assert_eq!(app.notice(), Some("model-0 is warm; unload it first"));
        app.facts.residents.clear();

        press(&mut app, Key::Char('x'));
        assert!(matches!(app.modal, Some(Modal::Remove(_))));
        assert!(press(&mut app, Key::Char('n')).is_empty());
        assert!(app.modal.is_none());

        press(&mut app, Key::Char('x'));
        assert_eq!(
            press(&mut app, Key::Char('y')),
            vec![Effect::Spawn(TaskKind::Remove {
                id: id.clone(),
                name: "model-0".to_owned()
            })]
        );
        assert!(app.modal.is_none());

        press(&mut app, Key::Char('x'));
        app.facts.residents.push(resident(&id, Holder::Gateway));
        assert!(press(&mut app, Key::Char('y')).is_empty());
        assert_eq!(app.notice(), Some("model-0 is warm; unload it first"));
        assert!(app.modal.is_none());
    }

    #[test]
    fn enter_expands_the_detail_and_escape_folds_it() {
        let mut one = app(1);
        press(&mut one, Key::Enter);
        assert!(one.expanded);
        press(&mut one, Key::Escape);
        assert!(!one.expanded);
        let mut empty = app(0);
        press(&mut empty, Key::Enter);
        assert!(!empty.expanded);
        press(&mut one, Key::Enter);
        one.reduce(Event::Refreshed(Refreshed {
            sequence: 9,
            records: Vec::new(),
            facts: Facts::default(),
        }));
        assert!(!one.expanded);
    }

    #[test]
    fn the_filter_narrows_the_shelf_and_escape_clears_it() {
        let mut app = app(3);
        press(&mut app, Key::Char('/'));
        assert!(app.filtering);
        press(&mut app, Key::Char('2'));
        assert_eq!(app.order, vec![2]);
        assert_eq!(
            app.selected_record().map(|r| r.name.as_str()),
            Some("model-2")
        );
        press(&mut app, Key::Enter);
        assert!(!app.filtering);
        assert_eq!(app.order.len(), 1);
        press(&mut app, Key::Escape);
        assert_eq!(app.order.len(), 3);
        assert_eq!(
            app.selected_record().map(|r| r.name.as_str()),
            Some("model-2")
        );
    }

    #[test]
    fn sort_cycles_and_keeps_the_selection() {
        let mut app = app(3);
        app.records[0].footprint_mb = Some(1);
        app.records[2].footprint_mb = Some(9);
        press(&mut app, Key::Char('o'));
        assert_eq!(app.sort, Sort::Size);
        assert_eq!(app.order[0], 2);
        assert_eq!(
            app.selected_record().map(|r| r.name.as_str()),
            Some("model-0")
        );
    }

    #[test]
    fn copy_yields_the_path_or_a_notice() {
        let mut app = app(1);
        assert!(press(&mut app, Key::Char('y')).is_empty());
        assert_eq!(app.notice(), Some("model-0 has no path"));
        app.records[0].primary_weight_path = Some("/w".to_owned());
        assert_eq!(
            press(&mut app, Key::Char('y')),
            vec![Effect::Copy("/w".to_owned())]
        );
        let id = app.records[0].id.clone();
        assert_eq!(press(&mut app, Key::Char('Y')), vec![Effect::Copy(id)]);
    }

    #[test]
    fn dismiss_drops_the_newest_failure() {
        let mut app = app(1);
        let id = TaskId::next();
        app.started(id, TaskKind::Scan);
        app.reduce(Event::Task(TaskEvent {
            id,
            state: TaskState::Failed("no".to_owned()),
        }));
        press(&mut app, Key::Char('d'));
        assert!(app.tasks.rows().is_empty());
        press(&mut app, Key::Char('d'));
        assert_eq!(app.notice(), Some("nothing to dismiss"));
    }

    #[test]
    fn dismiss_leaves_a_failure_the_strip_does_not_show() {
        let mut app = app(1);
        let failed = TaskId::next();
        app.started(failed, TaskKind::Scan);
        app.reduce(Event::Task(TaskEvent {
            id: failed,
            state: TaskState::Failed("no".to_owned()),
        }));
        for _ in 0..layout::MAX_TASK_ROWS {
            let id = TaskId::next();
            app.started(id, TaskKind::Scan);
            app.reduce(Event::Task(TaskEvent {
                id,
                state: TaskState::Done("ok".to_owned()),
            }));
        }
        app.notice = None;
        press(&mut app, Key::Char('d'));
        assert_eq!(app.notice(), Some("nothing to dismiss"));
        assert_eq!(app.tasks.rows().len(), 1 + layout::MAX_TASK_ROWS as usize);
        assert_eq!(app.tasks.newest_failure(), Some(failed));
    }

    #[test]
    fn state_round_trips_through_restore() {
        let mut app = app(3);
        press(&mut app, Key::Char('G'));
        let state = app.remembered();
        let mut fresh = app_from(app.records.clone());
        fresh.restore(&state);
        assert_eq!(
            fresh.selected_record().map(|r| &r.id),
            Some(&app.records[2].id)
        );
    }

    #[test]
    fn help_closes_on_escape_question_mark_or_q_only() {
        for closer in [Key::Escape, Key::Char('?'), Key::Char('q')] {
            let mut app = app(3);
            press(&mut app, Key::Down);
            assert_eq!(app.selected(), 1);
            press(&mut app, Key::Char('?'));
            assert_eq!(app.modal, Some(Modal::Help));
            assert!(press(&mut app, Key::Char('j')).is_empty());
            assert_eq!(app.modal, Some(Modal::Help));
            assert_eq!(app.selected(), 1);
            assert!(press(&mut app, closer).is_empty());
            assert!(app.modal.is_none());
        }
    }

    #[test]
    fn launch_offers_harnesses_and_hands_off_on_an_allowed_one() {
        let mut app = app(1);
        press(&mut app, Key::Char('l'));
        assert!(matches!(app.modal, Some(Modal::Launch(_))));
        press(&mut app, Key::Escape);
        assert!(app.modal.is_none());
        let record = record(0);
        app.open(Modal::Launch(Box::new(LaunchModal::open_with(
            &record,
            |_| Some(std::path::PathBuf::from("/bin/harness")),
        ))));
        let effects = press(&mut app, Key::Enter);
        assert!(matches!(
            effects.as_slice(),
            [Effect::HandOff(hand_off)] if matches!(**hand_off, HandOff::Launch { .. })
        ));
        assert!(app.modal.is_none());
        app.open(Modal::Launch(Box::new(LaunchModal::open_with(
            &record,
            |_| None,
        ))));
        assert!(press(&mut app, Key::Enter).is_empty());
        assert!(
            app.notice()
                .is_some_and(|notice| notice.contains("not installed"))
        );
    }

    #[test]
    fn the_scroll_keys_reach_the_pane() {
        let mut app = app(1);
        let generation = ask(&mut app);
        reply(&mut app, generation, ReplyStep::Text("a\n".repeat(40)));
        app.chat_pane_mut().expect("the pane").measured(30);
        press(&mut app, Key::PageUp);
        assert_eq!(
            app.chat_pane().expect("the pane").first_line(),
            30 - PAGE_LINES
        );
        press(&mut app, Key::ScrollDown);
        assert_eq!(
            app.chat_pane().expect("the pane").first_line(),
            30 - PAGE_LINES + WHEEL_LINES
        );
        press(&mut app, Key::Bottom);
        assert_eq!(app.chat_pane().expect("the pane").first_line(), 30);
    }

    #[test]
    fn launch_is_refused_for_a_model_that_cannot_chat() {
        let mut app = app(1);
        app.records[0].capabilities = vec![Capability::speak()];
        app.reorder_in_place();
        assert!(press(&mut app, Key::Char('l')).is_empty());
        assert_eq!(
            app.notice(),
            Some("model-0 can't chat, so no harness can use it")
        );
    }

    #[test]
    fn coming_back_adds_a_finished_row_and_keeps_the_selection() {
        let mut app = app(3);
        press(&mut app, Key::Char('G'));
        let kept = app.records[2].clone();
        app.came_back(
            Refreshed {
                sequence: 5,
                records: vec![kept.clone(), record(7)],
                facts: Facts::default(),
            },
            TaskLabel {
                verb: "launch",
                subject: "x".to_owned(),
            },
            TaskState::Done("ran 4m".to_owned()),
        );
        assert_eq!(app.selected_record().map(|r| &r.id), Some(&kept.id));
        assert_eq!(app.tasks.rows().len(), 1);
        assert!(!app.busy());
        // A refresh that was in flight before leaving is older and must lose.
        app.reduce(Event::Refreshed(Refreshed {
            sequence: 4,
            records: Vec::new(),
            facts: Facts::default(),
        }));
        assert_eq!(app.records.len(), 2);
        // A refresh spawned after coming back still applies.
        app.reduce(Event::Refreshed(Refreshed {
            sequence: 6,
            records: vec![record(1)],
            facts: Facts::default(),
        }));
        assert_eq!(app.records.len(), 1);
    }

    /// Open the pane on the first model, type `hi`, send it; the ask's number.
    fn ask(app: &mut App) -> u64 {
        press(app, Key::Char('t'));
        for c in "hi".chars() {
            press(app, Key::Char(c));
        }
        match press(app, Key::Enter).as_slice() {
            [Effect::Ask { generation, .. }] => *generation,
            other => panic!("expected an ask, got {other:?}"),
        }
    }

    fn reply(app: &mut App, generation: u64, step: ReplyStep) -> Vec<Effect> {
        app.reduce(Event::Reply(Reply { generation, step }))
    }

    #[test]
    fn try_opens_the_chat_pane_and_enter_asks() {
        let mut app = app(1);
        press(&mut app, Key::Char('t'));
        assert!(matches!(app.modal, Some(Modal::Chat(_))));
        assert!(press(&mut app, Key::Enter).is_empty());
        press(&mut app, Key::Escape);
        assert!(ask(&mut app) > 0);
        assert!(press(&mut app, Key::Char('q')).is_empty());
        assert!(matches!(app.modal, Some(Modal::Chat(_))));
    }

    #[test]
    fn a_streamed_reply_lands_in_the_pane_and_refreshes_when_done() {
        let mut app = app(1);
        let generation = ask(&mut app);
        app.take_dirty();
        let effects = reply(&mut app, generation, ReplyStep::Text("yo".to_owned()));
        assert!(effects.is_empty() && app.take_dirty());
        let effects = reply(&mut app, generation, ReplyStep::Done(None));
        assert_eq!(effects, vec![Effect::Refresh]);
        let pane = app.chat_pane().expect("the pane");
        assert_eq!(pane.turns.last().map(|turn| turn.text.as_str()), Some("yo"));
        assert!(!pane.streaming());
    }

    #[test]
    fn escape_stops_a_reply_first_and_closes_the_pane_second() {
        let mut app = app(1);
        let generation = ask(&mut app);
        assert_eq!(press(&mut app, Key::Escape), vec![Effect::StopAsk]);
        app.take_dirty();
        let effects = reply(&mut app, generation, ReplyStep::Text("late".to_owned()));
        assert!(effects.is_empty() && !app.take_dirty());
        assert!(press(&mut app, Key::Escape).is_empty());
        assert!(app.modal.is_none());
        assert_eq!(press(&mut app, Key::Interrupt), vec![Effect::Quit]);
    }

    #[test]
    fn ctrl_c_closes_an_idle_chat_pane_and_quits_from_every_other_modal() {
        let mut app = app(1);
        press(&mut app, Key::Char('t'));
        assert!(press(&mut app, Key::Interrupt).is_empty());
        assert!(app.modal.is_none());
        for open in ['p', 'x', 'l', '?'] {
            press(&mut app, Key::Char(open));
            assert!(app.modal.is_some(), "{open} opens a modal");
            assert_eq!(press(&mut app, Key::Interrupt), vec![Effect::Quit]);
            app.modal = None;
        }
    }

    #[test]
    fn a_reopened_pane_never_takes_the_closed_ones_reply() {
        let mut app = app(1);
        let first = ask(&mut app);
        press(&mut app, Key::Escape);
        press(&mut app, Key::Escape);
        let second = ask(&mut app);
        assert!(second > first);
        reply(&mut app, first, ReplyStep::Text("stale".to_owned()));
        let pane = app.chat_pane().expect("the pane");
        assert_eq!(pane.turns.last().map(|turn| turn.text.as_str()), Some(""));
    }

    #[test]
    fn chat_and_serve_hand_off_when_they_can() {
        let mut app = app(1);
        assert!(matches!(
            press(&mut app, Key::Char('T')).as_slice(),
            [Effect::HandOff(hand_off)] if matches!(**hand_off, HandOff::Chat { .. })
        ));
        assert!(matches!(
            press(&mut app, Key::Char('S')).as_slice(),
            [Effect::HandOff(hand_off)] if matches!(**hand_off, HandOff::Serve)
        ));
        app.facts.gateway_port = Some(4321);
        assert!(press(&mut app, Key::Char('S')).is_empty());
        assert_eq!(app.notice(), Some("the gateway is already on :4321"));
    }

    #[test]
    fn a_finished_pull_selects_what_it_pulled_on_the_next_refresh() {
        let mut app = app(2);
        let id = TaskId::next();
        let plan = plan("qwen2.5:14b");
        app.started(id, TaskKind::Pull(plan));
        let effects = app.reduce(Event::Task(TaskEvent {
            id,
            state: TaskState::Done("pulled".to_owned()),
        }));
        assert_eq!(effects, vec![Effect::Refresh]);
        // A refresh that predates the pulled record leaves the intent alone.
        app.reduce(Event::Refreshed(Refreshed {
            sequence: 8,
            records: app.records.clone(),
            facts: Facts::default(),
        }));
        let mut records = app.records.clone();
        records.push(ModelRecord::new(
            "qwen2.5:14b",
            Modality::text(),
            vec![Capability::chat()],
            ModelSource::new(SourceKind::ollama(), "qwen2.5:14b"),
        ));
        app.reduce(Event::Refreshed(Refreshed {
            sequence: 9,
            records,
            facts: Facts::default(),
        }));
        assert_eq!(
            app.selected_record().map(|r| r.name.as_str()),
            Some("qwen2.5:14b")
        );
    }

    #[test]
    fn actions_follow_the_selected_model() {
        let mut one = app(1);
        let id = one.records[0].id.clone();
        assert_eq!(one.actions(), vec!["w", "l", "t", "T", "x"]);
        one.facts.residents.push(resident(&id, Holder::Daemon));
        assert_eq!(one.actions(), vec!["u", "l", "t", "T"]);
        one.facts.residents[0].holder = Holder::Gateway;
        assert_eq!(one.actions(), vec!["l", "t", "T"]);
        one.facts.residents.clear();
        one.records[0].capabilities = vec![Capability::speak()];
        one.records[0].primary_weight_path = Some("/w".to_owned());
        one.reorder_in_place();
        assert_eq!(one.actions(), vec!["w", "x", "y"]);
        for key in one.actions() {
            assert!(keymap::binding(key).is_some(), "{key} is not bound");
        }
        let empty = app(0);
        assert!(empty.actions().is_empty());
    }

    /// The keys a binding names, as the reducer receives them.
    fn keys_of(binding: &keymap::Binding) -> Vec<Key> {
        match binding.key {
            "enter" => vec![Key::Enter],
            "esc" => vec![Key::Escape],
            "↑/↓" => vec![Key::Up, Key::Down],
            key => keymap::chars(key).into_iter().map(Key::Char).collect(),
        }
    }

    #[test]
    fn every_binding_does_something() {
        for binding in keymap::BINDINGS
            .iter()
            .filter(|binding| binding.group != keymap::Group::Screen)
        {
            for key in keys_of(binding) {
                // Selected in the middle of three, so each move key has
                // somewhere to go; expanded, so escape has something to
                // collapse.
                let mut app = app(3);
                press(&mut app, Key::Down);
                if binding.key == "esc" {
                    press(&mut app, Key::Enter);
                }
                app.take_dirty();
                let effects = press(&mut app, key);
                assert!(
                    !effects.is_empty() || app.take_dirty() || app.notice().is_some(),
                    "{} ({key:?}) does nothing",
                    binding.key
                );
            }
        }
    }

    #[test]
    fn every_unbound_char_does_nothing() {
        let bound: Vec<char> = keymap::BINDINGS
            .iter()
            .flat_map(|binding| keymap::chars(binding.key))
            .collect();
        for c in (0x20u8..=0x7e)
            .map(char::from)
            .filter(|c| !bound.contains(c))
        {
            let mut app = app(3);
            press(&mut app, Key::Down);
            app.take_dirty();
            let effects = press(&mut app, Key::Char(c));
            assert!(effects.is_empty(), "{c:?} has effects but is not bound");
            assert!(!app.take_dirty(), "{c:?} redraws but is not bound");
            assert!(app.notice().is_none(), "{c:?} notifies but is not bound");
        }
    }

    #[test]
    fn an_older_refresh_never_overwrites_a_newer_one() {
        let mut app = app(1);
        app.reduce(Event::Refreshed(Refreshed {
            sequence: 5,
            records: vec![record(5)],
            facts: Facts::default(),
        }));
        app.reduce(Event::Refreshed(Refreshed {
            sequence: 4,
            records: vec![record(4)],
            facts: Facts::default(),
        }));
        assert_eq!(app.records[0].name, "model-5");
    }
}
