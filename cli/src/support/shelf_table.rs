//! The aligned shelf table `hedos ls` prints and the interactive picker offers,
//! plus the label helpers the UI's own table shares with it.

use std::collections::HashSet;

use kernel::profiles::FitVerdict;
use kernel::records::{Capability, ModelRecord, ModelState};

use crate::support::table::{self, DASH};

/// The six columns shown for a model: state marker, name, runtime, store, fit, caps.
pub(crate) fn cells(record: &ModelRecord, warm: bool, total_memory_bytes: u64) -> [String; 6] {
    [
        marker(record, warm).to_owned(),
        record.display_name().to_owned(),
        runtime_label(record).to_owned(),
        record.source.kind.as_str().to_owned(),
        fit_label(record, total_memory_bytes).to_owned(),
        record
            .capabilities
            .iter()
            .map(Capability::as_str)
            .collect::<Vec<_>>()
            .join(", "),
    ]
}

/// The gutter mark: a cross for a model whose weights are gone, else a filled
/// dot for a warm model and a hollow one for a cold one. Shared with the UI's
/// own shelf so a row means the same thing wherever it is drawn.
pub(crate) fn marker(record: &ModelRecord, warm: bool) -> &'static str {
    match (record.state == ModelState::Missing, warm) {
        (true, _) => "✕",
        (false, true) => "●",
        (false, false) => "○",
    }
}

/// The runtime id, or [`DASH`] for an unresolved runtime.
pub(crate) fn runtime_label(record: &ModelRecord) -> &str {
    record.runtime.id.as_ref().map_or(DASH, |id| id.as_str())
}

/// How a model of `footprint_bytes` on disk fits in `memory_bytes`, when the
/// footprint is known.
pub(crate) fn verdict(footprint_bytes: Option<i64>, memory_bytes: u64) -> Option<FitVerdict> {
    FitVerdict::assess(footprint_bytes, memory_bytes).map(|fit| fit.verdict)
}

/// The short human form of a verdict: `fits` / `tight` / `too big`, empty
/// when there is none.
pub(crate) fn verdict_label(verdict: Option<FitVerdict>) -> &'static str {
    match verdict {
        Some(FitVerdict::RunsWell) => "fits",
        Some(FitVerdict::TightFit) => "tight",
        Some(FitVerdict::TooLarge) => "too big",
        None => "",
    }
}

/// The fit column from the model's footprint and the machine's memory, or `—`
/// when the footprint is unknown (the same dash the runtime column uses for an
/// unresolved runtime).
///
/// A model whose weights are gone reads `gone` instead of a verdict: there is
/// nothing left to fit, and a fit here would be the row's one honest-looking
/// claim about a model that cannot run at all.
fn fit_label(record: &ModelRecord, total_memory_bytes: u64) -> &'static str {
    if record.state == ModelState::Missing {
        return "gone";
    }
    match verdict(record.footprint_bytes, total_memory_bytes) {
        Some(fit) => verdict_label(Some(fit)),
        None => DASH,
    }
}

/// The six columns as a row of cells, for the shared table helpers.
fn row_cells(record: &ModelRecord, warm: bool, total_memory_bytes: u64) -> Vec<String> {
    cells(record, warm, total_memory_bytes).to_vec()
}

/// The `hedos ls` header row, one label per column.
pub(crate) const HEADERS: [&str; 6] = ["", "NAME", "RUNTIME", "STORE", "FIT", "CAPABILITIES"];

/// The full `hedos ls` table: a header row followed by one aligned row per model,
/// with fit judged against `total_memory_bytes`.
pub fn table(records: &[&ModelRecord], warm: &HashSet<String>, total_memory_bytes: u64) -> String {
    let rows: Vec<Vec<String>> = records
        .iter()
        .map(|record| row_cells(record, warm.contains(&record.id), total_memory_bytes))
        .collect();
    table::render(&HEADERS, &rows)
}

/// Aligned one-line labels for the interactive picker, one per model, in the same
/// column layout as [`table`] but without a header.
pub fn picker_labels(
    records: &[&ModelRecord],
    warm: &HashSet<String>,
    total_memory_bytes: u64,
) -> Vec<String> {
    let rows: Vec<Vec<String>> = records
        .iter()
        .map(|record| row_cells(record, warm.contains(&record.id), total_memory_bytes))
        .collect();
    let widths = table::widths(&rows, None);
    rows.iter().map(|row| table::row(row, &widths)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel::records::{Modality, ModelSource, SourceKind};

    const GIB: u64 = 1 << 30;

    fn model(name: &str, footprint_bytes: Option<i64>) -> ModelRecord {
        let mut record = ModelRecord::new(
            name,
            Modality::text(),
            vec![Capability::chat()],
            ModelSource::new(SourceKind::ollama(), name),
        );
        record.footprint_bytes = footprint_bytes;
        record
    }

    fn fit_of(record: &ModelRecord) -> String {
        // Column 4 (0-indexed) is FIT; judged against a fixed 16 GiB machine.
        cells(record, false, 16 * GIB)[4].clone()
    }

    #[test]
    fn fit_column_tracks_the_verdict() {
        // Boundaries mirror the kernel's own fit tests: 1 GiB fits, 12 GiB is
        // tight, 16 GiB is too big against a 16 GiB machine.
        assert_eq!(fit_of(&model("small", Some(GIB as i64))), "fits");
        assert_eq!(fit_of(&model("mid", Some(12 * GIB as i64))), "tight");
        assert_eq!(fit_of(&model("huge", Some(16 * GIB as i64))), "too big");
    }

    #[test]
    fn a_model_whose_weights_are_gone_says_so_instead_of_fitting() {
        let mut record = model("tiny", Some(GIB as i64));
        record.state = ModelState::Missing;
        let row = cells(&record, false, 16 * GIB);
        assert_eq!(row[0], "✕", "the gutter marks it");
        assert_eq!(row[4], "gone", "and the fit column says why");
    }

    #[test]
    fn a_warm_model_keeps_its_filled_dot() {
        assert_eq!(marker(&model("warm", Some(GIB as i64)), true), "●");
        assert_eq!(marker(&model("cold", Some(GIB as i64)), false), "○");
    }

    #[test]
    fn an_unknown_footprint_renders_a_dash() {
        assert_eq!(fit_of(&model("mystery", None)), "—");
    }

    #[test]
    fn the_table_has_a_fit_header() {
        let record = model("gemma", Some(GIB as i64));
        let records = [&record];
        let rendered = table(&records, &HashSet::new(), 16 * GIB);
        let header = rendered.lines().next().expect("a header row");
        let fit = header.find("FIT").expect("a FIT header");
        let capabilities = header.find("CAPABILITIES").expect("a CAPABILITIES header");
        // FIT is a fixed-width column placed before the ragged capabilities tail.
        assert!(fit < capabilities);
    }
}
