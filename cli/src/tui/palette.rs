//! The style vocabulary every screen draws from. It lives apart from the
//! panes because the bench draws with it too, from a plain command as well
//! as from the shelf, and a row must read the same wherever it is drawn.
//!
//! The screen is warm greys on the terminal's own ground, and the accent is
//! brightness, never a hue: what has focus or is moving is the
//! brightest thing on the screen. Hierarchy comes from the greys: `BOLD` and
//! `INK` the loud register, `SOFT` figures and facts beside a value, `DIM`
//! labels, verbs and absences. Hues carry state and nothing else: `WARM` for
//! what is loaded or up, `CAUTION` for a tight fit or a stop, `FAILED` for
//! what failed. `BACKDROP` flattens the screen behind a card and
//! `SELECTED_ROW` lifts the selected row of a list one step of grey.
//!
//! Every colour is a fixed `Rgb` chosen against [`PAPER`], a near-black
//! ground, so blends and fades have a colour to start from. The finished
//! frame is handed to the terminal by [`onto_ground`]: the ground itself is
//! left unpainted, the greys keep their places on the ramp from whatever
//! ground the terminal has, and on a light terminal every colour turns its
//! lightness over. [`quantize`] then maps a frame onto the 256-colour palette
//! for a terminal that has no more, and [`onto_terminal_ground`] hands the
//! loudest greys back to the terminal where the bench draws inline.

use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier, Style};
use std::collections::HashMap;

use terminal_colorsaurus::{QueryOptions, background_color};

/// The ground every pane draws over and every blend starts from. It is never
/// painted: [`onto_ground`] gives its cells back to the terminal.
pub(crate) const PAPER: Color = Color::Rgb(PAPER_LEVEL, PAPER_LEVEL, PAPER_LEVEL);
/// Every channel of [`PAPER`].
const PAPER_LEVEL: u8 = 14;
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
/// The screen behind a card: its ink flattened toward the ground, and the
/// near black the flattening blends its fills toward.
pub(crate) const BACKDROP_INK: Color = Color::Rgb(42, 42, 41);
pub(crate) const BACKDROP_GROUND: Color = Color::Rgb(9, 9, 9);

/// Every cell before a pane draws: ink on the ground.
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
/// so over [`GROUND`] it reads in ink and where the bench draws inline, in
/// the terminal's own foreground.
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
/// The screen behind a card: every colour and emphasis flattened toward the
/// ground so the card is the only thing lit.
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
/// The bar heights a cell can take, in eighths: a sparkline's hour, a
/// column of the header's pulse.
pub(crate) const LEVELS: [&str; 9] = [" ", "▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];
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
/// for the bench drawn inline, where nothing reads the terminal's colours:
/// ink on a light terminal would vanish where its own foreground reads.
pub(crate) fn onto_terminal_ground(buffer: &mut Buffer) {
    for cell in &mut buffer.content {
        if matches!(cell.fg, BRIGHT | INK_COLOR) {
            cell.fg = Color::Reset;
        }
    }
}

/// The terminal's own ground, read once at launch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Ground {
    /// Nothing said what the terminal's ground is, so the UI paints [`PAPER`]
    /// itself, the ground every colour was chosen against, and reads the same
    /// on a terminal of any colour.
    #[default]
    Painted,
    /// The terminal's ground, drawn on and never painted: whether it is
    /// light, and its colour when the terminal said what it is.
    Terminal {
        light: bool,
        rgb: Option<(u8, u8, u8)>,
    },
}

impl Ground {
    /// The terminal's ground. The terminal is asked for its background; its
    /// lightness says light or dark unless `HEDOS_THEME` names one, and a
    /// terminal that does not answer gets the painted ground, or with
    /// `HEDOS_THEME` set, the named one of unknown colour. The answer arrives
    /// as input, so this runs before the UI reads keys; whatever is queued
    /// once it returns is dropped, and a reply too late even for that is
    /// left out of the keys by the input thread.
    pub(crate) fn detect() -> Self {
        let named = Self::from_theme(std::env::var("HEDOS_THEME").ok().as_deref());
        let asked = background_color(QueryOptions::default()).ok();
        crate::support::tty::discard_input();
        match (named, asked) {
            (named, Some(color)) => Self::Terminal {
                light: named.unwrap_or(color.perceived_lightness() > 0.5),
                rgb: Some(color.scale_to_8bit()),
            },
            (Some(light), None) => Self::Terminal { light, rgb: None },
            (None, None) => Self::Painted,
        }
    }

