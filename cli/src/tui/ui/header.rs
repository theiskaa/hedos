//! The header. With room, the hero: the koala, the wordmark in pixel type,
//! what hedos is, and the shelf and machine in big figures. Without, one line
//! of numbers.

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use kernel::profiles::FitTally;
use kernel::records::ModelState;

use super::{BOLD, DIM, SOFT, wordmark};
use crate::tui::app::App;
use crate::tui::koala;
use crate::tui::layout::HERO_ROWS;
use crate::tui::motion::Motion;
use crate::tui::palette::{GHOST, INK_COLOR, OLIVE, OLIVE_DIM, OLIVE_HOT, WHITE, WORD, mix};
use crate::tui::pixel::{self, DIGITS, WORD as WORD_FONT};
use crate::tui::text;

/// Where the hero's parts sit, from the header's top-left corner.
const KOALA_AT: (u16, u16) = (2, 1);
const WORD_AT: (u16, u16) = (27, 2);
const TAGLINE_ROW: u16 = 7;
const GLOSS_ROW: u16 = 8;
const DIGITS_ROW: u16 = 3;
const LABEL_ROW: u16 = 7;
const SUB_ROW: u16 = 8;
/// What hedos is, and what ἕδος means.
const TAGLINE: &str = "one home for every local model on your machine";
const GLOSS: &str = "ἕδος · the place where something comes to rest";
/// Columns between two tiles, and between the tiles and the margin.
const TILE_GAP: u16 = 3;
const RIGHT_MARGIN: u16 = 2;
/// The pixel gap between two glyphs of the wordmark and of a figure.
const PIXEL_GAP: usize = 1;

/// When each part arrives, in milliseconds from launch.
const WORD_FROM_MS: u64 = 60;
const WORD_STEP_MS: u64 = 5;
const WORD_FRESH_MS: u64 = 120;
const VERSION_FROM_MS: u64 = 200;
const TAGLINE_FROM_MS: u64 = 150;
const GLOSS_FROM_MS: u64 = 250;
const TAGLINE_PER_MS: u64 = 3;
const TILE_FROM_MS: u64 = 60;
const TILE_STEP_MS: u64 = 30;
/// The figures count up from zero over this long, and ease to a change.
pub(crate) const FIGURES_FROM_MS: u64 = 80;
pub(crate) const FIGURES_MS: u64 = 350;
/// The koala's shine comes round every six seconds; the wordmark's sweep
/// follows it, this far into the cycle, for this long, this wide.
const CYCLE_S: f32 = 6.0;
const INTRO_S: f32 = 1.0;
const SWEEP_AT_S: f32 = 1.9;
const SWEEP_S: f32 = 0.9;
const SWEEP_HALF_WIDTH: f32 = 3.5;
/// The gateway dot's pulse, the rhythm of hedos.ai's live dot.
pub(crate) const PULSE_MS: u64 = 1800;

/// Draw the header into `area`, the hero or one line by its height. The
/// one-line header carries the free memory only when `machine_shown` is
/// false, since the machine card says it.
pub(super) fn draw(frame: &mut Frame, area: Rect, app: &App, machine_shown: bool) {
    if area.height >= HERO_ROWS {
        draw_hero(frame.buffer_mut(), area, app);
    } else {
        frame.render_widget(
            Paragraph::new(summary_line(app, machine_shown, area.width as usize)),
            area,
        );
    }
}

/// The gateway dot's colour now: a pulse from bright to dim, or the warm
/// hue itself when nothing moves.
pub(super) fn pulse(motion: &Motion) -> Color {
    match motion.age(0) {
        Some(age) => {
            let phase = (age * 1000.0 % PULSE_MS as f32) / PULSE_MS as f32;
            mix(OLIVE_DIM, OLIVE_HOT, 1.0 - phase.sqrt())
        }
        None => OLIVE,
    }
}

