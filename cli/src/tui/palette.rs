//! The style vocabulary every screen draws from. It lives apart from the
//! panes because the bench draws with it too, from a plain command as well
//! as from the shelf, and a row must read the same wherever it is drawn.
//!
//! The screen is warm greys on a near-black ground the UI paints itself, and
//! the accent is brightness, never a hue: what has focus or is moving is the
//! brightest thing on the screen. Hierarchy comes from the greys: `BOLD` and
//! `INK` the loud register, `SOFT` figures and facts beside a value, `DIM`
//! labels, verbs and absences. Hues carry state and nothing else: `WARM` for
//! what is loaded or up, `CAUTION` for a tight fit or a stop, `FAILED` for
//! what failed. `BACKDROP` flattens the screen behind a card and
//! `SELECTED_ROW` lifts the selected row of a list one step of grey.
//!
//! Every colour is a fixed `Rgb`, so the screen reads the same on any
//! truecolor terminal; [`quantize`] maps a frame onto the 256-colour palette
//! for a terminal that has no more, and [`onto_terminal_ground`] hands the
//! loudest greys back to the terminal where the UI does not paint its ground.

use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier, Style};

/// The ground the UI paints under everything.
pub(crate) const PAPER: Color = Color::Rgb(14, 14, 14);
/// The ground of a card, a step up from the paper.
pub(crate) const SURFACE: Color = Color::Rgb(19, 19, 19);
/// Your words' bubble in the try screen, and an inline code chip.
pub(crate) const BUBBLE: Color = Color::Rgb(31, 31, 30);
/// A fenced code block's panel, and the ink its code is set in.
pub(crate) const CODE_GROUND: Color = Color::Rgb(24, 24, 24);
pub(crate) const CODE_INK: Color = Color::Rgb(216, 213, 207);
/// A reply's prose, a step under ink so your own words read louder.
pub(crate) const READ: Color = Color::Rgb(201, 198, 192);
/// An input box's border at rest, and while something is typed into it.
pub(crate) const BOX_IDLE: Color = Color::Rgb(62, 62, 62);
pub(crate) const BOX_LIVE: Color = Color::Rgb(107, 105, 100);
/// The chosen chip's ground, and the unfilled part of a filling button.
pub(crate) const CHIP_ON: Color = Color::Rgb(38, 38, 38);
pub(crate) const TRACK_DARK: Color = Color::Rgb(38, 38, 38);
/// A button waiting on something, and one that says why it cannot act.
pub(crate) const BUTTON_GHOST: Color = Color::Rgb(58, 57, 54);
pub(crate) const BUTTON_MUTED: Color = Color::Rgb(28, 28, 28);
/// A chip's ground, a step up from the card.
pub(crate) const RAISED: Color = Color::Rgb(27, 27, 27);
/// A card's border.
pub(crate) const LINE: Color = Color::Rgb(50, 50, 50);
/// The selected row's ground, a step up from the card it sits in.
pub(crate) const SEL: Color = Color::Rgb(34, 34, 34);
/// The track of a bar: the part a figure has not filled.
pub(crate) const LINE_STRONG: Color = Color::Rgb(62, 62, 62);
/// The loud grey: names, values, keys.
pub(crate) const INK_COLOR: Color = Color::Rgb(230, 228, 223);
/// The middle grey: figures and facts beside a value.
pub(crate) const SOFT_COLOR: Color = Color::Rgb(143, 141, 136);
/// The quiet grey: labels, verbs, borders' titles, absences.
pub(crate) const FAINT_COLOR: Color = Color::Rgb(92, 90, 86);
/// A grey barely off the ground: the koala at rest, a quiet glyph.
pub(crate) const GHOST: Color = Color::Rgb(58, 57, 54);
/// Pure white, for a pen tip, a pressed button, a shimmer's peak.
pub(crate) const WHITE: Color = Color::Rgb(255, 255, 255);
/// The wordmark at rest, a step under white so its sweep reads.
pub(crate) const WORD: Color = Color::Rgb(217, 214, 208);
/// The accent: the brightest grey, for what has focus or is in motion.
pub(crate) const BRIGHT: Color = Color::Rgb(247, 245, 241);
/// What the rest of the loaded models hold, in a gauge of one model's fit.
pub(crate) const ACCENT_DIM: Color = Color::Rgb(85, 83, 79);
/// The second and third shades of a bar that has several segments.
pub(crate) const ACCENT_MID: Color = Color::Rgb(161, 158, 152);
pub(crate) const SEGMENT_3: Color = Color::Rgb(108, 106, 102);
/// The state hues: warm or up, tight or stopped, failed.
pub(crate) const OLIVE: Color = Color::Rgb(163, 184, 92);
/// The ends of the gateway dot's pulse.
pub(crate) const OLIVE_HOT: Color = Color::Rgb(207, 224, 138);
pub(crate) const OLIVE_DIM: Color = Color::Rgb(86, 96, 47);
pub(crate) const AMBER: Color = Color::Rgb(217, 165, 74);
pub(crate) const RED: Color = Color::Rgb(224, 100, 79);
/// The screen behind a card: ink and ground flattened nearly to black.
pub(crate) const BACKDROP_INK: Color = Color::Rgb(42, 42, 41);
pub(crate) const BACKDROP_GROUND: Color = Color::Rgb(9, 9, 9);