    /// `dark` or `light` as `HEDOS_THEME` spells it, or `None` for anything
    /// else, which leaves the terminal to say.
    fn from_theme(value: Option<&str>) -> Option<bool> {
        match value?.trim().to_ascii_lowercase().as_str() {
            "light" => Some(true),
            "dark" => Some(false),
            _ => None,
        }
    }
}

/// How hard a light terminal darkens what was drawn for a dark one: the
/// turned lightness is raised to this, so a label keeps the contrast with a
/// pale ground that it had with a dark one.
const INK_CURVE: f32 = 1.25;

/// The widest a colour's channels may spread and still count as a grey.
const GREY_SPREAD: u8 = 12;

/// `buffer`, drawn over [`PAPER`], handed to the terminal's `ground`; a
/// painted ground keeps the frame as it was drawn. A background at or under
/// the paper is the ground itself and is left unpainted. Every other colour,
/// foreground and background alike, goes through [`onto`], so a fill's
/// half-block edge, drawn as a foreground, still meets its body. A frame
/// holds few distinct colours, so each is worked out once.
pub(crate) fn onto_ground(buffer: &mut Buffer, ground: Ground) {
    let Ground::Terminal { light, rgb } = ground else {
        return;
    };
    let mut seen: HashMap<Color, Color> = HashMap::new();
    let mut map = |color: Color| *seen.entry(color).or_insert_with(|| onto(color, light, rgb));
    for cell in &mut buffer.content {
        cell.bg = if is_ground(cell.bg) {
            Color::Reset
        } else {
            map(cell.bg)
        };
        cell.fg = map(cell.fg);
    }
}

/// Whether `color` is the ground: a grey at or under the paper, which the
/// backdrop and the fades blend toward.
fn is_ground(color: Color) -> bool {
    let Color::Rgb(red, green, blue) = color else {
        return false;
    };
    let high = red.max(green).max(blue);
    high - red.min(green).min(blue) <= GREY_SPREAD && high <= PAPER_LEVEL
}

/// `color`, chosen against [`PAPER`], as it reads on the terminal's ground,
/// channel by channel so a colour blending between a grey and a hue changes
/// smoothly. On a dark ground lighter than the paper, the ramp from the
/// paper to white is laid from the terminal's ground to white instead, so a
/// border or a fill stands as far off it as it did off the paper; a darker
/// ground leaves every colour as it is, already standing further off it. On
/// a light ground every colour turns its lightness over, keeping its hue,
/// and is laid on the ramp down from the terminal's ground when that is
/// known.
fn onto(color: Color, light: bool, rgb: Option<(u8, u8, u8)>) -> Color {
    let paper = f32::from(PAPER_LEVEL);
    let level = |value: f32| value.round().clamp(0.0, 255.0) as u8;
    match (color, light, rgb) {
        (Color::Rgb(red, green, blue), false, Some(base)) => {
            let lift = |value: u8, ground: u8| {
                let ground = f32::from(ground.max(PAPER_LEVEL));
                let value = f32::from(value);
                if value >= paper {
                    level(ground + (value - paper) * (255.0 - ground) / (255.0 - paper))
                } else {
                    level(ground * value / paper)
                }
            };
            Color::Rgb(lift(red, base.0), lift(green, base.1), lift(blue, base.2))
        }
        (Color::Rgb(..), true, Some(base)) => {
            let (Color::Rgb(red, green, blue), Color::Rgb(turned_paper, _, _)) =
                (turned(color), turned(PAPER))
            else {
                return color;
            };
            let laid = |value: u8, ground: u8| {
                level(f32::from(value) * f32::from(ground) / f32::from(turned_paper))
            };
            Color::Rgb(laid(red, base.0), laid(green, base.1), laid(blue, base.2))
        }
        (Color::Rgb(..), true, None) => turned(color),
        _ => color,
    }
}

