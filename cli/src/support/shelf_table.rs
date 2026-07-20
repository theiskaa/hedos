//! Shared rendering of shelf rows: the aligned columns `hedos ls` prints and the
//! interactive model picker offers. Both draw from one column layout so a model
//! looks identical whether it is listed or selected.

use std::collections::HashSet;

use kernel::records::{Capability, ModelRecord};

/// The five columns shown for a model: warm marker, name, runtime, store, caps.
fn cells(record: &ModelRecord, warm: bool) -> [String; 5] {
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
        record
            .capabilities
            .iter()
            .map(Capability::as_str)
            .collect::<Vec<_>>()
            .join(", "),
    ]
}

/// Column widths wide enough for every row and the optional header.
fn widths(rows: &[[String; 5]], headers: Option<&[&str; 5]>) -> [usize; 5] {
    let mut widths = headers.map_or([0; 5], |headers| headers.map(str::len));
    for row in rows {
        for (column, cell) in row.iter().enumerate() {
            widths[column] = widths[column].max(cell.chars().count());
        }
    }
    widths
}

/// Pad each cell to its column width and join with two spaces.
fn format_row(cells: &[String; 5], widths: &[usize; 5]) -> String {
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

/// The full `hedos ls` table: a header row followed by one aligned row per model.
pub fn table(records: &[&ModelRecord], warm: &HashSet<String>) -> String {
    let rows: Vec<[String; 5]> = records
        .iter()
        .map(|record| cells(record, warm.contains(&record.id)))
        .collect();
    let headers = ["", "NAME", "RUNTIME", "STORE", "CAPABILITIES"];
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
pub fn picker_labels(records: &[&ModelRecord], warm: &HashSet<String>) -> Vec<String> {
    let rows: Vec<[String; 5]> = records
        .iter()
        .map(|record| cells(record, warm.contains(&record.id)))
        .collect();
    let widths = widths(&rows, None);
    rows.iter().map(|row| format_row(row, &widths)).collect()
}