/// Every cell before a pane draws: ink on the painted ground.
pub(crate) const GROUND: Style = Style::new().fg(INK_COLOR).bg(PAPER);
/// The quiet register: labels, verbs, borders, models that can't run here.
pub(crate) const DIM: Style = Style::new().fg(FAINT_COLOR);
/// Figures and facts beside a value: the runtime and store, a total, a
/// holder.
pub(crate) const SOFT: Style = Style::new().fg(SOFT_COLOR);
/// Ink without emphasis: a title over a border, a value that should read
/// whatever it is drawn over.
pub(crate) const INK: Style = Style::new().fg(INK_COLOR);
/// The loud register: what the eye should land on first. A modifier only,
/// so on the painted ground it reads in ink and on a terminal's own ground,
/// where the bench draws inline, in the terminal's own foreground.
pub(crate) const BOLD: Style = Style::new().add_modifier(Modifier::BOLD);
/// What is in focus, names a mode, or is in motion: brightness, not a hue.
pub(crate) const ACCENT: Style = Style::new().fg(BRIGHT);
/// A heading over a run of rows: column headers, section names, group
/// labels.
pub(crate) const EYEBROW: Style = Style::new().fg(FAINT_COLOR);
/// What is loaded or up: a warm model, a gateway that is on.
pub(crate) const WARM: Style = Style::new().fg(OLIVE);
/// A warning: a tight fit, a reply that was stopped.
pub(crate) const CAUTION: Style = Style::new().fg(AMBER);
/// What failed, and nothing else.
pub(crate) const FAILED: Style = Style::new().fg(RED);
/// The unfilled part of a bar.
pub(crate) const TRACK: Style = Style::new().fg(LINE_STRONG);
/// The selected row of a list: one step of grey under the row, so the text
/// keeps its hues where a reversed row would flatten them. Patched over a
/// row, never set, so a dim row stays dim under it.
pub(crate) const SELECTED_ROW: Style = Style::new().bg(SEL);
/// The screen behind a card: every colour and emphasis flattened to near
/// black so the card is the only thing lit.
pub(crate) const BACKDROP: Style = Style::new()
    .fg(BACKDROP_INK)
    .bg(BACKDROP_GROUND)
    .remove_modifier(Modifier::BOLD);
