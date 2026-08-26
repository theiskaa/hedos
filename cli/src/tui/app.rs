//! The whole UI state and the reducer over it. Pure: it never touches the
//! kernel or the terminal, so every transition is unit-testable.

use std::time::Duration;

use kernel::records::{Capability, ModelRecord};
use kernel::removal::{ModelDeletionPreview, is_deletable, preview};
use ratatui::widgets::TableState;

use super::effect::Effect;
use super::event::{Event, Key, Planned, Refreshed, Searched};
use super::facts::Facts;
use super::order::{Sort, order};
use super::pull::{PullModal, Stage};
use super::state::UiState;
use super::tasks::{TaskEvent, TaskId, TaskKind, TaskState};
use crate::support::residency::Holder;

/// How often the loop ticks; every cadence below is counted in these.
pub(super) const TICK: Duration = Duration::from_millis(250);
const TICKS_PER_SECOND: u64 = 1000 / TICK.as_millis() as u64;
/// Refresh cadence while a task runs, and while idle.
const BUSY_REFRESH_TICKS: u64 = 2 * TICKS_PER_SECOND;
const IDLE_REFRESH_TICKS: u64 = 10 * TICKS_PER_SECOND;
/// How long a finished task stays in the strip, and how long a failed one does.
const DONE_LINGER_TICKS: u64 = 60 * TICKS_PER_SECOND;
const FAILED_LINGER_TICKS: u64 = 10 * 60 * TICKS_PER_SECOND;
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
}

/// A task as the strip shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRow {
    pub id: TaskId,
    pub kind: TaskKind,
    pub state: TaskState,
    /// The tick the task finished on, for expiry.
    finished_at: Option<u64>,
}

impl TaskRow {
    /// Whether the task is still going.
    pub fn running(&self) -> bool {
        self.state.running()
    }
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
    pub filter: String,
    /// Whether keys are typing into the filter.
    pub filtering: bool,
    /// The sort in effect.
    pub sort: Sort,
    /// Background work, oldest first.
    pub tasks: Vec<TaskRow>,
    /// The modal in front of the shelf, while one is open.
    pub modal: Option<Modal>,
    /// Whether the detail pane has the whole body.
    pub expanded: bool,
    /// A short message in the footer, until the tick it expires on.
    notice: Option<(String, u64)>,
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
            filter: String::new(),
            filtering: false,
            sort: Sort::default(),
            tasks: Vec::new(),
            modal: None,
            expanded: false,
            notice: None,
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
        self.order = order(&self.records, &self.facts, &self.filter, self.sort);
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

    /// The footer notice, if one is showing.
    pub fn notice(&self) -> Option<&str> {
        self.notice.as_ref().map(|(text, _)| text.as_str())
    }

    /// Whether any task is still running.
    pub fn busy(&self) -> bool {
        self.tasks.iter().any(TaskRow::running)
    }

