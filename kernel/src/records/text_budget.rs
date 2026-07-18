//! Clipping text to a byte budget without splitting a UTF-8 character.

/// The result of clipping text to a byte cap.
#[derive(Debug, PartialEq, Eq)]
pub struct Clip<'a> {
    /// The kept prefix, never longer than the cap and always char-aligned.
    pub kept: &'a str,
    /// Whether the text was longer than the cap and had to be trimmed.
    pub overflowed: bool,
    /// The full UTF-8 byte length of the original text.
    pub total: usize,
}

/// Clip `text` so its UTF-8 length does not exceed `cap` bytes, trimming back to
/// the nearest character boundary rather than splitting a multi-byte character.
pub fn clip(text: &str, cap: usize) -> Clip<'_> {
    let total = text.len();
    if total <= cap {
        return Clip {
            kept: text,
            overflowed: false,
            total,
        };
    }
    let mut end = cap;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    Clip {
        kept: &text[..end],
        overflowed: true,
        total,
    }
}
