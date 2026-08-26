//! The builders every test module under `tui` needs: a record, a resident,
//! a plan, and a way to read a rendered line back as text.

use kernel::install::plan::InstallPlan;
use kernel::install::provider::InstallProviderId;
use kernel::records::SourceKind;
use kernel::records::{Capability, Modality, ModelRecord, ModelSource};
use ratatui::text::Line;

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
    Resident {
        id: id.to_owned(),
        name: id.to_owned(),
        bytes: 0,
        holder,
        expires_at_millis: None,
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
pub fn line_text(line: &Line) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}