    /// Whether something changed since the last draw; reading it clears it.
    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    /// Record that the loop started `kind` as task `id`.
    pub fn started(&mut self, id: TaskId, kind: TaskKind) {
        self.tasks.push(TaskRow {
            id,
            kind,
            state: TaskState::Running,
            finished_at: None,
        });
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
        }
    }

    fn key(&mut self, key: Key) -> Vec<Effect> {
        if key == Key::Interrupt {
            return vec![Effect::Quit];
        }
        match self.modal {
            Some(Modal::Pull(_)) => return self.pull_key(key),
            Some(Modal::Remove(_)) => return self.remove_key(key),
            Some(Modal::Help) => {
                self.modal = None;
                self.dirty = true;
                return Vec::new();
            }
            None => {}
        }
        if self.filtering && !matches!(key, Key::Up | Key::Down) {
            return self.filter_key(key);
        }
        match key {
            Key::Char('q') => return vec![Effect::Quit],
            Key::Down | Key::Char('j') => self.select(self.selected().saturating_add(1)),
            Key::Up | Key::Char('k') => self.select(self.selected().saturating_sub(1)),
            Key::Top | Key::Char('g') => self.select(0),
            Key::Bottom | Key::Char('G') => self.select(usize::MAX),
            Key::Char('s') if !self.already_running(&TaskKind::Scan) => {
                return vec![Effect::Spawn(TaskKind::Scan)];
            }
            Key::Char('r') => return self.refresh(),
            Key::Char('w') => return self.warm(),
            Key::Char('u') => return self.unload(),
            Key::Char('p') => {
                self.modal = Some(Modal::Pull(Box::new(PullModal::open(
                    &self.records,
                    self.facts.memory_bytes,
                ))));
                self.dirty = true;
            }
            Key::Char('x') => return self.remove(),
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
            Key::Char('?') => {
                self.modal = Some(Modal::Help);
                self.dirty = true;
            }
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
            (Stage::Listing, Key::Up) => modal.step(-1),
            (Stage::Listing, Key::Down) => modal.step(1),
            (Stage::Listing, Key::Backspace) => modal.backspace(now),
            (Stage::Listing, Key::Char(c)) => modal.type_char(c, now),
            (Stage::Listing, Key::Enter) => {
                if let Some((provider, reference)) = modal.choose() {
                    return vec![Effect::Plan(provider, reference)];
                }
            }
            (Stage::Preview(plan), Key::Enter) => {
                let kind = TaskKind::Pull(plan.clone());
                if self.already_running(&kind) {
                    let reference = kind.subject().to_owned();
                    return self.notify(format!("{reference} is already downloading"));
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
        match key {
            Key::Char(c) => {
                self.filter.push(c);
                self.reorder_in_place();
            }
            Key::Backspace => {
                self.filter.pop();
                self.reorder_in_place();
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

    /// Drop the newest failed task from the strip.
    fn dismiss(&mut self) -> Vec<Effect> {
        let failed = self
            .tasks
            .iter()
            .rposition(|row| matches!(row.state, TaskState::Failed(_)));
        match failed {
            Some(index) => {
                self.tasks.remove(index);
                self.dirty = true;
                Vec::new()
            }
            None => self.notify("nothing to dismiss".to_owned()),
        }
    }

    /// Open the removal confirmation for the selected model, or say why not.
    fn remove(&mut self) -> Vec<Effect> {
        let Some(record) = self.selected_record() else {
            return Vec::new();
        };
        if let Err(reason) = self.removable(record) {
            return self.notify(reason);
        }
        self.modal = Some(Modal::Remove(preview(record)));
        self.dirty = true;
        Vec::new()
    }

    /// Why `record` can't be removed right now, if it can't.
    fn removable(&self, record: &ModelRecord) -> Result<(), String> {
        let name = record.display_name();
        if self.busy_with(&record.id) {
            Err(format!("{name} is busy"))
        } else if self.facts.is_warm(&record.id) {
            Err(format!("{name} is warm; unload it first"))
        } else if record.downloading {
            Err(format!("{name} is still downloading; cancel it first"))
        } else if !is_deletable(record) {
            Err(format!("{name} can't be removed from here"))
        } else {
            Ok(())
        }
    }

    /// `y` re-checks everything `x` checked: the shelf and facts refresh
    /// behind the modal while it waits.
    fn remove_key(&mut self, key: Key) -> Vec<Effect> {
        let Some(Modal::Remove(shown)) = &self.modal else {
            return Vec::new();
        };
        let confirmed = match key {
            Key::Char('y') => true,
            Key::Char('n') | Key::Escape => false,
            _ => return Vec::new(),
        };
        let shown = shown.clone();
        let model_id = shown.model_id.clone();
        self.modal = None;
        self.dirty = true;
        if !confirmed {
            return Vec::new();
        }
        let Some(record) = self.records.iter().find(|record| record.id == model_id) else {
            return self.notify(format!("{} is no longer on the shelf", shown.name));
        };
        if let Err(reason) = self.removable(record) {
            return self.notify(reason);
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
        let pull = self
            .tasks
            .iter()
            .rev()
            .find(|row| row.running() && matches!(row.kind, TaskKind::Pull(_)));
        match pull {
            Some(row) => vec![Effect::Cancel(row.id)],
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
            modal.planned(&planned.reference, planned.result);
            self.dirty = true;
        }
    }

    fn warm(&mut self) -> Vec<Effect> {
        let Some(record) = self.selected_record() else {
            return Vec::new();
        };
        if self.busy_with(&record.id) {
            return Vec::new();
        }
        if self.facts.is_warm(&record.id) {
            return self.notify(format!("{} is already warm", record.display_name()));
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
        let name = record.display_name().to_owned();
        let id = record.id.clone();
        if self.busy_with(&id) {
            return Vec::new();
        }
        match self.facts.resident(&id).map(|resident| resident.holder) {
            None => self.notify(format!("{name} is not warm")),
            Some(Holder::Gateway) => {
                let port = self.facts.gateway_port.unwrap_or_default();
                self.notify(format!(
                    "{name} is held by the gateway on :{port}; it unloads there after its warm window"
                ))
            }
            Some(Holder::Local | Holder::Daemon) => {
                vec![Effect::Spawn(TaskKind::Unload { id, name })]
            }
        }
    }

    /// Whether a running task already concerns model `id`.
    fn busy_with(&self, id: &str) -> bool {
        self.tasks
            .iter()
            .any(|row| row.running() && row.kind.model_id() == Some(id))
    }

    /// Whether a task of `kind`'s shape is already running. Pulls match on
    /// what they fetch: two plans for one reference are one download.
    fn already_running(&self, kind: &TaskKind) -> bool {
        self.tasks.iter().any(|row| {
            row.running()
                && match (&row.kind, kind) {
                    (TaskKind::Pull(running), TaskKind::Pull(wanted)) => {
                        running.provider == wanted.provider && running.reference == wanted.reference
                    }
                    (running, wanted) => running == wanted,
                }
        })
    }

    fn notify(&mut self, text: String) -> Vec<Effect> {
        self.notice = Some((text, self.ticks + NOTICE_TICKS));
        self.dirty = true;
        Vec::new()
    }

    fn refresh(&mut self) -> Vec<Effect> {
        self.last_refresh = self.ticks;
        vec![Effect::Refresh]
    }

    fn tick(&mut self) -> Vec<Effect> {
        self.ticks += 1;
        let now = self.ticks;
        if let Some(Modal::Pull(modal)) = self.modal.as_mut()
            && let Some(query) = modal.search_due(now)
        {
            return vec![Effect::Search(query)];
        }
        if let Some((_, until)) = &self.notice
            && self.ticks >= *until
        {
            self.notice = None;
            self.dirty = true;
        }
        let before = self.tasks.len();
        self.tasks.retain(|row| {
            row.finished_at.is_none_or(|finished| {
                let linger = match row.state {
                    TaskState::Failed(_) => FAILED_LINGER_TICKS,
                    _ => DONE_LINGER_TICKS,
                };
                now < finished + linger
            })
        });
        if self.tasks.len() != before {
            self.dirty = true;
        }
        let cadence = if self.busy() {
            BUSY_REFRESH_TICKS
        } else {
            IDLE_REFRESH_TICKS
        };
        if self.ticks - self.last_refresh >= cadence {
            return self.refresh();
        }
        Vec::new()
    }

    fn task(&mut self, event: TaskEvent) -> Vec<Effect> {
        let Some(row) = self.tasks.iter_mut().find(|row| row.id == event.id) else {
            return Vec::new();
        };
        let finished = !event.state.running();
        row.state = event.state;
        if finished {
            row.finished_at = Some(self.ticks);
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
        self.reorder(selected_id);
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
    use crate::support::residency::Resident;
    use kernel::records::{Capability, Modality, ModelSource, SourceKind};

    fn record(index: usize) -> ModelRecord {
        ModelRecord::new(
            &format!("model-{index}"),
            Modality::text(),
            vec![Capability::chat()],
            ModelSource::new(SourceKind::ollama(), &format!("model-{index}")),
        )
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

    fn resident(id: &str, holder: Holder) -> Resident {
        Resident {
            id: id.to_owned(),
            name: id.to_owned(),
            bytes: 0,
            holder,
            expires_at_millis: None,
        }
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
        assert_eq!(app.tasks.len(), 1);
        ticks(&mut app, DONE_LINGER_TICKS + 1);
        assert!(app.tasks.is_empty());
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
        assert_eq!(app.tasks.len(), 1);
        ticks(&mut app, FAILED_LINGER_TICKS);
        assert!(app.tasks.is_empty());
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
        assert_eq!(pull(&app).input, "q");
        press(&mut app, Key::Backspace);
        let effects = press(&mut app, Key::Enter);
        assert!(matches!(effects.as_slice(), [Effect::Plan(_, _)]));
        press(&mut app, Key::Escape);
        assert_eq!(pull(&app).stage, Stage::Listing);
        press(&mut app, Key::Escape);
        assert!(app.modal.is_none());
        assert_eq!(press(&mut app, Key::Interrupt), vec![Effect::Quit]);
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
    fn cancel_targets_the_newest_running_pull() {
        let mut app = app(1);
        assert!(press(&mut app, Key::Char('c')).is_empty());
        assert_eq!(app.notice(), Some("nothing is downloading"));
        let id = TaskId::next();
        let plan = kernel::install::plan::InstallPlan {
            provider: kernel::install::provider::InstallProviderId::ollama(),
            reference: "x".to_owned(),
            display_name: "x".to_owned(),
            revision: None,
            files: Vec::new(),
            total_bytes: None,
            remaining_bytes: None,
            destination: String::new(),
            requires_auth: false,
        };
        app.started(id, TaskKind::Pull(plan.clone()));
        assert_eq!(press(&mut app, Key::Char('c')), vec![Effect::Cancel(id)]);

        let mut modal = PullModal::open(&[], 0);
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
        assert!(app.tasks.is_empty());
        press(&mut app, Key::Char('d'));
        assert_eq!(app.notice(), Some("nothing to dismiss"));
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
    fn help_opens_on_question_mark_and_any_key_closes_it() {
        let mut app = app(1);
        press(&mut app, Key::Char('?'));
        assert_eq!(app.modal, Some(Modal::Help));
        press(&mut app, Key::Char('j'));
        assert!(app.modal.is_none());
        assert_eq!(app.selected(), 0);
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
