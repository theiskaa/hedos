//! Breaking text into lines of at most so many cells. Measured the way the
//! terminal draws: per grapheme, by display width, so an emoji with a
//! variation selector is one cell wide and a family emoji is never torn.

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// How many columns a tab advances to; a fixed expansion, since the pane has
/// no tab stops of its own.
const TAB_CELLS: usize = 4;

/// One grapheme with its width and whatever style the caller carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell<S> {
    pub grapheme: String,
    pub width: usize,
    pub style: S,
}

impl<S> Cell<S> {
    fn blank(&self) -> bool {
        self.grapheme == " "
    }
}

/// `text` as cells carrying `style`, tabs expanded to spaces and carriage
/// returns dropped, so the caller's newlines are the only line structure.
pub fn cells<S: Copy>(text: &str, style: S) -> Vec<Cell<S>> {
    let mut cells = Vec::new();
    for grapheme in text.graphemes(true) {
        match grapheme {
            "\t" => cells.extend((0..TAB_CELLS).map(|_| Cell {
                grapheme: " ".to_owned(),
                width: 1,
                style,
            })),
            "\r" => {}
            _ => cells.push(Cell {
                grapheme: grapheme.to_owned(),
                width: grapheme.width(),
                style,
            }),
        }
    }
    cells
}

/// `text` broken into lines no wider than `width` cells: on its own newlines
/// first, then between words, then inside a word longer than a line. Runs of
/// spaces inside a line are kept, since a reply's code keeps its shape
/// through them; only the spaces a break lands on are dropped.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    text.split('\n')
        .flat_map(|paragraph| {
            wrap_cells(cells(paragraph, ()), width)
                .into_iter()
                .map(|line| line.into_iter().map(|cell| cell.grapheme).collect())
        })
        .collect()
}

/// One paragraph of cells broken into lines no wider than `width`, by the
/// rules of [`wrap`]; always at least one line, so a blank paragraph keeps
/// its row.
pub fn wrap_cells<S: Clone>(cells: Vec<Cell<S>>, width: usize) -> Vec<Vec<Cell<S>>> {
    let width = width.max(1);
    let mut lines: Vec<Vec<Cell<S>>> = Vec::new();
    let mut line: Vec<Cell<S>> = Vec::new();
    let mut used = 0;
    let has_text = |line: &[Cell<S>]| line.iter().any(|cell| !cell.blank());
    for mut token in runs(cells) {
        let blank = token.first().is_some_and(Cell::blank);
        while !token.is_empty() {
            let wanted: usize = token.iter().map(|cell| cell.width).sum();
            if used + wanted <= width {
                line.append(&mut token);
                used += wanted;
                break;
            }
            if has_text(&line) {
                trim_end(&mut line);
                lines.push(std::mem::take(&mut line));
                used = 0;
                if blank {
                    break;
                }
                continue;
            }
            // Only indentation so far, or nothing: the word is cut where the
            // line ends, keeping its indent in front of the first piece.
            let mut cut = 0;
            let mut taken = used;
            for cell in &token {
                if taken + cell.width > width {
                    break;
                }
                taken += cell.width;
                cut += 1;
            }
            // A glyph wider than what is left still goes somewhere.
            let cut = cut.max(1);
            let rest = token.split_off(cut);
            line.append(&mut token);
            if rest.is_empty() {
                used = line.iter().map(|cell| cell.width).sum();
                break;
            }
            lines.push(std::mem::take(&mut line));
            used = 0;
            token = rest;
        }
    }
    // Spaces dropped at the last break leave nothing worth a row of its own.
    if lines.is_empty() || has_text(&line) {
        lines.push(line);
    }
    lines
}

/// `cells` as alternating runs of spaces and of anything else.
fn runs<S>(cells: Vec<Cell<S>>) -> Vec<Vec<Cell<S>>> {
    let mut runs: Vec<Vec<Cell<S>>> = Vec::new();
    for cell in cells {
        match runs.last_mut() {
            Some(run) if run.last().is_some_and(|last| last.blank() == cell.blank()) => {
                run.push(cell);
            }
            _ => runs.push(vec![cell]),
        }
    }
    runs
}

fn trim_end<S>(line: &mut Vec<Cell<S>>) {
    while line.last().is_some_and(Cell::blank) {
        line.pop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_breaks_between_words_and_inside_long_ones() {
        assert_eq!(wrap("the quick brown fox", 9), ["the quick", "brown fox"]);
        assert_eq!(wrap("a\n\nb", 5), ["a", "", "b"]);
        assert_eq!(wrap("abcdefgh ij", 3), ["abc", "def", "gh", "ij"]);
        assert_eq!(wrap("", 4), [""]);
        assert_eq!(wrap("xy z", 0), ["x", "y", "z"]);
        assert_eq!(wrap("aa   bb", 4), ["aa", "bb"]);
        assert_eq!(wrap("abcd e", 4), ["abcd", "e"]);
    }

    #[test]
    fn wrap_keeps_indentation_and_counts_cells() {
        assert_eq!(wrap("    fn main()", 20), ["    fn main()"]);
        assert_eq!(wrap("a  b", 10), ["a  b"]);
        assert_eq!(wrap("aa bb", 2), ["aa", "bb"]);
        assert_eq!(wrap("日本語 text", 6), ["日本語", "text"]);
        assert_eq!(wrap("日本", 1), ["日", "本"]);
    }

    #[test]
    fn an_indented_long_word_keeps_its_indent_and_adds_no_blank_row() {
        assert_eq!(wrap("    abcdefghij", 8), ["    abcd", "efghij"]);
        assert_eq!(wrap("a   ", 2), ["a"]);
        assert_eq!(wrap("   ", 2), ["  "]);
    }

    #[test]
    fn tabs_expand_and_returns_vanish() {
        assert_eq!(wrap("\tx\r\ny", 10), ["    x", "y"]);
    }

    #[test]
    fn graphemes_are_measured_as_the_terminal_draws_them() {
        assert_eq!(wrap("a ❤️❤️❤️", 6), ["a", "❤️❤️❤️"]);
        assert_eq!(wrap("a ❤️❤️❤️", 5), ["a", "❤️❤️", "❤️"]);
        assert_eq!(wrap("xx👨‍👩‍👧yy", 4), ["xx👨‍👩‍👧", "yy"]);
        assert_eq!(wrap("e\u{301}x", 1), ["e\u{301}", "x"]);
    }
}
