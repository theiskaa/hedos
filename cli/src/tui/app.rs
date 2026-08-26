//! The whole UI state and the reducer over it. Pure: it never touches the
//! kernel or the terminal, so every transition is unit-testable.

use kernel::profiles::FitVerdict;
use kernel::records::ModelRecord;
use ratatui::widgets::TableState;

use super::effect::Effect;
use super::event::{Event, Key};
use super::facts::Facts;

/// Everything the screen shows.
pub struct App {
    /// The shelf, in the order it is listed.
    pub records: Vec<ModelRecord>,
    /// The machine facts from the last refresh.
    pub facts: Facts,
    /// The shelf's selection and scroll position; ratatui keeps the selected
    /// row in view through it.
    pub shelf: TableState,
    dirty: bool,
}

impl App {
    /// A UI over `records`, selecting the first.
    pub fn new(records: Vec<ModelRecord>, facts: Facts) -> Self {
        Self {
            records,
            facts,
            shelf: TableState::new().with_selected(0),
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

    /// Whether something changed since the last draw; reading it clears it.
    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    /// Apply `event` and return the effects the loop must perform.
    pub fn reduce(&mut self, event: Event) -> Vec<Effect> {
        match event {
            Event::Key(key) => self.key(key),
            Event::Resize => {
                self.dirty = true;
                Vec::new()
            }
            Event::Tick => Vec::new(),
        }
    }

    fn key(&mut self, key: Key) -> Vec<Effect> {
        match key {
            Key::Char('q') | Key::Interrupt => return vec![Effect::Quit],
            Key::Down | Key::Char('j') => self.select(self.selected().saturating_add(1)),
            Key::Up | Key::Char('k') => self.select(self.selected().saturating_sub(1)),
            Key::Top | Key::Char('g') => self.select(0),
            Key::Bottom | Key::Char('G') => self.select(usize::MAX),
            _ => {}
        }
        Vec::new()
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
    use crate::tui::facts::{Holder, Resident};
    use kernel::records::{Capability, Modality, ModelSource, SourceKind};

    fn app(count: usize) -> App {
        let records = (0..count)
            .map(|index| {
                ModelRecord::new(
                    &format!("model-{index}"),
                    Modality::text(),
                    vec![Capability::chat()],
                    ModelSource::new(SourceKind::ollama(), &format!("model-{index}")),
                )
            })
            .collect();
        App::new(records, Facts::default())
    }

    fn press(app: &mut App, key: Key) -> Vec<Effect> {
        app.reduce(Event::Key(key))
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
        for id in [app.records[0].id.clone(), "not-on-the-shelf".to_owned()] {
            app.facts.residents.push(Resident {
                id,
                name: String::new(),
                bytes: 0,
                holder: Holder::Local,
                expires_at_millis: None,
            });
        }
        assert_eq!(app.warm_count(), 1);
    }
}
