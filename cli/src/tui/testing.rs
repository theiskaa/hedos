//! The builders every test module under `tui` needs: a record, a resident,
//! a plan, a deletion preview, the machine's facts, and ways to read a
//! rendered line back as text.
//!
//! The tripwire policy: a test derives its expectations from the constants
//! the code is built from, so a layout change moves the test with it. At
//! most one literal pin per layout is kept, marked as such, to trip when
//! the constants themselves drift.

use kernel::install::plan::InstallPlan;
use kernel::install::provider::InstallProviderId;
use kernel::records::SourceKind;
use kernel::records::{Capability, Modality, ModelRecord, ModelSource};
use kernel::removal::ModelDeletionPreview;
use ratatui::text::Line;

use super::facts::Facts;
use crate::support::residency::{Holder, Resident};

/// An Ollama-sourced chat model called `name`.
pub fn record(name: &str) -> ModelRecord {
    record_with(name, vec![Capability::chat()])
}

/// An Ollama-sourced model called `name` with `capabilities`.
pub fn record_with(name: &str, capabilities: Vec<Capability>) -> ModelRecord {
    ModelRecord::new(
        name,
        Modality::text(),
        capabilities,
        ModelSource::new(SourceKind::ollama(), name),
    )
}

/// Model `id` loaded by `holder`, with no size or deadline.
pub fn resident(id: &str, holder: Holder) -> Resident {
    resident_with_bytes(id, holder, 0)
}

/// Model `id` loaded by `holder`, `bytes` large, with no deadline.
pub fn resident_with_bytes(id: &str, holder: Holder, bytes: i64) -> Resident {
    Resident {
        id: id.to_owned(),
        name: id.to_owned(),
        bytes,
        holder,
        expires_at_millis: None,
    }
}

/// A machine with `gib` GiB of memory and nothing else known about it.
pub fn facts_with_memory(gib: u64) -> Facts {
    Facts {
        memory_bytes: gib << 30,
        ..Facts::default()
    }
}

/// What removing the Ollama model `m` would delete: `paths`, sized at
/// nothing, by hand rather than through the daemon.
pub fn deletion_preview(paths: Vec<String>) -> ModelDeletionPreview {
    ModelDeletionPreview {
        model_id: "m".to_owned(),
        name: "m".to_owned(),
        kind: SourceKind::ollama(),
        paths,
        bytes_estimate: 0,
        via_daemon: false,
        missing: false,
    }
}

/// An Ollama plan for `reference` with nothing known about its size.
pub fn plan(reference: &str) -> InstallPlan {
    InstallPlan {
        provider: InstallProviderId::ollama(),
        reference: reference.to_owned(),
        display_name: reference.to_owned(),
        revision: None,
        files: Vec::new(),
        total_bytes: None,
        remaining_bytes: None,
        destination: String::new(),
        requires_auth: false,
    }
}

/// The text of a rendered line, styles dropped.
pub fn text(line: &Line) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

/// The text of every line in `lines`.
pub fn texts(lines: &[Line]) -> Vec<String> {
    lines.iter().map(text).collect()
}

/// The label a row starts with, for the tests that hold each pane to its
/// label list: the cells after the leading space up to the label column's
/// `width`, trimmed.
pub fn leading_label(line: &Line, width: usize) -> String {
    text(line)
        .chars()
        .skip(1)
        .take(width)
        .collect::<String>()
        .trim_end()
        .to_owned()
}
