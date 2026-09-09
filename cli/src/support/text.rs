//! Text cut to a width in terminal cells, for a value that must fit a column
//! or a row: cells rather than characters or bytes, so a wide script and a
//! multi-codepoint grapheme take the room they draw in and no more. The few
//! labels every surface writes the same way live here too, since the shelf is
//! not the only thing that draws them.

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