/// The koala, the wordmark and version, the tagline, and the tiles.
fn draw_hero(buf: &mut Buffer, area: Rect, app: &App) {
    let motion = &app.motion;
    let x0 = area.x;
    let y0 = area.y;
    for (row, cells) in koala::frame(motion).into_iter().enumerate() {
        for (column, cell) in cells.into_iter().enumerate() {
            let (x, y) = (
                x0 + KOALA_AT.0 + column as u16,
                y0 + KOALA_AT.1 + row as u16,
            );
            if let Some(cell) = cell
                && x < area.right()
                && y < area.bottom()
            {
                buf[(x, y)].set_char(cell.glyph).set_fg(cell.fg);
            }
        }
    }

    let word_x = x0 + WORD_AT.0;
    let columns = pixel::columns("hedos", WORD_FONT, PIXEL_GAP);
    let word_width = columns.len() as u16;
    let sweep = sweep_centre(motion, columns.len());
    pixel::draw(buf, word_x, y0 + WORD_AT.1, &columns, |index| {
        let at = WORD_FROM_MS + index as u64 * WORD_STEP_MS;
        if motion.progress(at, 1) < 1.0 {
            return None;
        }
        let fresh = 1.0 - motion.progress(at, WORD_FRESH_MS);
        let shine = sweep.map_or(0.0, |centre| {
            (1.0 - (index as f32 - centre).abs() / SWEEP_HALF_WIDTH).clamp(0.0, 1.0)
        });
        Some(mix(mix(WORD, WHITE, shine * 0.95), GHOST, fresh))
    });
    let version = format!("v{}", env!("CARGO_PKG_VERSION"));
    let version_x = word_x + word_width + 2;
    if motion.progress(VERSION_FROM_MS, 1) >= 1.0 {
        put(buf, area, version_x, y0 + WORD_AT.1 + 3, &version, DIM);
    }

    let floor = version_x + version.width() as u16 + 2;
    let tagline_end = word_x + TAGLINE.width().max(GLOSS.width()) as u16 + 2;
    let right = area.right().saturating_sub(RIGHT_MARGIN);
    let tiles = tiles(app);
    let placed = place(&tiles, floor, tagline_end, right);
    let first_tile = placed.first().map_or(right, |(x, _)| *x);
    let room = first_tile.saturating_sub(word_x + 2) as usize;
    reveal(
        buf,
        area,
        word_x,
        y0 + TAGLINE_ROW,
        TAGLINE,
        room,
        TAGLINE_FROM_MS,
        SOFT,
        motion,
    );
    reveal(
        buf,
        area,
        word_x,
        y0 + GLOSS_ROW,
        GLOSS,
        room,
        GLOSS_FROM_MS,
        DIM,
        motion,
    );
    for (order, (x, index)) in placed.iter().enumerate() {
        if motion.progress(TILE_FROM_MS + order as u64 * TILE_STEP_MS, 1) < 1.0 {
            continue;
        }
        draw_tile(buf, area, *x, y0, &tiles[*index], app);
    }
}

/// Where the wordmark's sweep is, in columns, while it runs.
fn sweep_centre(motion: &Motion, columns: usize) -> Option<f32> {
    let age = motion.age(0)?;
    if age <= INTRO_S {
        return None;
    }
    let cycle = (age - INTRO_S) % CYCLE_S;
    (SWEEP_AT_S..=SWEEP_AT_S + SWEEP_S + 0.2)
        .contains(&cycle)
        .then(|| (cycle - SWEEP_AT_S) / SWEEP_S * (columns as f32 + 12.0) - 6.0)
}

/// `text` from `(x, y)` in `style`, held inside `area`.
fn put(buf: &mut Buffer, area: Rect, x: u16, y: u16, text: &str, style: Style) {
    if y >= area.bottom() || x >= area.right() {
        return;
    }
    let room = (area.right() - x) as usize;
    buf.set_stringn(x, y, text, room, style);
}

/// `text`, clipped to `room`, revealed letter by letter from `from_ms`.
#[allow(clippy::too_many_arguments)]
fn reveal(
    buf: &mut Buffer,
    area: Rect,
    x: u16,
    y: u16,
    text: &str,
    room: usize,
    from_ms: u64,
    style: Style,
    motion: &Motion,
) {
    let clipped = text::clip(text, room);
    let mut cx = x;
    for (piece, ramp) in motion.reveal(&clipped, from_ms, TAGLINE_PER_MS) {
        let style = if ramp { DIM } else { style };
        put(buf, area, cx, y, &piece, style);
        cx += piece.width() as u16;
    }
}

