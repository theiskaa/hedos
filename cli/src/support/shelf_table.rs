//! Shared rendering of shelf rows: the aligned columns `hedos ls` prints and the
//! interactive model picker offers. Both draw from one column layout so a model
//! looks identical whether it is listed or selected.

use std::collections::HashSet;

use kernel::profiles::FitVerdict;
use kernel::records::{Capability, ModelRecord};

/// The six columns shown for a model: warm marker, name, runtime, store, fit, caps.
fn cells(record: &ModelRecord, warm: bool, total_memory_bytes: u64) -> [String; 6] {
    [
        if warm { "●" } else { "○" }.to_owned(),
        record.display_name().to_owned(),
        record
            .runtime
            .id
            .as_ref()
            .map_or("—", |id| id.as_str())
            .to_owned(),
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

/// A short human fit label from the model's footprint and the machine's memory:
/// `fits` / `tight` / `too big`, or `—` when the footprint is unknown (the same
/// dash the runtime column uses for an unresolved runtime).
fn fit_label(record: &ModelRecord, total_memory_bytes: u64) -> &'static str {
    match FitVerdict::assess(record.footprint_mb, total_memory_bytes).map(|fit| fit.verdict) {
        Some(FitVerdict::RunsWell) => "fits",
        Some(FitVerdict::TightFit) => "tight",
        Some(FitVerdict::TooLarge) => "too big",
        None => "—",
    }
}

/// Column widths wide enough for every row and the optional header.
fn widths(rows: &[[String; 6]], headers: Option<&[&str; 6]>) -> [usize; 6] {
    let mut widths = headers.map_or([0; 6], |headers| headers.map(str::len));
    for row in rows {
        for (column, cell) in row.iter().enumerate() {
            widths[column] = widths[column].max(cell.chars().count());
        }
    }
    widths
}

/// Pad each cell to its column width and join with two spaces.
fn format_row(cells: &[String; 6], widths: &[usize; 6]) -> String {
    cells
        .iter()
        .enumerate()
        .map(|(column, cell)| {
            let pad = widths[column].saturating_sub(cell.chars().count());
            format!("{cell}{}", " ".repeat(pad))
        })
        .collect::<Vec<_>>()
        .join("  ")
        .trim_end()
        .to_owned()
}

/// The full `hedos ls` table: a header row followed by one aligned row per model,
/// with fit judged against `total_memory_bytes`.
pub fn table(records: &[&ModelRecord], warm: &HashSet<String>, total_memory_bytes: u64) -> String {
    let rows: Vec<[String; 6]> = records
        .iter()
        .map(|record| cells(record, warm.contains(&record.id), total_memory_bytes))
        .collect();
    let headers = ["", "NAME", "RUNTIME", "STORE", "FIT", "CAPABILITIES"];
    let widths = widths(&rows, Some(&headers));

    let mut lines = Vec::with_capacity(rows.len() + 1);
    lines.push(format_row(&headers.map(String::from), &widths));
    for row in &rows {
        lines.push(format_row(row, &widths));
    }
    lines.join("\n")
}

/// Aligned one-line labels for the interactive picker, one per model, in the same
/// column layout as [`table`] but without a header.
pub fn picker_labels(
    records: &[&ModelRecord],
    warm: &HashSet<String>,
    total_memory_bytes: u64,
) -> Vec<String> {
    let rows: Vec<[String; 6]> = records
        .iter()
        .map(|record| cells(record, warm.contains(&record.id), total_memory_bytes))
        .collect();
    let widths = widths(&rows, None);
    rows.iter().map(|row| format_row(row, &widths)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel::records::{Modality, ModelSource, SourceKind};

    const GIB: u64 = 1 << 30;

    fn model(name: &str, footprint_mb: Option<i64>) -> ModelRecord {
        let mut record = ModelRecord::new(
            name,
            Modality::text(),
            vec![Capability::chat()],
            ModelSource::new(SourceKind::ollama(), name),
        );
        record.footprint_mb = footprint_mb;
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
        assert_eq!(fit_of(&model("small", Some(1024))), "fits");
        assert_eq!(fit_of(&model("mid", Some(12 * 1024))), "tight");
        assert_eq!(fit_of(&model("huge", Some(16 * 1024))), "too big");
    }

    #[test]
    fn an_unknown_footprint_renders_a_dash() {
        assert_eq!(fit_of(&model("mystery", None)), "—");
    }

    #[test]
    fn the_table_has_a_fit_header() {
        let record = model("gemma", Some(1024));
        let records = [&record];
        let rendered = table(&records, &HashSet::new(), 16 * GIB);
        let header = rendered.lines().next().expect("a header row");
        let fit = header.find("FIT").expect("a FIT header");
        let capabilities = header.find("CAPABILITIES").expect("a CAPABILITIES header");
        // FIT is a fixed-width column placed before the ragged capabilities tail.
        assert!(fit < capabilities);
    }
}
