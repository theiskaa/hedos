//! The help card: the keymap's groups laid out in columns of `key  verb`
//! cells, three of them on a wide terminal and two on a narrow one, the
//! closer under the table and the one idea behind the keys under that.

use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use super::{BORDER_COLUMNS, BORDER_ROWS, MARGIN, modal_width};
use crate::tui::keymap;
use crate::tui::ui::{DIM, EYEBROW, keys, padded};

/// Bindings the help shows in one cell: side by side under a verb they
/// share, or with `/` between the keys and ` / ` between the verbs. `Y`'s
/// gloss is the bare `id` because it only ever reads joined with `y`, as
/// `copy path / id`.
const JOINED: [&[&str]; 2] = [&["j/k", "↑/↓"], &["y", "Y"]];
/// Cells between a key and its verb, and between a verb and the next key.
const KEY_GAP: usize = 3;
const VERB_GAP: usize = 2;
/// The one idea behind the keys, shown under them when the card is wide
/// enough for the whole sentence.
const HELP_NOTE: &str = "  every key is a hedos subcommand: p is pull, x is rm, w is warm";

/// One line of a help column.
enum HelpCell {
    Header(&'static str),
    Blank,
    Row { key: String, verb: String },
}

/// The help as it lays out at one terminal width: its columns, the card's
/// width, and what the border leaves inside once the card is clamped to
/// the terminal.
pub(super) struct HelpLayout {
    columns: Vec<Vec<HelpCell>>,
    /// The card's width: the table, a cell of air on its right, the border.
    pub(super) width: u16,
    /// Cells inside the card's border.
    pub(super) inner: usize,
}

impl HelpLayout {
    /// The layout at a terminal `width` cells wide: three columns from
    /// [`three_columns_from`], two under it.
    pub(super) fn at(width: u16) -> Self {
        let columns = if width >= three_columns_from() {
            three_columns()
        } else {
            two_columns()
        };
        let card = card_width(&columns);
        let inner = modal_width(card, width).saturating_sub(BORDER_COLUMNS) as usize;
        Self {
            columns,
            width: card,
            inner,
        }
    }

    /// The key table and its closer, then the one idea behind the keys when
    /// the card is wide enough for it; the closer comes first so a short
    /// terminal clips the note, not the way out.
    pub(super) fn lines(&self) -> Vec<Line<'static>> {
        let mut lines = vec![Line::default()];
        lines.extend(table_lines(&self.columns));
        lines.push(Line::default());
        lines.push(keys(&[("esc", "close")]));
        if self.inner >= HELP_NOTE.width() {
            lines.push(Line::default());
            lines.push(Line::from(Span::styled(HELP_NOTE, DIM)));
        }
        lines
    }

    /// The card's rows: its lines and the border.
    pub(super) fn height(&self) -> u16 {
        self.lines().len() as u16 + BORDER_ROWS
    }

    /// Whether the help has folded to two columns.
    #[cfg(test)]
    pub(super) fn folded(&self) -> bool {
        self.columns.len() == 2
    }
}

/// A help cell as `(keys, gloss)`: bindings that share a gloss are listed
/// side by side under it, the rest are joined with `/` and their glosses
/// with ` / `.
fn help_cell(keys: &[&str]) -> (String, String) {
    let bindings: Vec<&keymap::Binding> =
        keys.iter().filter_map(|key| keymap::binding(key)).collect();
    let mut glosses: Vec<&str> = Vec::new();
    for binding in &bindings {
        if !glosses.contains(&binding.gloss()) {
            glosses.push(binding.gloss());
        }
    }
    let keys: Vec<&str> = bindings.iter().map(|binding| binding.key).collect();
    let separator = if glosses.len() == 1 { " " } else { "/" };
    (keys.join(separator), glosses.join(" / "))
}

/// A group's heading and its bindings in the order they are declared, the
/// joined ones as one cell.
fn group_cells(group: keymap::Group) -> Vec<HelpCell> {
    let mut cells = vec![HelpCell::Header(group.label())];
    let mut joined_already: Vec<&str> = Vec::new();
    for binding in keymap::BINDINGS
        .iter()
        .filter(|binding| binding.group == group)
    {
        if joined_already.contains(&binding.key) {
            continue;
        }
        let keys = JOINED
            .iter()
            .find(|set| set[0] == binding.key)
            .copied()
            .unwrap_or(std::slice::from_ref(&binding.key));
        joined_already.extend(&keys[1..]);
        let (key, verb) = help_cell(keys);
        cells.push(HelpCell::Row { key, verb });
    }
    cells
}

/// `top` over `bottom` in one column, a blank between them.
fn stacked(top: keymap::Group, bottom: keymap::Group) -> Vec<HelpCell> {
    let mut cells = group_cells(top);
    cells.push(HelpCell::Blank);
    cells.extend(group_cells(bottom));
    cells
}

/// The help in three columns, the screen keys under the move keys.
fn three_columns() -> Vec<Vec<HelpCell>> {
    use keymap::Group;
    vec![
        stacked(Group::Move, Group::Screen),
        group_cells(Group::Model),
        group_cells(Group::Shelf),
    ]
}

