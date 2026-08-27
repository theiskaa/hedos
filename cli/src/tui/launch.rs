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
        Self::open_with(record, HarnessSpec::locate)
    }

    /// [`open`](Self::open) finding each harness's binary through `locate`,
    /// so the rows can be built without looking at `PATH`.
    pub fn open_with(
        record: &ModelRecord,
        locate: impl Fn(&HarnessSpec) -> Option<PathBuf>,
    ) -> Self {
        let rows: Vec<HarnessRow> = HARNESSES
            .iter()
            .map(|spec| {
                let program = locate(spec);
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
    use crate::tui::testing::{record, record_with};
    use kernel::records::Capability;

    fn installed(_: &HarnessSpec) -> Option<PathBuf> {
        Some(PathBuf::from("/usr/local/bin/harness"))
    }

    #[test]
    fn tool_driven_harnesses_are_blocked_without_tools() {
        let modal = LaunchModal::open_with(&record("m"), installed);
        assert_eq!(modal.rows.len(), HARNESSES.len());
        for row in &modal.rows {
            if row.spec.needs_tools {
                assert!(
                    row.blocked
                        .as_deref()
                        .is_some_and(|why| why.starts_with("needs tools"))
                );
            } else {
                assert_eq!(row.blocked, None);
            }
        }
        let with_tools = record_with("m", vec![Capability::chat(), Capability::tools()]);
        assert!(
            LaunchModal::open_with(&with_tools, installed)
                .rows
                .iter()
                .all(|row| row.blocked.is_none())
        );
        let none = LaunchModal::open_with(&record("m"), |_| None);
        assert!(none.rows.iter().all(|row| {
            row.blocked
                .as_deref()
                .is_some_and(|why| why.starts_with("not installed"))
        }));
    }

    #[test]
    fn stepping_clamps() {
        let mut modal = LaunchModal::open_with(&record("m"), installed);
        modal.step(-5);
        assert_eq!(modal.selected, 0);
        modal.step(50);
        assert_eq!(modal.selected, HARNESSES.len() - 1);
    }
}