/// What a tile shows in its big type.
#[derive(Debug, Clone, PartialEq)]
enum Figure {
    /// A count, eased from its slot in the app's header figures.
    Count { slot: usize, target: u64 },
    /// The gateway's dot and port, or that it is off.
    Gateway(Option<u16>),
}

/// One of the hero's figures with its label and the line under that.
#[derive(Debug, Clone, PartialEq)]
struct Tile {
    label: &'static str,
    figure: Figure,
    sub: String,
    /// Whether a non-zero figure reads quieter: gone models are a note, not
    /// a boast.
    quiet: bool,
}

impl Tile {
    /// Cells the tile takes: the widest of its figure, label and sub-label.
    fn width(&self) -> u16 {
        let figure = match &self.figure {
            Figure::Count { target, .. } => pixel::width(&target.to_string(), DIGITS, PIXEL_GAP),
            Figure::Gateway(port) => gateway_lines(*port)
                .0
                .width()
                .max(gateway_lines(*port).1.width()) as u16,
        };
        figure
            .max(self.label.width() as u16)
            .max(self.sub.width() as u16)
    }
}

/// The two lines of the gateway tile's figure: its state and how to reach
/// it, or that it is off and how to start it.
fn gateway_lines(port: Option<u16>) -> (String, String) {
    match port {
        Some(port) => ("● on".to_owned(), format!(":{port}")),
        None => ("○ off".to_owned(), "S serve".to_owned()),
    }
}

/// The tiles, left to right: models, warm, gone, free, gateway.
fn tiles(app: &App) -> Vec<Tile> {
    let figures = figures(app);
    let held = app.facts.resident_bytes();
    let stores = app
        .records
        .iter()
        .filter(|record| record.state != ModelState::Missing)
        .map(|record| record.source.kind.as_str())
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    let memory = text::gib(app.facts.memory_bytes as i64);
    let gateway = app.facts.gateway_port;
    vec![
        Tile {
            label: "MODELS",
            figure: Figure::Count {
                slot: 0,
                target: figures[0],
            },
            sub: text::count(stores, "store"),
            quiet: false,
        },
        Tile {
            label: "WARM",
            figure: Figure::Count {
                slot: 1,
                target: figures[1],
            },
            sub: if held > 0 {
                format!("{} GiB", text::gib(held))
            } else {
                "none held".to_owned()
            },
            quiet: false,
        },
        Tile {
            label: "GONE",
            figure: Figure::Count {
                slot: 2,
                target: figures[2],
            },
            sub: "missing".to_owned(),
            quiet: true,
        },
        Tile {
            label: "FREE",
            figure: Figure::Count {
                slot: 3,
                target: figures[3],
            },
            sub: format!("GiB of {}", memory.trim_end_matches(".0")),
            quiet: false,
        },
        Tile {
            label: "GATEWAY",
            figure: Figure::Gateway(gateway),
            sub: gateway.map_or_else(String::new, |_| {
                format!("{} req/min", app.facts.activity.requests_last_minute)
            }),
            quiet: false,
        },
    ]
}

/// The counts the tiles show: models, warm, gone, and free GiB.
pub(crate) fn figures(app: &App) -> [u64; 4] {
    let warm = app
        .records
        .iter()
        .filter(|record| app.facts.is_warm(&record.id))
        .count();
    let gone = app
        .records
        .iter()
        .filter(|record| record.state == ModelState::Missing)
        .count();
    let free = app.facts.free_bytes().max(0) as f64 / (1u64 << 30) as f64;
    [
        app.records.len() as u64,
        warm as u64,
        gone as u64,
        free.round() as u64,
    ]
}

/// The order tiles leave in when the row is short: gone, then warm, then
/// models; free and gateway stay.
const DROP_ORDER: [usize; 3] = [2, 1, 0];