/// The gutter mark on the selected row of a list, in the cell its leading
/// space took; the one selection signal a terminal without colour keeps.
pub(crate) const SELECTED_MARK: &str = "▌";
/// Rows a bordered block spends on its top and bottom edges.
pub(crate) const BORDER_ROWS: u16 = 2;
/// Columns a bordered block spends on its left and right edges.
pub(crate) const BORDER_COLUMNS: u16 = 2;
/// The glyphs of a horizontal bar: filled, then the track. They are the same
/// thin rule, told apart by colour.
pub(crate) const BAR_FILLED: &str = "━";
pub(crate) const BAR_EMPTY: &str = "━";
/// The text cursor shown while something is being typed.
pub(crate) const CURSOR: &str = "▏";
/// The glyphs of the spinner that turns while something is waited on, one
/// per frame.
pub(crate) const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// The spinner's glyph on tick `ticks`.
pub(crate) fn spinner(ticks: u64) -> &'static str {
    SPINNER[(ticks % SPINNER.len() as u64) as usize]
}

/// `a` moved toward `b` by `t`, in 64 steps so animated cells take few
/// distinct values and the terminal's diff stays small. A colour that is not
/// an `Rgb` is `a` below halfway and `b` from there.
pub(crate) fn mix(a: Color, b: Color, t: f32) -> Color {
    let t = (t.clamp(0.0, 1.0) * 64.0).round() / 64.0;
    match (a, b) {
        (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) => {
            let blend =
                |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
            Color::Rgb(blend(r1, r2), blend(g1, g2), blend(b1, b2))
        }
        _ if t < 0.5 => a,
        _ => b,
    }
}

/// How many colours the terminal shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Depth {
    /// Every `Rgb` as it is.
    #[default]
    True,
    /// The 256-colour palette: every `Rgb` is mapped to its nearest index.
    Indexed,
}

impl Depth {
    /// What the environment says the terminal shows: `COLORTERM` set to
    /// `truecolor` or `24bit` is the only reliable sign of more than 256
    /// colours.
    pub(crate) fn detect() -> Self {
        Self::from_colorterm(std::env::var("COLORTERM").ok().as_deref())
    }

    fn from_colorterm(value: Option<&str>) -> Self {
        match value.map(str::to_ascii_lowercase).as_deref() {
            Some("truecolor" | "24bit") => Self::True,
            _ => Self::Indexed,
        }
    }
}

/// Every `Rgb` in `buffer` mapped to its nearest xterm-256 index, for a
/// terminal that would otherwise guess.
pub(crate) fn quantize(buffer: &mut Buffer) {
    for cell in &mut buffer.content {
        cell.fg = indexed(cell.fg);
        cell.bg = indexed(cell.bg);
    }
}

/// The loud greys in `buffer` handed back to the terminal's own foreground,
/// for a frame drawn on the terminal's ground rather than the UI's: ink on a
/// light terminal would vanish where the terminal's own foreground reads.
pub(crate) fn onto_terminal_ground(buffer: &mut Buffer) {
    for cell in &mut buffer.content {
        if matches!(cell.fg, BRIGHT | INK_COLOR) {
            cell.fg = Color::Reset;
        }
    }
}

/// The xterm-256 index nearest `color`, or `color` itself when it is not an
/// `Rgb`.
fn indexed(color: Color) -> Color {
    // The cards' surface sits five steps over the ground, and both fall
    // nearest one grey of the 256; it takes the next grey up, so a card
    // still stands off the ground.
    if color == SURFACE {
        return Color::Indexed(234);
    }
    let Color::Rgb(red, green, blue) = color else {
        return color;
    };
    Color::Indexed(nearest_index(red, green, blue))
}

/// The levels of each channel of the 6×6×6 cube at indices 16 to 231.
const CUBE: [u8; 6] = [0, 95, 135, 175, 215, 255];

