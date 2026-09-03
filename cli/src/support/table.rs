//! Aligned plain-text tables: column widths wide enough for every cell, and a
//! row padded to them. The shelf and the pull listing share it so their columns
//! line up the same way.

use unicode_width::UnicodeWidthStr;

/// The placeholder for a value a row does not have.
pub const DASH: &str = "—";

/// Column widths wide enough for every row and the optional header.
pub fn widths(rows: &[Vec<String>], headers: Option<&[&str]>) -> Vec<usize> {
    let mut widths: Vec<usize> = headers
        .map(|headers| headers.iter().map(|header| header.width()).collect())
        .unwrap_or_default();
    for row in rows {
        if row.len() > widths.len() {
            widths.resize(row.len(), 0);
        }
        for (column, cell) in row.iter().enumerate() {
            widths[column] = widths[column].max(cell.width());
        }
    }
    widths
}

/// Pad each cell to its column width and join with two spaces.
pub fn row(cells: &[String], widths: &[usize]) -> String {
    cells
        .iter()
        .enumerate()
        .map(|(column, cell)| {
            let pad = widths
                .get(column)
                .copied()
                .unwrap_or_default()
                .saturating_sub(cell.width());
            format!("{cell}{}", " ".repeat(pad))
        })
        .collect::<Vec<_>>()
        .join("  ")
        .trim_end()
        .to_owned()
}

/// A header row followed by one row per entry.
pub fn render(headers: &[&str], rows: &[Vec<String>]) -> String {
    let widths = widths(rows, Some(headers));
    let header: Vec<String> = headers.iter().map(|label| (*label).to_owned()).collect();
    let mut lines = vec![row(&header, &widths)];
    lines.extend(rows.iter().map(|cells| row(cells, &widths)));
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cells(row: &[&str]) -> Vec<String> {
        row.iter().map(|cell| (*cell).to_owned()).collect()
    }

    #[test]
    fn a_column_is_as_wide_as_its_widest_cell_or_its_header() {
        let rows = vec![cells(&["a", "long-value"]), cells(&["bbbb", "x"])];
        assert_eq!(widths(&rows, Some(&["HEADER", "H"])), vec![6, 10]);
        assert_eq!(widths(&rows, None), vec![4, 10]);
    }

    #[test]
    fn cells_are_padded_to_their_column_and_the_line_is_not() {
        assert_eq!(row(&cells(&["a", "b"]), &[3, 3]), "a    b");
    }

    #[test]
    fn a_cell_wider_than_one_column_measures_by_what_it_takes_on_screen() {
        // A double-width glyph counts for two cells, which is what keeps the
        // next column under its header rather than one place to the left.
        let rows = vec![cells(&["日本", "x"])];
        assert_eq!(widths(&rows, None), vec![4, 1]);
    }

    #[test]
    fn a_table_puts_its_header_over_its_rows() {
        let table = render(&["ID", "STATE"], &[cells(&["abc", "running"])]);
        assert_eq!(table, "ID   STATE\nabc  running");
    }
}
