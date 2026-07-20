//! Filesystem-path helpers shared by the media commands.

use std::path::PathBuf;

/// A default output path in the current directory: the slugged model name plus
/// an extension (`speak`/`image` use this when `-o` is omitted).
pub fn default_path(name: &str, extension: &str) -> PathBuf {
    let slug: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let trimmed = slug.trim_matches('-');
    let base = if trimmed.is_empty() {
        "output"
    } else {
        trimmed
    };
    PathBuf::from(format!("{base}.{extension}"))
}
