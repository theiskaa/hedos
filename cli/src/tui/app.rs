//! The whole UI state and the reducer over it. Pure: it never touches the
//! kernel or the terminal, so every transition is unit-testable.

use std::time::Duration;

use kernel::profiles::FitVerdict;
use kernel::records::{Capability, ModelRecord};
use ratatui::widgets::TableState;

use super::effect::Effect;
use super::event::{Event, Key, Refreshed};
use super::facts::{Facts, Holder};
use super::tasks::{TaskEvent, TaskId, TaskKind, TaskState};

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
        self.state == TaskState::Running
    }
}

/// Everything the screen shows.
pub struct App {
    /// The shelf, in the order it is listed.
    pub records: Vec<ModelRecord>,
    /// The machine facts from the last refresh.
    pub facts: Facts,
    /// The shelf's selection and scroll position; ratatui keeps the selected
    /// row in view through it.
    pub shelf: TableState,
    /// Background work, oldest first.
    pub tasks: Vec<TaskRow>,
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
        Self {
            records,
            facts,
            shelf: TableState::new().with_selected(0),
            tasks: Vec::new(),
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

    /// The selected record, if the shelf has any.
    pub fn selected_record(&self) -> Option<&ModelRecord> {
        self.records.get(self.selected())
    }

    /// The machine's memory, for fit labels.
    pub fn memory_budget_bytes(&self) -> u64 {
        self.facts.memory_bytes
    }

    /// How many models on the shelf can't run on this machine.
    pub fn too_big_count(&self) -> usize {
        self.records
            .iter()
            .filter(|record| too_big(record, self.facts.memory_bytes))
            .count()
    }

    /// How many models on the shelf are held in memory.
    pub fn warm_count(&self) -> usize {
        self.records
            .iter()
            .filter(|record| self.facts.is_warm(&record.id))
            .count()
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
        }
    }

    fn key(&mut self, key: Key) -> Vec<Effect> {
        match key {
            Key::Char('q') | Key::Interrupt => return vec![Effect::Quit],
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
            _ => {}
        }
        Vec::new()
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
            Some(Holder::Local) => vec![Effect::Spawn(TaskKind::Unload { id, name })],
        }
    }

    /// Whether a running task already concerns model `id`.
    fn busy_with(&self, id: &str) -> bool {
        self.tasks
            .iter()
            .any(|row| row.running() && row.kind.model_id() == Some(id))
    }

    /// Whether a task of `kind`'s shape is already running.
    fn already_running(&self, kind: &TaskKind) -> bool {
        self.tasks
            .iter()
            .any(|row| row.running() && &row.kind == kind)
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
        if let Some((_, until)) = &self.notice
            && self.ticks >= *until
        {
            self.notice = None;
            self.dirty = true;
        }
        let before = self.tasks.len();
        let now = self.ticks;
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
        let finished = event.state != TaskState::Running;
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
        let index = selected_id
            .and_then(|id| self.records.iter().position(|record| record.id == id))
            .unwrap_or(self.selected());
        self.shelf
            .select(Some(index.min(self.records.len().saturating_sub(1))));
        self.dirty = true;
    }

    fn select(&mut self, index: usize) {
        let last = self.records.len().saturating_sub(1);
        let clamped = index.min(last);
        if clamped != self.selected() {
            self.shelf.select(Some(clamped));
            self.dirty = true;
        }
    }
}

/// Whether `record` is judged too large for a machine with `memory_bytes`.
pub(crate) fn too_big(record: &ModelRecord, memory_bytes: u64) -> bool {
    FitVerdict::assess(record.footprint_mb, memory_bytes)
        .is_some_and(|fit| fit.verdict == FitVerdict::TooLarge)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::facts::Resident;
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
        App::new((0..count).map(record).collect(), Facts::default())
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
    fn warm_count_only_counts_shelf_models() {
        let mut app = app(2);
        app.facts
            .residents
            .push(resident(&app.records[0].id, Holder::Local));
        app.facts
            .residents
            .push(resident("not-on-the-shelf", Holder::Local));
        assert_eq!(app.warm_count(), 1);
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
