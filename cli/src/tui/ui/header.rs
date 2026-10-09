//! The header. With room, the hero: the koala, the wordmark in pixel type,
//! what hedos is, and on the right the gateway's pulse over the last day with
//! the shelf and memory in two lines under it. Without, one line of numbers.

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use kernel::profiles::FitTally;
use kernel::records::ModelState;

use super::{BOLD, DIM, INK, SOFT, wordmark};
use crate::tui::app::App;
use crate::tui::facts::PULSE_SLOTS;
use crate::tui::koala;
use crate::tui::layout::HERO_ROWS;
use crate::tui::motion::Motion;
use crate::tui::palette::{
    BRIGHT, FAINT_COLOR, GHOST, LEVELS, LINE, OLIVE, OLIVE_DIM, OLIVE_HOT, SOFT_COLOR, WHITE, WORD,
    mix,
};
use crate::tui::pixel::{self, WORD as WORD_FONT};
use crate::tui::text;

/// Where the hero's parts sit, from the header's top-left corner.
const KOALA_AT: (u16, u16) = (2, 1);
const WORD_AT: (u16, u16) = (27, 2);
const TAGLINE_ROW: u16 = 7;
const GLOSS_ROW: u16 = 8;
/// The pulse's rows: the gateway's badge, the two rows of bars, the axis.
const BADGE_ROW: u16 = 2;
const BARS_ROW: u16 = 3;
const AXIS_ROW: u16 = 5;
/// The count lines share the tagline's rows, against the right margin.
const COUNTS_ROW: u16 = TAGLINE_ROW;
const MEMORY_ROW: u16 = GLOSS_ROW;
/// What hedos is, and what ἕδος means.
const TAGLINE: &str = "one home for every local model on your machine";
const GLOSS: &str = "ἕδος · the place where something comes to rest";
/// Columns between the hero and the right edge, and between the version and
/// the pulse: wide enough that the axis under the bars, on the version's row,
/// does not read as part of it.
const RIGHT_MARGIN: u16 = 2;
const PULSE_GAP: u16 = 5;
/// The pulse's widest, two quarter hours a column, and its narrowest, under
/// which the bars go and the badge stays.
const PULSE_MAX: u16 = 48;
const PULSE_MIN: u16 = 24;
/// Eighths of a cell the two rows of bars hold between them.
const PULSE_LEVELS: f32 = 16.0;
/// The ends of the pulse's axis.
const AXIS_FROM: &str = "24h ago";
const AXIS_TO: &str = "now";
/// The columns kept between the badge and the rate at its right.
const BADGE_GAP: usize = 2;
/// The pixel gap between two glyphs of the wordmark.
const PIXEL_GAP: usize = 1;

/// When each part arrives, in milliseconds from launch.
const WORD_FROM_MS: u64 = 60;
const WORD_STEP_MS: u64 = 5;
const WORD_FRESH_MS: u64 = 120;
const VERSION_FROM_MS: u64 = 200;
const TAGLINE_FROM_MS: u64 = 150;
const GLOSS_FROM_MS: u64 = 250;
const TAGLINE_PER_MS: u64 = 3;
const BADGE_FROM_MS: u64 = 60;
/// The bars rise once at launch, left to right, each column a step behind
/// the one before it.
const BARS_FROM_MS: u64 = 60;
const BAR_STEP_MS: u64 = 4;
const BAR_RISE_MS: u64 = 160;
/// The counts count up from zero over this long, and ease to a change.
pub(crate) const FIGURES_FROM_MS: u64 = 80;
pub(crate) const FIGURES_MS: u64 = 350;
/// The koala's shine comes round every six seconds; the wordmark's sweep
/// follows it, this far into the cycle, for this long, this wide.
const CYCLE_S: f32 = 6.0;
const INTRO_S: f32 = 1.0;
const SWEEP_AT_S: f32 = 1.9;
const SWEEP_S: f32 = 0.9;
const SWEEP_HALF_WIDTH: f32 = 3.5;
/// The gateway dot's beat, the rhythm of hedos.ai's live dot.
const DOT_MS: u64 = 1800;

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

