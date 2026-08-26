//! The launch modal's state: every harness hedos knows, the ones that can
//! seat the selected model selectable, the rest with the reason they can't.

use std::path::PathBuf;

use kernel::records::ModelRecord;

use crate::support::harnesses::{HARNESSES, HarnessSpec};

/// One harness as the modal offers it.
#[derive(Debug, Clone, PartialEq)]
pub struct HarnessRow {
    pub spec: &'static HarnessSpec,
    /// The binary, when it is installed.
    pub program: Option<PathBuf>,
    /// Why it can't be launched on this model, if it can't.
    pub blocked: Option<String>,
}

/// The launch modal.
#[derive(Debug, Clone, PartialEq)]
pub struct LaunchModal {
    pub record: ModelRecord,
    pub rows: Vec<HarnessRow>,
    pub selected: usize,
}

impl LaunchModal {
    /// The modal for `record`, the first launchable harness selected.
    pub fn open(record: &ModelRecord) -> Self {
        let rows: Vec<HarnessRow> = HARNESSES
            .iter()
            .map(|spec| {
                let program = spec.locate();
                let blocked = if program.is_none() {
                    Some(format!("not installed · {}", spec.homepage))
                } else if !record.capabilities.contains(&spec.needed_capability()) {
                    Some(format!(
                        "needs {} · {} has none",
                        spec.needed_capability().as_str(),
                        record.display_name()
                    ))
                } else {
                    None
                };
                HarnessRow {
                    spec,
                    program,
                    blocked,
                }
            })
            .collect();
        let selected = rows
            .iter()
            .position(|row| row.blocked.is_none())
            .unwrap_or(0);
        Self {
            record: record.clone(),
            rows,
            selected,
        }
    }

    /// The highlighted row.
    pub fn selected_row(&self) -> &HarnessRow {
        &self.rows[self.selected.min(self.rows.len() - 1)]
    }

    /// Move the highlight by `delta` rows.
    pub fn step(&mut self, delta: isize) {
        let last = self.rows.len().saturating_sub(1) as isize;
        self.selected = (self.selected as isize + delta).clamp(0, last) as usize;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel::records::{Capability, Modality, ModelSource, SourceKind};

    fn record(capabilities: Vec<Capability>) -> ModelRecord {
        ModelRecord::new(
            "m",
            Modality::text(),
            capabilities,
            ModelSource::new(SourceKind::ollama(), "m"),
        )
    }

    #[test]
    fn tool_driven_harnesses_are_blocked_without_tools() {
        let modal = LaunchModal::open(&record(vec![Capability::chat()]));
        assert_eq!(modal.rows.len(), HARNESSES.len());
        for row in modal.rows.iter().filter(|row| row.spec.needs_tools) {
            assert!(row.blocked.is_some());
        }
    }

    #[test]
    fn stepping_clamps() {
        let mut modal = LaunchModal::open(&record(vec![Capability::chat()]));
        modal.step(-5);
        assert_eq!(modal.selected, 0);
        modal.step(50);
        assert_eq!(modal.selected, HARNESSES.len() - 1);
    }
}