/// `color` with its lightness turned over and raised to [`INK_CURVE`], its
/// hue and saturation kept; anything but an `Rgb` is left to the terminal.
fn turned(color: Color) -> Color {
    let Color::Rgb(red, green, blue) = color else {
        return color;
    };
    let [r, g, b] = [red, green, blue].map(|channel| f32::from(channel) / 255.0);
    let high = r.max(g).max(b);
    let low = r.min(g).min(b);
    let lightness = (high + low) / 2.0;
    let target = (1.0 - lightness).powf(INK_CURVE);
    let spread = high - low;
    if spread <= f32::EPSILON {
        let value = (target * 255.0).round() as u8;
        return Color::Rgb(value, value, value);
    }
    let saturation = spread / (1.0 - (2.0 * lightness - 1.0).abs());
    let hue = if high == r {
        ((g - b) / spread).rem_euclid(6.0)
    } else if high == g {
        (b - r) / spread + 2.0
    } else {
        (r - g) / spread + 4.0
    };
    let chroma = (1.0 - (2.0 * target - 1.0).abs()) * saturation;
    let second = chroma * (1.0 - (hue % 2.0 - 1.0).abs());
    let (r, g, b) = match hue as u8 {
        0 => (chroma, second, 0.0),
        1 => (second, chroma, 0.0),
        2 => (0.0, chroma, second),
        3 => (0.0, second, chroma),
        4 => (second, 0.0, chroma),
        _ => (chroma, 0.0, second),
    };
    let base = target - chroma / 2.0;
    let channel = |value: f32| ((value + base).clamp(0.0, 1.0) * 255.0).round() as u8;
    Color::Rgb(channel(r), channel(g), channel(b))
}