/// The gateway dot's colour now: a beat from bright to dim, or the warm hue
/// itself when nothing moves.
pub(super) fn live_dot(motion: &Motion) -> Color {
    match motion.age(0) {
        Some(age) => {
            let phase = (age * 1000.0 % DOT_MS as f32) / DOT_MS as f32;
            mix(OLIVE_DIM, OLIVE_HOT, 1.0 - phase.sqrt())
        }
        None => OLIVE,
    }
}

/// The koala, the wordmark and version, the tagline, the pulse and the
/// count lines.
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

    let floor = version_x + version.width() as u16 + PULSE_GAP;
    let right = area.right().saturating_sub(RIGHT_MARGIN);
    draw_pulse(buf, area, floor, right, app);
    let [counts_x, memory_x] = draw_counts(buf, area, right, app);
    let room = |x: u16| x.saturating_sub(word_x + 2) as usize;
    reveal(
        buf,
        area,
        word_x,
        y0 + TAGLINE_ROW,
        TAGLINE,
        room(counts_x),
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
        room(memory_x),
        GLOSS_FROM_MS,
        DIM,
        motion,
    );
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

/// `spans` from `(x, y)`, each in its own style, held inside `area`.
fn put_spans(buf: &mut Buffer, area: Rect, x: u16, y: u16, spans: &[Span<'static>]) {
    let mut cx = x;
    for span in spans {
        put(buf, area, cx, y, &span.content, span.style);
        cx = cx.saturating_add(span.width() as u16);
    }
}

/// Cells `spans` take.
fn spans_width(spans: &[Span<'static>]) -> usize {
    spans.iter().map(Span::width).sum()
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

/// The gateway's pulse between `floor` and `right`: its badge, and with room
/// the last day's requests as two rows of bars over an axis. A quiet column
/// is a low rule while the gateway is on and left empty while it is off, so
/// a column with any request stands out from one without.
fn draw_pulse(buf: &mut Buffer, area: Rect, floor: u16, right: u16, app: &App) {
    let width = right.saturating_sub(floor).min(PULSE_MAX);
    if width == 0 || app.motion.progress(BADGE_FROM_MS, 1) < 1.0 {
        return;
    }
    let x = right - width;
    let y0 = area.y;
    let (left, rate) = badge(app, width as usize);
    put_spans(buf, area, x, y0 + BADGE_ROW, &left);
    let rate_width = spans_width(&rate) as u16;
    put_spans(
        buf,
        area,
        right.saturating_sub(rate_width),
        y0 + BADGE_ROW,
        &rate,
    );
    if width < PULSE_MIN {
        return;
    }
    let on = app.facts.gateway_port.is_some();
    for (column, level) in pulse_levels(&app.facts.activity.pulse, width as usize)
        .into_iter()
        .enumerate()
    {
        let risen = app
            .motion
            .eased(BARS_FROM_MS + column as u64 * BAR_STEP_MS, BAR_RISE_MS);
        let level = (level * risen).round() as usize;
        let cx = x + column as u16;
        let newest = column + 1 == width as usize;
        let colour = match (on, newest) {
            (false, _) => GHOST,
            (true, true) => BRIGHT,
            (true, false) => mix(FAINT_COLOR, SOFT_COLOR, level as f32 / PULSE_LEVELS),
        };
        let bottom = y0 + BARS_ROW + 1;
        if level == 0 {
            if on {
                put(buf, area, cx, bottom, LEVELS[1], Style::new().fg(LINE));
            }
            continue;
        }
        put(
            buf,
            area,
            cx,
            bottom,
            LEVELS[level.min(8)],
            Style::new().fg(colour),
        );
        if level > 8 {
            put(
                buf,
                area,
                cx,
                bottom - 1,
                LEVELS[(level - 8).min(8)],
                Style::new().fg(colour),
            );
        }
    }
    put(buf, area, x, y0 + AXIS_ROW, AXIS_FROM, DIM);
    put(
        buf,
        area,
        right - AXIS_TO.width() as u16,
        y0 + AXIS_ROW,
        AXIS_TO,
        DIM,
    );
}

/// The badge over the pulse, the gateway's state at the left and what goes
/// with it at the right, in the longest form that fits `width`: with the
/// address, with the port alone, without the word `GATEWAY`, and only then
/// without the live rate.
fn badge(app: &App, width: usize) -> (Vec<Span<'static>>, Vec<Span<'static>>) {
    let forms = match app.facts.gateway_port {
        Some(port) => {
            let state = |word: &'static str, reach: String| {
                vec![
                    Span::styled("●", Style::new().fg(live_dot(&app.motion))),
                    Span::styled(word, INK),
                    Span::styled(reach, DIM),
                ]
            };
            let rate = vec![Span::styled(
                format!("{} req/min", app.facts.activity.requests_last_minute),
                SOFT,
            )];
            vec![
                (
                    state(" GATEWAY ON", format!("  127.0.0.1:{port}")),
                    rate.clone(),
                ),
                (state(" GATEWAY ON", format!("  :{port}")), rate.clone()),
                (state(" ON", format!("  :{port}")), rate),
                (state(" ON", format!("  :{port}")), Vec::new()),
            ]
        }
        None => {
            let serve = vec![Span::styled("S", BOLD), Span::styled(" serve", DIM)];
            vec![
                (
                    vec![Span::styled("○", DIM), Span::styled(" GATEWAY OFF", SOFT)],
                    serve.clone(),
                ),
                (
                    vec![Span::styled("○", DIM), Span::styled(" OFF", SOFT)],
                    serve,
                ),
            ]
        }
    };
    let fits = |(left, right): &(Vec<Span<'static>>, Vec<Span<'static>>)| {
        let gap = if right.is_empty() { 0 } else { BADGE_GAP };
        spans_width(left) + gap + spans_width(right) <= width
    };
    let last = forms.last().cloned().unwrap_or_default();
    forms.into_iter().find(fits).unwrap_or(last)
}

/// The pulse's `slots` laid onto `width` columns, each column the mean of the
/// quarter hours it covers, as eighths of the two rows: the busiest column
/// fills both, a column with any request shows at least one eighth.
fn pulse_levels(slots: &[u32; PULSE_SLOTS], width: usize) -> Vec<f32> {
    let means: Vec<f32> = (0..width)
        .map(|column| {
            let from = column * PULSE_SLOTS / width;
            let to = ((column + 1) * PULSE_SLOTS / width).max(from + 1);
            let covered = &slots[from..to.min(PULSE_SLOTS)];
            covered.iter().sum::<u32>() as f32 / covered.len().max(1) as f32
        })
        .collect();
    let highest = means.iter().copied().fold(0.0, f32::max);
    if highest <= 0.0 {
        return vec![0.0; width];
    }
    means
        .into_iter()
        .map(|mean| {
            if mean > 0.0 {
                (mean / highest * PULSE_LEVELS).max(1.0)
            } else {
                0.0
            }
        })
        .collect()
}

/// The two count lines against `right` on the tagline's rows, as they ease.
/// Returns, per row, where the tagline or the gloss beside it has to stop:
/// where the line starts once its counts have settled, so the cut holds
/// still while they count up, or further left while an easing line is the
/// wider one, as when a gone count eases back to none.
fn draw_counts(buf: &mut Buffer, area: Rect, right: u16, app: &App) -> [u16; 2] {
    let start = |spans: &[Span<'static>]| right.saturating_sub(spans_width(spans) as u16);
    let settled = count_lines(app, figures(app));
    let shown = count_lines(app, eased_figures(app));
    let mut stops = [right; 2];
    for (index, row) in [COUNTS_ROW, MEMORY_ROW].into_iter().enumerate() {
        let x = start(&shown[index]);
        put_spans(buf, area, x, area.y + row, &shown[index]);
        stops[index] = x.min(start(&settled[index]));
    }
    stops
}

/// The shelf (`19 models · 3 warm · 3 gone`, gone only when something is)
/// and the memory (`18.6 GiB held · 45 GiB free of 64`, the free part only
/// when the machine's memory is known) for `counts`.
fn count_lines(app: &App, counts: [u64; 4]) -> [Vec<Span<'static>>; 2] {
    let [models, warm, gone, free] = counts;
    let mut shelf = vec![
        Span::styled(models.to_string(), BOLD),
        Span::styled(if models == 1 { " model" } else { " models" }, SOFT),
        Span::styled(" · ", DIM),
        Span::styled(warm.to_string(), BOLD),
        Span::styled(" warm", SOFT),
    ];
    if gone > 0 {
        shelf.push(Span::styled(" · ", DIM));
        shelf.push(Span::styled(gone.to_string(), SOFT));
        shelf.push(Span::styled(" gone", DIM));
    }
    let held = app.facts.resident_bytes();
    let mut memory = if held > 0 {
        vec![
            Span::styled(format!("{} GiB", text::gib(held)), SOFT),
            Span::styled(" held", DIM),
        ]
    } else {
        vec![Span::styled("nothing held", DIM)]
    };
    if app.facts.memory_bytes() > 0 {
        memory.extend([
            Span::styled(" · ", DIM),
            Span::styled(format!("{free} GiB"), SOFT),
            Span::styled(
                format!(
                    " free of {}",
                    text::gib_short(app.facts.memory_bytes() as i64)
                ),
                DIM,
            ),
        ]);
    }
    [shelf, memory]
}

/// The counts as they ease from zero at launch and to each change.
fn eased_figures(app: &App) -> [u64; 4] {
    let targets = figures(app);
    std::array::from_fn(|slot| {
        app.header_figures[slot]
            .value(
                targets[slot] as f64,
                FIGURES_FROM_MS,
                FIGURES_MS,
                &app.motion,
            )
            .round() as u64
    })
}

/// The counts the hero shows: models, warm, gone, and free GiB.
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

/// ` hedos v1.5.0  12 models · 3 warm · 1 too big`, then against the right
/// edge the gateway (`● :11434`, or `○ gateway off`) and the free memory
/// when no machine card shows it, held to `width` cells: the counts of what
/// won't run go first, then the counts are clipped, so the right side
/// survives a narrow terminal.
fn summary_line(app: &App, machine_shown: bool, width: usize) -> Line<'static> {
    let mark = wordmark();
    let mut right = match app.facts.gateway_port {
        Some(port) => vec![
            Span::styled("●", Style::new().fg(live_dot(&app.motion))),
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
    let too_big = FitTally::over(&app.records, &app.facts.machine).too_large;
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

    /// The hero's row `y` at `width`, read back as text.
    fn row(buffer: &Buffer, y: u16) -> String {
        (0..buffer.area.width)
            .map(|x| buffer[(x, y)].symbol())
            .collect()
    }

    fn served(port: Option<u16>) -> App {
        let mut records: Vec<_> = (0..19).map(|index| record(&format!("m{index}"))).collect();
        for gone in &mut records[..3] {
            gone.state = ModelState::Missing;
        }
        let mut facts = Facts {
            gateway_port: port,
            ..facts()
        };
        facts.activity.requests_last_minute = 6;
        for (slot, count) in facts.activity.pulse.iter_mut().enumerate() {
            *count = if slot % 10 < 4 {
                0
            } else {
                (slot % 7) as u32 + 1
            };
        }
        App::new(records, facts)
    }

    #[test]
    fn the_pulse_lays_its_quarter_hours_onto_the_columns() {
        let mut slots = [0; PULSE_SLOTS];
        slots[PULSE_SLOTS - 1] = 4;
        slots[PULSE_SLOTS - 2] = 4;
        slots[0] = 1;
        let levels = pulse_levels(&slots, 48);
        assert_eq!(levels.len(), 48);
        assert_eq!(
            levels[47], PULSE_LEVELS,
            "the busiest column fills both rows"
        );
        assert_eq!(levels[0], 2.0, "a quiet column still shows");
        assert!(levels[1..47].iter().all(|level| *level == 0.0));
        assert_eq!(pulse_levels(&[0; PULSE_SLOTS], 30), vec![0.0; 30]);
        let uneven = pulse_levels(&[3; PULSE_SLOTS], 26);
        assert!(
            uneven.iter().all(|level| *level == PULSE_LEVELS),
            "{uneven:?}"
        );
    }

    #[test]
    fn the_pulse_keeps_clear_of_the_version_and_its_badge_always_shows() {
        for port in [Some(43367), None] {
            let app = served(port);
            for width in [132, 120, 100, 96] {
                let buffer = hero(&app, width);
                let badge = row(&buffer, BADGE_ROW);
                let state = if port.is_some() { "ON  " } else { "OFF" };
                assert!(badge.contains(state), "{badge:?} at {width}");
                let version = row(&buffer, WORD_AT.1 + 3);
                let at = version.find('v').expect("the version");
                let after = &version[at..];
                assert!(
                    after
                        .trim_end()
                        .starts_with(&format!("v{}", env!("CARGO_PKG_VERSION"))),
                    "{version:?} at {width}"
                );
                assert!(
                    row(&buffer, AXIS_ROW).contains(AXIS_FROM),
                    "the bars fit at {width}"
                );
            }
        }
        let wide = hero(&served(Some(43367)), 132);
        let badge = row(&wide, BADGE_ROW);
        assert!(badge.contains("● GATEWAY ON  127.0.0.1:43367"), "{badge:?}");
        assert!(badge.trim_end().ends_with("6 req/min"), "{badge:?}");
        let narrow = row(&hero(&served(Some(43367)), 96), BADGE_ROW);
        assert!(narrow.contains("● ON  :43367"), "{narrow:?}");
        assert!(
            narrow.trim_end().ends_with("6 req/min"),
            "the rate stays: {narrow:?}"
        );
        let off = row(&hero(&served(None), 132), BADGE_ROW);
        assert!(off.contains("○ GATEWAY OFF"), "{off:?}");
        assert!(off.trim_end().ends_with("S serve"), "{off:?}");
    }

    #[test]
    fn the_newest_bar_is_the_brightest_and_an_off_gateway_greys_the_day() {
        let app = served(Some(43367));
        let buffer = hero(&app, 132);
        let right = 132 - RIGHT_MARGIN;
        let bottom = BARS_ROW + 1;
        assert_eq!(buffer[(right - 1, bottom)].fg, BRIGHT);
        assert_ne!(buffer[(right - 2, bottom)].fg, BRIGHT);
        let bars: String = (right - PULSE_MAX..right)
            .map(|x| buffer[(x, bottom)].symbol())
            .collect();
        assert_eq!(bars.chars().count(), PULSE_MAX as usize, "{bars:?}");
        assert_eq!(
            buffer[(right - 7, bottom)].fg,
            LINE,
            "a quiet column is a rule"
        );
        let off = hero(&served(None), 132);
        assert!(
            (right - PULSE_MAX..right)
                .all(|x| off[(x, bottom)].fg == GHOST || off[(x, bottom)].symbol() == " "),
            "an off gateway's day is drawn in ghost, its quiet columns left empty"
        );
    }

    #[test]
    fn the_counts_sit_on_the_tagline_rows_and_the_tagline_stays_whole() {
        let buffer = hero(&served(Some(43367)), 132);
        let counts = row(&buffer, COUNTS_ROW);
        assert!(counts.contains(TAGLINE), "{counts:?}");
        assert!(
            counts.trim_end().ends_with("19 models · 0 warm · 3 gone"),
            "{counts:?}"
        );
        let memory = row(&buffer, MEMORY_ROW);
        assert!(
            memory.contains("the place where something comes to rest"),
            "{memory:?}"
        );
        assert!(
            memory
                .trim_end()
                .ends_with("nothing held · 64 GiB free of 64"),
            "{memory:?}"
        );
        let none_gone = hero(&App::new(vec![record("a")], facts()), 132);
        let counts = row(&none_gone, COUNTS_ROW);
        assert!(
            counts.trim_end().ends_with("1 model · 0 warm"),
            "{counts:?}"
        );
    }

    #[test]
    fn the_settled_hero_shows_the_wordmark() {
        let buffer = hero(&served(Some(43367)), 132);
        let wordmark: String = (WORD_AT.0..WORD_AT.0 + 29)
            .map(|x| buffer[(x, WORD_AT.1 + 3)].symbol())
            .collect();
        assert_eq!(wordmark, "▀▀ ▀▀ ▀▀▀▀▀ ▀▀▀▀▀ ▀▀▀▀▀ ▀▀▀▀▀");
    }
}