/// Lay `tiles` from `right` leftward, `(x, index)` left to right. They keep
/// clear of the whole tagline when they can, dropping tiles in
/// [`DROP_ORDER`] to do it; when even the last two cannot, they keep clear
/// of the version at `floor` and the tagline is clipped instead.
fn place(tiles: &[Tile], floor: u16, tagline_end: u16, right: u16) -> Vec<(u16, usize)> {
    let try_place = |shown: &[usize], limit: u16| -> Option<Vec<(u16, usize)>> {
        let mut x = right;
        let mut placed = Vec::new();
        for &index in shown.iter().rev() {
            let width = tiles[index].width();
            let start = x.checked_sub(width)?;
            if start < limit {
                return None;
            }
            placed.push((start, index));
            x = start.saturating_sub(TILE_GAP);
        }
        placed.reverse();
        Some(placed)
    };
    let mut shown: Vec<usize> = (0..tiles.len()).collect();
    if let Some(placed) = try_place(&shown, tagline_end) {
        return placed;
    }
    for drop in DROP_ORDER {
        shown.retain(|&index| index != drop);
        if let Some(placed) = try_place(&shown, tagline_end) {
            return placed;
        }
    }
    try_place(&shown, floor)
        .or_else(|| try_place(&shown[shown.len().saturating_sub(1)..], floor))
        .unwrap_or_default()
}

/// One tile at column `x`: the figure in big type (or the gateway's two
/// lines), the label, and the line under it.
fn draw_tile(buf: &mut Buffer, area: Rect, x: u16, y0: u16, tile: &Tile, app: &App) {
    match &tile.figure {
        Figure::Count { slot, target } => {
            let shown = app.header_figures[*slot]
                .value(*target as f64, FIGURES_FROM_MS, FIGURES_MS, &app.motion)
                .round() as u64;
            let colour = if tile.quiet && shown > 0 {
                crate::tui::palette::SOFT_COLOR
            } else {
                INK_COLOR
            };
            let mut columns = pixel::columns(&shown.to_string(), DIGITS, PIXEL_GAP);
            // A figure easing down from a wider one keeps to the slot its
            // target was given, rather than running into the next tile.
            columns.truncate(tile.width() as usize);
            pixel::draw(buf, x, y0 + DIGITS_ROW, &columns, |_| Some(colour));
        }
        Figure::Gateway(port) => {
            let (state, reach) = gateway_lines(*port);
            let y = y0 + DIGITS_ROW + 1;
            match port {
                Some(_) => {
                    put(buf, area, x, y, "●", Style::new().fg(pulse(&app.motion)));
                    put(buf, area, x + 2, y, "on", BOLD);
                    put(buf, area, x, y + 1, &reach, SOFT);
                }
                None => {
                    put(buf, area, x, y, &state, DIM);
                    put(buf, area, x, y + 1, "S", BOLD);
                    put(buf, area, x + 2, y + 1, "serve", DIM);
                }
            }
        }
    }
    put(buf, area, x, y0 + LABEL_ROW, tile.label, DIM);
    put(buf, area, x, y0 + SUB_ROW, &tile.sub, SOFT);
}

/// ` hedos v1.5.0  12 models · 3 warm · 1 too big`, then against the right
/// edge the gateway (`● :11434`, or `○ gateway off`) and the free memory
/// when no machine card shows it, held to `width` cells: the counts of what
/// won't run go first, then the counts are clipped, so the right side
/// survives a narrow terminal.
fn summary_line(app: &App, machine_shown: bool, width: usize) -> Line<'static> {
    let mark = wordmark();
    let mut right = match app.facts.gateway_port {
        Some(port) => vec![
            Span::styled("●", Style::new().fg(pulse(&app.motion))),
            Span::styled(format!(" :{port}"), SOFT),
        ],
        None => vec![Span::styled("○ gateway off", DIM)],
    };
    if !machine_shown {
        right.push(Span::styled(
            format!(" · {} GiB free", text::gib(app.facts.free_bytes())),
            DIM,
        ));
    }
    right.push(Span::raw(" "));
    let right_width: usize = right.iter().map(Span::width).sum();
    let left_width: usize = mark.iter().map(Span::width).sum::<usize>() + 2;
    let room = width.saturating_sub(left_width + right_width + 2);
    let mut counts = shelf_line(app, true);
    if counts.width() > room {
        counts = shelf_line(app, false);
    }
    let counts = text::clip(&counts, room);
    let mut spans = mark.to_vec();
    spans.push(Span::raw("  "));
    spans.push(Span::styled(counts.clone(), SOFT));
    let used = left_width + counts.width();
    if used + right_width <= width {
        spans.push(Span::raw(" ".repeat(width - used - right_width)));
        spans.extend(right);
    }
    Line::from(spans)
}