/// The help in two columns, the shelf keys under the model keys as well.
fn two_columns() -> Vec<Vec<HelpCell>> {
    use keymap::Group;
    vec![
        stacked(Group::Move, Group::Screen),
        stacked(Group::Model, Group::Shelf),
    ]
}

/// Terminal columns from which the help lays its groups out in three
/// columns: the three-column table, the border, and a margin either side.
/// Narrower, it folds to two.
pub(super) fn three_columns_from() -> u16 {
    table_width(&three_columns()) as u16 + BORDER_COLUMNS + 2 * MARGIN
}

/// The card's width for `columns`: the table with a cell of air on its
/// right, and the border.
fn card_width(columns: &[Vec<HelpCell>]) -> u16 {
    table_width(columns) as u16 + BORDER_COLUMNS + 2
}

/// A column's key and verb widths with their gaps: the widest key plus
/// `KEY_GAP`, the widest verb plus `VERB_GAP`.
fn column_widths(column: &[HelpCell]) -> (usize, usize) {
    let widest = |pick: fn(&String, &String) -> usize| {
        column
            .iter()
            .filter_map(|cell| match cell {
                HelpCell::Row { key, verb } => Some(pick(key, verb)),
                _ => None,
            })
            .max()
            .unwrap_or(0)
    };
    (
        widest(|key, _| key.width()) + KEY_GAP,
        widest(|_, verb| verb.width()) + VERB_GAP,
    )
}

/// The key table: `columns` side by side, each as wide as its widest key
/// and verb, the last one unpadded and every row trimmed on the right.
fn table_lines(columns: &[Vec<HelpCell>]) -> Vec<Line<'static>> {
    let widths: Vec<(usize, usize)> = columns.iter().map(|column| column_widths(column)).collect();
    let rows = columns.iter().map(Vec::len).max().unwrap_or(0);
    let mut lines = Vec::with_capacity(rows);
    for row in 0..rows {
        let mut spans = vec![Span::raw("  ")];
        for (index, (column, (key_width, verb_width))) in columns.iter().zip(&widths).enumerate() {
            let last = index + 1 == columns.len();
            let cell_width = key_width + verb_width;
            let fill = |text: &str, width: usize| {
                if last {
                    text.to_owned()
                } else {
                    padded(text, width)
                }
            };
            match column.get(row) {
                Some(HelpCell::Header(name)) => {
                    spans.push(Span::styled(fill(name, cell_width), EYEBROW));
                }
                Some(HelpCell::Row { key, verb }) => {
                    spans.push(Span::styled(padded(key, *key_width), DIM));
                    spans.push(Span::raw(fill(verb, *verb_width)));
                }
                Some(HelpCell::Blank) | None => spans.push(Span::raw(fill("", cell_width))),
            }
        }
        while spans
            .last()
            .is_some_and(|span| span.content.trim().is_empty())
        {
            spans.pop();
        }
        if let Some(last) = spans.last_mut() {
            last.content = last.content.trim_end().to_owned().into();
        }
        lines.push(Line::from(spans));
    }
    lines
}