/// The nearest of the cube and the grey ramp (232 to 255, 8 to 238 in steps
/// of 10), by squared distance.
fn nearest_index(red: u8, green: u8, blue: u8) -> u8 {
    let level = |value: u8| {
        (0..CUBE.len())
            .min_by_key(|&index| (i32::from(CUBE[index]) - i32::from(value)).abs())
            .unwrap_or(0)
    };
    let (r, g, b) = (level(red), level(green), level(blue));
    let cube = (CUBE[r], CUBE[g], CUBE[b]);
    let average = (u16::from(red) + u16::from(green) + u16::from(blue)) / 3;
    let step = (average.saturating_sub(3) / 10).min(23) as u8;
    let grey = 8 + 10 * step;
    let distance = |(x, y, z): (u8, u8, u8)| {
        [(x, red), (y, green), (z, blue)]
            .iter()
            .map(|&(a, b)| (i32::from(a) - i32::from(b)).pow(2))
            .sum::<i32>()
    };
    if distance((grey, grey, grey)) < distance(cube) {
        232 + step
    } else {
        16 + 36 * r as u8 + 6 * g as u8 + b as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use ratatui::layout::Rect;

    #[test]
    fn only_a_colorterm_that_says_so_is_truecolor() {
        assert_eq!(Depth::from_colorterm(Some("truecolor")), Depth::True);
        assert_eq!(Depth::from_colorterm(Some("24BIT")), Depth::True);
        assert_eq!(Depth::from_colorterm(Some("256")), Depth::Indexed);
        assert_eq!(Depth::from_colorterm(None), Depth::Indexed);
    }

    /// One literal pin per token, to trip if a token or the mapping drifts.
    #[test]
    fn every_token_lands_on_its_index() {
        for (color, index) in [
            (PAPER, 233),
            (SEL, 235),
            (LINE_STRONG, 237),
            (INK_COLOR, 254),
            (SOFT_COLOR, 245),
            (FAINT_COLOR, 240),
            (BRIGHT, 255),
            (OLIVE, 143),
            (AMBER, 179),
            (RED, 167),
        ] {
            assert_eq!(indexed(color), Color::Indexed(index), "{color:?}");
        }
        assert_eq!(indexed(Color::Reset), Color::Reset);
    }

    #[test]
    fn quantize_maps_every_cell_and_leaves_the_rest() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 2, 1));
        buffer.set_style(Rect::new(0, 0, 1, 1), GROUND);
        quantize(&mut buffer);
        assert_eq!(buffer[(0, 0)].fg, Color::Indexed(254));
        assert_eq!(buffer[(0, 0)].bg, Color::Indexed(233));
        assert_eq!(buffer[(1, 0)].bg, Color::Reset);
        assert_ne!(
            indexed(SURFACE),
            indexed(PAPER),
            "a card stands off the ground"
        );
    }

    #[test]
    fn the_terminal_takes_back_only_the_loud_greys() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 3, 1));
        buffer.set_style(Rect::new(0, 0, 1, 1), ACCENT);
        buffer.set_style(Rect::new(1, 0, 1, 1), GROUND);
        buffer.set_style(Rect::new(2, 0, 1, 1), DIM);
        onto_terminal_ground(&mut buffer);
        assert_eq!(buffer[(0, 0)].fg, Color::Reset);
        assert_eq!(buffer[(1, 0)].fg, Color::Reset);
        assert_eq!(buffer[(2, 0)].fg, FAINT_COLOR);
    }

    #[test]
    fn mix_blends_in_sixty_four_steps() {
        let black = Color::Rgb(0, 0, 0);
        let white = Color::Rgb(255, 255, 255);
        assert_eq!(mix(black, white, 0.0), black);
        assert_eq!(mix(black, white, 1.0), white);
        assert_eq!(mix(black, white, 0.5), Color::Rgb(128, 128, 128));
        assert_eq!(mix(black, white, 0.501), mix(black, white, 0.5));
        assert_eq!(mix(black, white, 2.0), white);
        assert_eq!(mix(Color::Reset, white, 0.4), Color::Reset);
        assert_eq!(mix(Color::Reset, white, 0.6), white);
    }

    #[test]
    fn the_spinner_cycles_through_every_frame() {
        assert_eq!(spinner(0), SPINNER[0]);
        assert_eq!(spinner(SPINNER.len() as u64 + 1), SPINNER[1]);
    }
}