/// `12 models · 3 warm · 1 too big · 2 gone`, the last two only when they
/// count something and `wont_run` asks for them. A record whose weights are
/// gone counts once, as gone: the shelf row shows it no verdict either.
fn shelf_line(app: &App, wont_run: bool) -> String {
    let [models, warm, gone, _] = figures(app);
    let mut parts = vec![text::count(models as usize, "model")];
    parts.push(format!("{warm} warm"));
    if !wont_run {
        return parts.join(" · ");
    }
    let too_big = FitTally::over(&app.records, app.facts.memory_bytes).too_large;
    if too_big > 0 {
        parts.push(format!("{too_big} too big"));
    }
    if gone > 0 {
        parts.push(format!("{gone} gone"));
    }
    parts.join(" · ")
}

/// The hero drawn alone at `width`, for reading it back in a test.
#[cfg(test)]
fn hero(app: &App, width: u16) -> Buffer {
    let area = Rect::new(0, 0, width, HERO_ROWS);
    let mut buffer = Buffer::empty(area);
    draw_hero(&mut buffer, area, app);
    buffer
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::tui::facts::Facts;
    use crate::tui::testing::{facts_with_memory, record, text};

    fn facts() -> Facts {
        facts_with_memory(64)
    }

    #[test]
    fn the_short_header_names_the_free_memory_only_when_the_card_is_gone() {
        let app = App::new(vec![record("a"), record("b")], facts());
        let with_card = text(&summary_line(&app, true, 100));
        assert!(with_card.contains("  2 models · 0 warm "), "{with_card:?}");
        assert!(with_card.ends_with("○ gateway off "), "{with_card:?}");
        assert!(!with_card.contains("GiB"));
        let alone = text(&summary_line(&app, false, 100));
        assert!(alone.ends_with("○ gateway off · 64 GiB free "), "{alone:?}");
        assert_eq!(Line::from(alone.as_str()).width(), 100);
    }

    #[test]
    fn a_gone_record_counts_once_as_gone() {
        let mut gone = record("m");
        gone.footprint_bytes = Some(200 * (1 << 30));
        gone.state = ModelState::Missing;
        let mut too_big = record("n");
        too_big.footprint_bytes = Some(200 * (1 << 30));
        let app = App::new(vec![gone, too_big, record("o")], facts());
        assert_eq!(
            shelf_line(&app, true),
            "3 models · 0 warm · 1 too big · 1 gone"
        );
        assert_eq!(shelf_line(&app, false), "3 models · 0 warm");
        let summary = text(&summary_line(&app, true, 200));
        assert!(
            summary.contains("3 models · 0 warm · 1 too big · 1 gone"),
            "{summary:?}"
        );
    }

    #[test]
    fn the_short_header_keeps_the_gateway_at_eighty_columns() {
        let mut gone = record("m");
        gone.state = ModelState::Missing;
        let mut too_big = record("n");
        too_big.footprint_bytes = Some(200 * (1 << 30));
        let mut records = vec![gone, too_big];
        records.extend((0..10).map(|index| record(&format!("model-{index}"))));
        let facts = Facts {
            gateway_port: Some(11434),
            ..facts()
        };
        let app = App::new(records, facts);
        let wide = text(&summary_line(&app, false, 200));
        assert!(
            wide.contains("12 models · 0 warm · 1 too big · 1 gone"),
            "{wide:?}"
        );
        assert!(wide.ends_with("● :11434 · 64 GiB free "), "{wide:?}");
        let full = text(&summary_line(&app, false, 80));
        assert!(full.contains("1 too big · 1 gone"), "{full:?}");
        let narrow = summary_line(&app, false, 70);
        assert!(narrow.width() <= 70, "{:?}", text(&narrow));
        let narrow = text(&narrow);
        assert!(narrow.contains("12 models · 0 warm "), "{narrow:?}");
        assert!(!narrow.contains("too big"));
        assert!(narrow.ends_with("● :11434 · 64 GiB free "), "{narrow:?}");
        let tiny = text(&summary_line(&app, false, 48));
        assert!(Line::from(tiny.as_str()).width() <= 48, "{tiny:?}");
        assert!(tiny.contains('…'), "{tiny:?}");
    }

    /// Every tile's columns, read back from where they were placed.
    fn tile_rects(app: &App, width: u16) -> Vec<(u16, u16)> {
        let tiles = tiles(app);
        let version = format!("v{}", env!("CARGO_PKG_VERSION"));
        let word_x = WORD_AT.0;
        let floor =
            word_x + pixel::width("hedos", WORD_FONT, PIXEL_GAP) + 2 + version.width() as u16 + 2;
        let tagline_end = word_x + TAGLINE.width().max(GLOSS.width()) as u16 + 2;
        place(&tiles, floor, tagline_end, width - RIGHT_MARGIN)
            .into_iter()
            .map(|(x, index)| (x, x + tiles[index].width()))
            .collect()
    }

    #[test]
    fn the_tiles_never_overlap_and_the_gateway_always_shows() {
        let mut records: Vec<_> = (0..15).map(|index| record(&format!("m{index}"))).collect();
        records[3].state = ModelState::Missing;
        let app = App::new(
            records,
            Facts {
                gateway_port: Some(11434),
                ..facts()
            },
        );
        for width in [132, 120, 100, 96] {
            let rects = tile_rects(&app, width);
            assert!(!rects.is_empty(), "at {width}");
            for pair in rects.windows(2) {
                assert!(pair[0].1 + TILE_GAP <= pair[1].0, "{rects:?} at {width}");
            }
            assert!(
                rects
                    .last()
                    .is_some_and(|rect| rect.1 <= width - RIGHT_MARGIN)
            );
            let buffer = hero(&app, width);
            let label_row: String = (0..width)
                .map(|x| buffer[(x, LABEL_ROW)].symbol())
                .collect();
            assert!(label_row.contains("GATEWAY"), "{label_row:?} at {width}");
            assert!(label_row.contains("FREE"), "{label_row:?} at {width}");
        }
        assert_eq!(tile_rects(&app, 132).len(), 5, "everything fits at 132");
    }

    #[test]
    fn the_tagline_is_whole_where_the_tiles_leave_it_room() {
        let app = App::new(vec![record("a")], facts());
        let buffer = hero(&app, 132);
        let row: String = (0..132)
            .map(|x| buffer[(x, TAGLINE_ROW)].symbol())
            .collect();
        assert!(row.contains(TAGLINE), "{row:?}");
        let gloss: String = (0..132).map(|x| buffer[(x, GLOSS_ROW)].symbol()).collect();
        assert!(
            gloss.contains("the place where something comes to rest"),
            "{gloss:?}"
        );
    }

    #[test]
    fn the_settled_hero_shows_the_figures_and_the_wordmark() {
        let app = App::new(
            (0..15).map(|index| record(&format!("m{index}"))).collect(),
            facts(),
        );
        let buffer = hero(&app, 132);
        let wordmark: String = (WORD_AT.0..WORD_AT.0 + 29)
            .map(|x| buffer[(x, WORD_AT.1 + 3)].symbol())
            .collect();
        assert_eq!(wordmark, "▀▀ ▀▀ ▀▀▀▀▀ ▀▀▀▀▀ ▀▀▀▀▀ ▀▀▀▀▀");
        let digits: String = (0..132).map(|x| buffer[(x, DIGITS_ROW)].symbol()).collect();
        assert!(digits.contains("▄█  █▀▀"), "15 is drawn: {digits:?}");
    }
}
