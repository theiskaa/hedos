//! The koala wordmark (the same web-hero 36×39 bitmap, rendered as braille dots
//! at 2×4 pixels per cell so the whole animal fits in ten rows) and the `--help`
//! banner that sets it beside a short identity panel.

use std::sync::LazyLock;

/// The koala, one row per line, every row the same width.
pub const KOALA: [&str; 10] = [
    "⠀⠀⠀⢰⢊⡩⢍⠑⠤⠔⠒⠢⠔⢉⠭⣉⠱⡀",
    "⠀⠀⠀⣇⢸⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⢸⠀⡇",
    "⠀⠀⠀⠐⠦⣁⠀⠰⠆⡠⠤⡀⠶⠀⢀⣡⠜⠁",
    "⠀⠀⠀⠀⢀⠤⢃⠄⠀⢧⣠⠇⠀⠀⡜⠀⢠⢤",
    "⠀⠀⢀⠔⠁⠀⠈⠑⠤⣄⣀⣠⡤⠊⠀⢀⠇⡸",
    "⢀⠔⠁⠀⠀⠀⠀⠀⠀⠀⠀⢣⡙⠲⠤⡞⠀⡇",
    "⡎⠀⠀⠀⢠⠔⠒⠒⠲⢄⠀⠀⠈⠒⠒⢳⣸⠁",
    "⢱⠀⠀⠀⠀⠀⠀⠀⠀⠈⠓⡖⠒⠤⠥⢤⡬⠃",
    "⠈⠒⢄⣀⣀⠀⠀⠀⠀⠀⠸⠥⡀⢰⠁⡞⠀⠀",
    "⠀⠀⠀⠀⠀⠉⠉⠑⠒⠤⠤⠤⠃⠀⠉⠀⠀⠀",
];

/// The koala's width in terminal cells.
pub const KOALA_WIDTH: u16 = 18;

/// The greeting, ἕδος's gloss, and the one-paragraph pitch set beside the koala.
const IDENTITY: [&str; 10] = [
    "",
    "hedos",
    "ἕδος — a seat, an abode, a foundation",
    "",
    "A headless engine for the local models",
    "already on your machine. It finds them,",
    "installs new ones, and serves each through",
    "the runtime that actually fits.",
    "",
    "",
];

/// The banner printed before the generated help. Plain UTF-8 (braille + text, no
/// ANSI) so it survives `NO_COLOR`, redirection, and clap's own color handling
/// untouched. Widest line is 65 columns, so it never wraps an 80-column terminal.
pub static BANNER: LazyLock<String> = LazyLock::new(|| {
    let rows: Vec<String> = KOALA
        .iter()
        .zip(IDENTITY)
        .map(|(koala, identity)| {
            if identity.is_empty() {
                format!("  {koala}")
            } else {
                format!("  {koala}   {identity}")
            }
        })
        .collect();
    format!("\n\n{}     ", rows.join("\n"))
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_banner_is_the_koala_beside_the_identity() {
        let banner = BANNER.as_str();
        assert!(banner.starts_with("\n\n  "));
        assert!(banner.ends_with("     "));
        assert_eq!(banner.lines().count(), KOALA.len() + 2);
        assert!(banner.contains("   hedos\n"));
    }
}
