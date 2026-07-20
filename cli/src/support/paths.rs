//! Filesystem-path helpers shared by the media commands.

use std::path::PathBuf;

/// A default output path in the current directory: `text` slugged plus an
/// extension (`speak`/`image` use this on the prompt/text when `-o` is omitted).
/// Runs of non-alphanumerics collapse to a single `-`, and the slug is capped so a
/// long prompt still yields a short filename, e.g. `"elon ma"` becomes `elon-ma`.
pub fn default_path(text: &str, extension: &str) -> PathBuf {
    const MAX: usize = 40;
    let mut slug = String::new();
    let mut pending_dash = false;
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            if pending_dash && !slug.is_empty() {
                slug.push('-');
            }
            pending_dash = false;
            slug.push(ch.to_ascii_lowercase());
            if slug.len() >= MAX {
                break;
            }
        } else {
            pending_dash = true;
        }
    }
    let base = if slug.is_empty() { "output" } else { &slug };
    PathBuf::from(format!("{base}.{extension}"))
}