/// Cells the widest row of the key table for `columns` takes.
fn table_width(columns: &[Vec<HelpCell>]) -> usize {
    table_lines(columns)
        .iter()
        .map(Line::width)
        .max()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::tui::testing::{text, texts};

    /// The key cell of every row in `columns`, in reading order.
    fn shown_keys(columns: &[Vec<HelpCell>]) -> Vec<String> {
        columns
            .iter()
            .flatten()
            .filter_map(|cell| match cell {
                HelpCell::Row { key, .. } => Some(key.clone()),
                _ => None,
            })
            .collect()
    }

    /// Every binding's key as the help cells it: joined bindings as their
    /// one cell, the rest as themselves.
    fn bound_keys() -> Vec<String> {
        let joined = |key: &str| JOINED.iter().find(|set| set.contains(&key));
        keymap::BINDINGS
            .iter()
            .filter_map(|binding| match joined(binding.key) {
                Some(set) if set[0] != binding.key => None,
                Some(set) => Some(help_cell(set).0),
                None => Some(binding.key.to_owned()),
            })
            .collect()
    }

    /// The table rows of `layout`: the lines after the leading blank, as
    /// many as the tallest column.
    fn table_rows(layout: &HelpLayout) -> Vec<String> {
        let rows = layout.columns.iter().map(Vec::len).max().unwrap_or(0);
        layout.lines().iter().skip(1).take(rows).map(text).collect()
    }

    /// The card's inner width: what its border leaves of `width`.
    fn inner(width: u16) -> usize {
        width.saturating_sub(BORDER_COLUMNS) as usize
    }

    /// The rows of `layout`: a blank, the table, a blank, the closer, and
    /// with the note a blank and the note, inside the border.
    fn expected_help_height(layout: &HelpLayout, with_note: bool) -> u16 {
        let rows = layout.columns.iter().map(Vec::len).max().unwrap_or(0) as u16;
        let note = if with_note { 2 } else { 0 };
        1 + rows + 1 + 1 + note + BORDER_ROWS
    }

    #[test]
    fn every_binding_is_in_the_help() {
        for set in JOINED {
            for key in set {
                assert!(keymap::binding(key).is_some(), "{key} is not bound");
            }
        }
        let wide = three_columns_from();
        for width in [wide, wide - 1] {
            let layout = HelpLayout::at(width);
            let shown = shown_keys(&layout.columns);
            let mut bound = bound_keys();
            let mut listed = shown.clone();
            bound.sort_unstable();
            listed.sort_unstable();
            assert_eq!(listed, bound, "the help at {width} and the keymap differ");
            let rendered = table_rows(&layout);
            for key in shown {
                assert!(
                    rendered.iter().any(|line| line.contains(&key)),
                    "{key} is not rendered at {width}"
                );
            }
        }
    }

    #[test]
    fn joined_keys_share_a_cell_and_the_note_closes_the_card() {
        assert_eq!(
            help_cell(&["j/k", "↑/↓"]),
            ("j/k ↑/↓".to_owned(), "move".to_owned())
        );
        assert_eq!(
            help_cell(&["y", "Y"]),
            ("y/Y".to_owned(), "copy path / id".to_owned())
        );
        let layout = HelpLayout::at(three_columns_from());
        let rows = table_rows(&layout);
        assert!(
            rows.iter().any(|line| line.contains("MOVE")
                && line.contains("MODEL")
                && line.contains("SHELF"))
        );
        assert!(rows.iter().any(|line| line.starts_with("  SCREEN")));
        assert!(rows.iter().any(|line| line.contains("launch a harness")));
        assert!(rows.iter().any(|line| line.contains("esc       collapse")));
        assert!(rows.iter().any(|line| line.contains("chat in terminal")));
        assert!(
            rows.iter()
                .any(|line| line.contains("y/Y   copy path / id"))
        );
        for row in &rows {
            assert_eq!(row, row.trim_end(), "{row:?} carries trailing air");
        }
        let rendered = texts(&layout.lines());
        let closer = rendered
            .iter()
            .position(|line| line.trim_end() == " esc close");
        assert_eq!(
            closer,
            Some(rows.len() + 2),
            "the closer is not under the table"
        );
        assert_eq!(rendered.last().map(String::as_str), Some(HELP_NOTE));
        assert!(!HELP_NOTE.ends_with('.'));
        assert!(HELP_NOTE.width() <= layout.inner);
    }

    #[test]
    fn help_columns_never_run_together() {
        let wide = three_columns_from();
        for width in [wide, wide - 1] {
            let layout = HelpLayout::at(width);
            for (row, shown) in table_rows(&layout).iter().enumerate() {
                let mut offset = 2;
                for column in &layout.columns {
                    let (key_width, verb_width) = column_widths(column);
                    if let Some(HelpCell::Row { key, verb }) = column.get(row) {
                        let before: String = shown.chars().take(offset).collect();
                        let cell: String = shown.chars().skip(offset).collect();
                        assert!(
                            before.ends_with("  "),
                            "{key} has no gutter before it in {shown:?}"
                        );
                        assert!(
                            cell.starts_with(key.as_str()),
                            "{key} is not at {offset} in {shown:?}"
                        );
                        let after_key: String = shown.chars().skip(offset + key.width()).collect();
                        assert!(
                            after_key.starts_with("  "),
                            "{key} runs into {verb} in {shown:?}"
                        );
                    }
                    offset += key_width + verb_width;
                }
            }
        }
    }

    #[test]
    fn the_help_folds_to_two_columns_on_a_narrow_terminal() {
        let wide = three_columns_from();
        // The one literal pin: the layout tripwire, tripped by any change to the keys.
        assert_eq!(wide, 75);
        let three = HelpLayout::at(wide);
        let two = HelpLayout::at(wide - 1);
        assert_eq!(three.columns.len(), 3);
        assert_eq!(two.columns.len(), 2);
        assert_eq!(three.width, card_width(&three.columns));
        assert_eq!(two.width, card_width(&two.columns));
        assert!(two.width < three.width);
        let narrow = two.lines();
        assert!(narrow.iter().all(|line| line.width() <= inner(two.width)));
        let rendered = texts(&narrow);
        assert!(
            rendered
                .iter()
                .any(|line| line.contains("MOVE") && line.contains("MODEL"))
        );
        assert!(
            rendered
                .iter()
                .any(|line| line.contains("SCREEN") && !line.contains("MOVE"))
        );
        assert!(
            rendered
                .iter()
                .any(|line| line.starts_with("  q ") && line.contains("SHELF"))
        );
        assert!(
            !rendered
                .iter()
                .any(|line| line.contains("hedos subcommand"))
        );
        assert_eq!(
            rendered.last().map(|line| line.trim_end()),
            Some(" esc close")
        );
        assert!(two.height() > three.height());
        assert_eq!(three.height(), expected_help_height(&three, true));
        assert_eq!(two.height(), expected_help_height(&two, false));
    }
}