/// The xterm-256 index nearest `color`, or `color` itself when it is not an
/// `Rgb`.
fn indexed(color: Color) -> Color {
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
    }

    const DARK: Ground = Ground::Terminal {
        light: false,
        rgb: None,
    };
    const LIGHT: Ground = Ground::Terminal {
        light: true,
        rgb: None,
    };
    const ONE_DARK: (u8, u8, u8) = (40, 44, 52);

    /// One cell of `bg` with a half-block of `fg` over it, handed to
    /// `ground`.
    fn handed(fg: Color, bg: Color, ground: Ground) -> (Color, Color) {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 1, 1));
        buffer[(0, 0)].set_symbol("▀").set_fg(fg).set_bg(bg);
        onto_ground(&mut buffer, ground);
        (buffer[(0, 0)].fg, buffer[(0, 0)].bg)
    }

    #[test]
    fn hedos_theme_names_a_ground_or_leaves_it_to_the_terminal() {
        assert_eq!(Ground::from_theme(Some("light")), Some(true));
        assert_eq!(Ground::from_theme(Some(" Dark ")), Some(false));
        assert_eq!(Ground::from_theme(Some("auto")), None);
        assert_eq!(Ground::from_theme(None), None);
    }

    #[test]
    fn a_painted_ground_keeps_the_frame_as_drawn() {
        assert_eq!(
            handed(INK_COLOR, PAPER, Ground::Painted),
            (INK_COLOR, PAPER)
        );
        assert_eq!(Ground::default(), Ground::Painted);
    }

    #[test]
    fn the_ground_and_the_backdrop_are_left_to_the_terminal() {
        let known = Ground::Terminal {
            light: false,
            rgb: Some(ONE_DARK),
        };
        for ground in [DARK, LIGHT, known] {
            assert_eq!(handed(INK_COLOR, PAPER, ground).1, Color::Reset);
            assert_eq!(handed(INK_COLOR, BACKDROP_GROUND, ground).1, Color::Reset);
            let dimmed = mix(SEL, BACKDROP_GROUND, 0.9);
            assert_eq!(handed(INK_COLOR, dimmed, ground).1, Color::Reset);
            assert_eq!(
                handed(Color::Reset, Color::Reset, ground),
                (Color::Reset, Color::Reset)
            );
        }
        assert_eq!(
            handed(INK_COLOR, SEL, DARK),
            (INK_COLOR, SEL),
            "an unknown dark ground keeps every colour"
        );
    }

    #[test]
    fn a_fills_edge_meets_its_body_on_every_ground() {
        let grounds = [
            DARK,
            LIGHT,
            Ground::Terminal {
                light: false,
                rgb: Some(ONE_DARK),
            },
            Ground::Terminal {
                light: false,
                rgb: Some((30, 30, 30)),
            },
            Ground::Terminal {
                light: true,
                rgb: Some((250, 250, 250)),
            },
        ];
        for ground in grounds {
            for fill in [BUBBLE, BUTTON_GHOST, BUTTON_MUTED, INK_COLOR, SEL] {
                let (fg, bg) = handed(fill, fill, ground);
                assert_eq!(fg, bg, "{fill:?} on {ground:?}");
            }
        }
    }

    #[test]
    fn the_greys_keep_their_step_off_a_lighter_dark_ground() {
        let one_dark = Ground::Terminal {
            light: false,
            rgb: Some(ONE_DARK),
        };
        let (border, _) = handed(LINE, PAPER, one_dark);
        let Color::Rgb(r, g, b) = border else {
            panic!("{border:?}")
        };
        assert!(
            r >= ONE_DARK.0 + 30 && g >= ONE_DARK.1 + 30 && b >= ONE_DARK.2 + 30,
            "a card's border still stands off the ground: {border:?}"
        );
        let (_, selected) = handed(INK_COLOR, SEL, one_dark);
        let Color::Rgb(r, g, b) = selected else {
            panic!("{selected:?}")
        };
        assert!(
            r > ONE_DARK.0 && g > ONE_DARK.1 && b > ONE_DARK.2,
            "{selected:?}"
        );
        let (bright, _) = handed(BRIGHT, PAPER, one_dark);
        assert!(
            matches!(bright, Color::Rgb(r, _, _) if r >= 245),
            "{bright:?}"
        );
        let Color::Rgb(r, g, b) = handed(OLIVE, PAPER, one_dark).0 else {
            unreachable!()
        };
        assert!(g > r && r > b, "olive stays olive: {r} {g} {b}");
        let (fade, _) = handed(PAPER, PAPER, one_dark);
        assert_eq!(
            fade,
            Color::Rgb(40, 44, 52),
            "a fade starts from the ground"
        );
    }

    #[test]
    fn a_ground_darker_than_the_paper_keeps_every_colour() {
        let black = Ground::Terminal {
            light: false,
            rgb: Some((0, 0, 0)),
        };
        for color in [LINE, SEL, RAISED, CODE_GROUND, BUBBLE, OLIVE, INK_COLOR] {
            assert_eq!(handed(color, color, black), (color, color), "{color:?}");
        }
    }

    #[test]
    fn a_hue_fading_toward_a_grey_changes_smoothly() {
        let channels = |color: Color| match color {
            Color::Rgb(r, g, b) => [r, g, b].map(i16::from),
            other => panic!("{other:?}"),
        };
        for ground in [
            Ground::Terminal {
                light: false,
                rgb: Some(ONE_DARK),
            },
            Ground::Terminal {
                light: true,
                rgb: Some((253, 246, 227)),
            },
        ] {
            for hue in [OLIVE, AMBER, RED] {
                let steps: Vec<[i16; 3]> = (0..=64)
                    .map(|step| {
                        channels(
                            handed(mix(hue, BACKDROP_INK, step as f32 / 64.0), PAPER, ground).0,
                        )
                    })
                    .collect();
                for pair in steps.windows(2) {
                    let jump = (0..3).map(|i| (pair[0][i] - pair[1][i]).abs()).max();
                    assert!(jump <= Some(12), "{hue:?} jumps on {ground:?}: {pair:?}");
                }
            }
        }
    }

    #[test]
    fn a_light_ground_turns_the_greys_over_and_keeps_the_hues() {
        let lightness = |color: Color| match color {
            Color::Rgb(r, g, b) => u16::from(r) + u16::from(g) + u16::from(b),
            other => panic!("{other:?}"),
        };
        let ink = |color: Color| handed(color, PAPER, LIGHT).0;
        for (lighter, darker) in [(INK_COLOR, SOFT_COLOR), (SOFT_COLOR, FAINT_COLOR)] {
            assert!(
                lightness(ink(lighter)) < lightness(ink(darker)),
                "{lighter:?} reads louder than {darker:?} on a light ground"
            );
        }
        assert!(lightness(ink(INK_COLOR)) < 120, "ink is near black");
        let Color::Rgb(r, g, b) = ink(OLIVE) else {
            unreachable!()
        };
        assert!(g > r && r > b, "olive stays olive: {r} {g} {b}");
        assert!(lightness(Color::Rgb(r, g, b)) < lightness(OLIVE));
        let white = Ground::Terminal {
            light: true,
            rgb: Some((250, 250, 250)),
        };
        let (fade, selected) = handed(PAPER, SEL, white);
        assert_eq!(
            fade,
            Color::Rgb(250, 250, 250),
            "a fade starts from the ground"
        );
        assert!(
            matches!(selected, Color::Rgb(v, _, _) if (200..250).contains(&v)),
            "a fill sits a step under a white ground: {selected:?}"
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
