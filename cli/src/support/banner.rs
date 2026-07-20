//! The `hedos` help banner: the koala wordmark (the same web-hero 36×39 bitmap,
//! rendered as braille dots at 2×4 pixels per cell so the whole animal fits in ten
//! rows) beside a short identity panel. Shown atop `hedos --help`.

/// The banner printed before the generated help. Plain UTF-8 (braille + text, no
/// ANSI) so it survives `NO_COLOR`, redirection, and clap's own color handling
/// untouched. Widest line is 65 columns, so it never wraps an 80-column terminal.
pub const BANNER: &str = "
  ⠀⠀⠀⢰⢊⡩⢍⠑⠤⠔⠒⠢⠔⢉⠭⣉⠱⡀
  ⠀⠀⠀⣇⢸⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⢸⠀⡇   hedos
  ⠀⠀⠀⠐⠦⣁⠀⠰⠆⡠⠤⡀⠶⠀⢀⣡⠜⠁   ἕδος — a seat, an abode, a foundation
  ⠀⠀⠀⠀⢀⠤⢃⠄⠀⢧⣠⠇⠀⠀⡜⠀⢠⢤
  ⠀⠀⢀⠔⠁⠀⠈⠑⠤⣄⣀⣠⡤⠊⠀⢀⠇⡸   A headless engine for the local models
  ⢀⠔⠁⠀⠀⠀⠀⠀⠀⠀⠀⢣⡙⠲⠤⡞⠀⡇   already on your machine. It finds them,
  ⡎⠀⠀⠀⢠⠔⠒⠒⠲⢄⠀⠀⠈⠒⠒⢳⣸⠁   installs new ones, and serves each through
  ⢱⠀⠀⠀⠀⠀⠀⠀⠀⠈⠓⡖⠒⠤⠥⢤⡬⠃   the runtime that actually fits.
  ⠈⠒⢄⣀⣀⠀⠀⠀⠀⠀⠸⠥⡀⢰⠁⡞⠀⠀
  ⠀⠀⠀⠀⠀⠉⠉⠑⠒⠤⠤⠤⠃⠀⠉⠀⠀⠀     ";
