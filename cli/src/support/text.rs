//! Text cut to a width in terminal cells, for a value that must fit a column
//! or a row: cells rather than characters or bytes, so a wide script and a
//! multi-codepoint grapheme take the room they draw in and no more. The few
//! labels every surface writes the same way live here too, since the shelf is
//! not the only thing that draws them.

use std::borrow::Cow;

use kernel::records::byte_format::{BYTES_PER_GIB, one_decimal};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// `text` cut to `width` cells by dropping its middle, so a path keeps both
/// its root and its file name.
pub fn elide_middle(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_owned();
    }
    let graphemes: Vec<&str> = text.graphemes(true).collect();
    if width < 5 {
        return take_cells(graphemes.iter().copied(), width);
    }
    let head = take_cells(graphemes.iter().copied(), (width - 1) / 2);
    let tail = take_cells(graphemes.iter().rev().copied(), width - 1 - head.width());
    let tail: String = tail.graphemes(true).rev().collect();
    format!("{head}…{tail}")
}

/// `text` cut to `width` cells from the tail, with `…` where it was cut, for
/// a value whose start carries the meaning.
pub fn clip(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_owned();
    }
    if width < 2 {
        return take_cells(text.graphemes(true), width);
    }
    format!(
        "{}…",
        take_cells(text.graphemes(true), width - 1).trim_end()
    )
}

/// Bytes in gibibytes with one decimal, for memory figures set against a
/// machine total: `14.2`. Negative counts read as zero.
pub fn gib(bytes: i64) -> String {
    one_decimal(bytes.max(0) as f64 / BYTES_PER_GIB as f64)
}

/// A count with a noun that takes a plain `s` plural: `1 model`, `12 models`.
pub fn count(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

/// A runtime id as the shelf shows it: the sidecar prefix and long vendor
/// names carry nothing at a glance.
pub fn short_runtime(id: &str) -> &str {
    match id {
        "apple-foundation" => "apple",
        other => other.strip_prefix("python:").unwrap_or(other),
    }
}

/// `text` padded with spaces to `width` terminal cells; a wide glyph counts
/// for two, where `{:<width$}` would count it once and leave the column
/// ragged.
pub fn padded(text: &str, width: usize) -> String {
    let pad = width.saturating_sub(text.width());
    format!("{text}{}", " ".repeat(pad))
}

/// `text` right-aligned in `width` cells, counted the same way.
pub fn right_aligned(text: &str, width: usize) -> String {
    let pad = width.saturating_sub(text.width());
    format!("{}{text}", " ".repeat(pad))
}

/// `text` with each character that could mislead the terminal written out as
/// its escape, so a name or path read from disk cannot color the terminal,
/// move the cursor, split a row or reorder what follows: control characters
/// (`\n`, `\u{1b}`), the bidirectional controls (`\u{202e}` and the rest of
/// U+202A to U+202E, U+2066 to U+2069, U+200E, U+200F, U+061C), and the line
/// and paragraph separators (`\u{2028}`, `\u{2029}`). A backslash is
/// doubled, so an escape printed always stands for the character it names.
pub fn printable(text: &str) -> Cow<'_, str> {
    if !text.chars().any(misleading) {
        return Cow::Borrowed(text);
    }
    let mut escaped = String::with_capacity(text.len() + 8);
    for character in text.chars() {
        if misleading(character) {
            escaped.extend(character.escape_debug());
        } else {
            escaped.push(character);
        }
    }
    Cow::Owned(escaped)
}

fn misleading(character: char) -> bool {
    character.is_control()
        || matches!(
            character,
            '\\'
                | '\u{61c}'
                | '\u{200e}'
                | '\u{200f}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2066}'..='\u{2069}'
                | '\u{2028}'
                | '\u{2029}'
        )
}

/// The leading graphemes of `graphemes` that fit in `width` cells.
fn take_cells<'a>(graphemes: impl Iterator<Item = &'a str>, width: usize) -> String {
    let mut used = 0;
    graphemes
        .take_while(|grapheme| {
            let fits = used + grapheme.width() <= width;
            if fits {
                used += grapheme.width();
            }
            fits
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn printable_writes_control_characters_out_and_leaves_the_rest() {
        assert_eq!(printable("plain 模型-ü"), "plain 模型-ü");
        assert!(matches!(printable("plain"), Cow::Borrowed(_)));
        assert_eq!(
            printable("evil\u{1b}[31mred\nline\ttab"),
            "evil\\u{1b}[31mred\\nline\\ttab"
        );
    }

    #[test]
    fn printable_writes_bidi_controls_separators_and_backslashes_out() {
        assert_eq!(printable("j\u{202e}owt.gguf"), "j\\u{202e}owt.gguf");
        assert_eq!(
            printable("a\u{2066}b\u{2069}c\u{200f}d\u{61c}e"),
            "a\\u{2066}b\\u{2069}c\\u{200f}d\\u{61c}e"
        );
        assert_eq!(
            printable("one\u{2028}two\u{2029}"),
            "one\\u{2028}two\\u{2029}"
        );
        assert_eq!(printable("a\\nb"), "a\\\\nb");
        assert_ne!(printable("a\\nb"), printable("a\nb"));
    }
}
