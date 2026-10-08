//! Display type made of cells. A terminal has one font, so the loud moments
//! are drawn in blocks: a cell is about twice as tall as it is wide, so a
//! half block (`▀`, `▄`, `█`) is a square pixel, and a glyph of eight pixel
//! rows takes four terminal rows.

use ratatui::buffer::Buffer;
use ratatui::style::Color;

/// A font: each glyph is rows of pixels, `X` lit, every row the same width,
/// an even number of rows.
type Font = fn(char) -> Option<&'static [&'static str]>;

/// The wordmark's lowercase letters, 5×8 with two-pixel stems, the last row
/// blank.
fn word(letter: char) -> Option<&'static [&'static str]> {
    Some(match letter {
        'h' => &[
            "XX...", "XX...", "XXXXX", "XX.XX", "XX.XX", "XX.XX", "XX.XX", ".....",
        ],
        'e' => &[
            ".....", ".....", "XXXXX", "XX.XX", "XXXXX", "XX...", "XXXXX", ".....",
        ],
        'd' => &[
            "...XX", "...XX", "XXXXX", "XX.XX", "XX.XX", "XX.XX", "XXXXX", ".....",
        ],
        'o' => &[
            ".....", ".....", "XXXXX", "XX.XX", "XX.XX", "XX.XX", "XXXXX", ".....",
        ],
        's' => &[
            ".....", ".....", "XXXXX", "XX...", "XXXXX", "...XX", "XXXXX", ".....",
        ],
        _ => return None,
    })
}

/// Digits at 3×5, the last row blank, three terminal rows tall.
fn digit(numeral: char) -> Option<&'static [&'static str]> {
    Some(match numeral {
        '0' => &["XXX", "X.X", "X.X", "X.X", "XXX", "..."],
        '1' => &[".X.", "XX.", ".X.", ".X.", "XXX", "..."],
        '2' => &["XXX", "..X", "XXX", "X..", "XXX", "..."],
        '3' => &["XXX", "..X", "XXX", "..X", "XXX", "..."],
        '4' => &["X.X", "X.X", "XXX", "..X", "..X", "..."],
        '5' => &["XXX", "X..", "XXX", "..X", "XXX", "..."],
        '6' => &["XXX", "X..", "XXX", "X.X", "XXX", "..."],
        '7' => &["XXX", "..X", "..X", "..X", "..X", "..."],
        '8' => &["XXX", "X.X", "XXX", "X.X", "XXX", "..."],
        '9' => &["XXX", "X.X", "XXX", "..X", "XXX", "..."],
        _ => return None,
    })
}

/// The wordmark's font.
pub(crate) const WORD: Font = word;
/// The figures' font.
pub(crate) const DIGITS: Font = digit;

/// One column of pixels, top to bottom; `None` is the gap between glyphs.
pub(crate) type Column = Option<Vec<bool>>;

/// `text` in `font` as columns, a blank column of `gap` between glyphs.
/// Characters the font has no glyph for are left out.
pub(crate) fn columns(text: &str, font: Font, gap: usize) -> Vec<Column> {
    let mut columns = Vec::new();
    for glyph in text.chars().filter_map(font) {
        if !columns.is_empty() {
            columns.extend(std::iter::repeat_n(None, gap));
        }
        let width = glyph.first().map_or(0, |row| row.len());
        for x in 0..width {
            columns.push(Some(
                glyph
                    .iter()
                    .map(|row| row.as_bytes().get(x) == Some(&b'X'))
                    .collect(),
            ));
        }
    }
    columns
}

/// How many cells `text` takes in `font`.
pub(crate) fn width(text: &str, font: Font, gap: usize) -> u16 {
    columns(text, font, gap).len() as u16
}

/// How many terminal rows a glyph of `font` takes.
#[cfg(test)]
pub(crate) fn rows(font: Font) -> u16 {
    font('0')
        .or_else(|| font('h'))
        .map_or(0, |glyph| glyph.len() as u16 / 2)
}

/// Draw `columns` into `buf` from `(x, y)`, each pair of pixel rows as one
/// half-block cell, coloured column by column by `colour_at`; a column it
/// returns `None` for is not drawn yet.
pub(crate) fn draw(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    columns: &[Column],
    colour_at: impl Fn(usize) -> Option<Color>,
) {
    let area = buf.area;
    for (index, column) in columns.iter().enumerate() {
        let Some(column) = column else { continue };
        let Some(colour) = colour_at(index) else {
            continue;
        };
        let cx = x + index as u16;
        for row in 0..column.len() / 2 {
            let top = column[2 * row];
            let bottom = column[2 * row + 1];
            let glyph = match (top, bottom) {
                (true, true) => "█",
                (true, false) => "▀",
                (false, true) => "▄",
                (false, false) => continue,
            };
            let cy = y + row as u16;
            if cx < area.right() && cy < area.bottom() && cx >= area.x && cy >= area.y {
                buf[(cx, cy)].set_symbol(glyph).set_fg(colour);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use ratatui::layout::Rect;

    fn drawn(text: &str, font: Font) -> Vec<String> {
        let cols = columns(text, font, 1);
        let area = Rect::new(0, 0, cols.len() as u16, rows(font));
        let mut buffer = Buffer::empty(area);
        draw(&mut buffer, 0, 0, &cols, |_| Some(Color::White));
        (0..area.height)
            .map(|y| {
                let row: String = (0..area.width).map(|x| buffer[(x, y)].symbol()).collect();
                row.trim_end().to_owned()
            })
            .collect()
    }

    /// The one literal pin: the wordmark as the mock draws it.
    #[test]
    fn the_wordmark_is_four_rows_of_half_blocks() {
        assert_eq!(width("hedos", WORD, 1), 29);
        assert_eq!(rows(WORD), 4);
        assert_eq!(
            drawn("hedos", WORD),
            [
                "██             ██",
                "██▀██ ██▀██ ██▀██ ██▀██ ██▀▀▀",
                "██ ██ ██▀▀▀ ██ ██ ██ ██ ▀▀▀██",
                "▀▀ ▀▀ ▀▀▀▀▀ ▀▀▀▀▀ ▀▀▀▀▀ ▀▀▀▀▀",
            ]
        );
    }

    #[test]
    fn a_digit_is_three_columns_by_three_rows() {
        assert_eq!(rows(DIGITS), 3);
        for numeral in '0'..='9' {
            assert_eq!(width(&numeral.to_string(), DIGITS, 1), 3);
        }
        assert_eq!(width("15", DIGITS, 1), 7);
        assert_eq!(
            width("x1", DIGITS, 1),
            3,
            "a character without a glyph is left out"
        );
    }

    #[test]
    fn a_column_not_due_yet_is_not_drawn() {
        let cols = columns("1", DIGITS, 1);
        let mut buffer = Buffer::empty(Rect::new(0, 0, 3, 3));
        draw(&mut buffer, 0, 0, &cols, |index| {
            (index != 1).then_some(Color::White)
        });
        assert_eq!(buffer[(1, 0)].symbol(), " ");
        assert_ne!(buffer[(0, 2)].symbol(), " ");
    }
}
