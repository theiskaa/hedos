//! The task strip: one line per background task, newest last. A key hint
//! sits only on the row it acts on, against the strip's right edge: `d` on
//! the newest failure, `c` on the newest running pull, `w` and `l` on a done
//! pull while its model is the selected one.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use kernel::install::event::InstallProgress;

use super::card::Card;
use super::{
    ACCENT, BOLD, DIM, FAILED, INK, SOFT, card, key_spans, label_width, padded, right_aligned,
    spinner,
};
use crate::support::clock;
use crate::tui::app::{App, Screen};
use crate::tui::keymap;
use crate::tui::motion::Motion;
use crate::tui::palette::{LINE_STRONG, SOFT_COLOR, WHITE, mix};
use crate::tui::pulls::Rate;
use crate::tui::strip::{HintTargets, RowHints, RowSource, TaskRow};
use crate::tui::tasks::{TaskKind, TaskState};
use crate::tui::text;

/// The narrowest download bar worth drawing; under it the figures stand
/// alone.
const MIN_BAR_WIDTH: u16 = 8;
/// The widest download bar, however much room the row has.
const MAX_BAR_WIDTH: u16 = 34;
/// Cells the percentage is held to: `100%` at the widest.
const PERCENT_WIDTH: usize = 4;
/// How long the shimmer takes to cross a download's bar, and how far either
/// side of its centre it lights.
const SHIMMER_MS: f32 = 1600.0;
const SHIMMER_HALF_WIDTH: f32 = 3.0;

/// A running task's verb: bright and loud, the one thing in motion.
const LIVE_VERB: Style = ACCENT.add_modifier(ratatui::style::Modifier::BOLD);

/// What a row needs beyond itself to move: the clock and the spinner's
/// frame.
pub(super) struct Look<'a> {
    pub motion: &'a Motion,
    pub spin_frame: u64,
}

/// Draw the strip into `area`; when it is short, the running rows stay and
/// the oldest finished ones go.
pub(super) fn draw(frame: &mut Frame, area: Rect, app: &App) {
    card("tasks").render(area, frame.buffer_mut());
    let inner = Card::text_inner(area);
    let height = inner.height as usize;
    // The reducer reads the hints back at key time and must agree with what is
    // drawn here, which on a short terminal is fewer rows than the layout asked
    // for.
    app.note_task_rows(height);
    let shown = app.tasks.shown(height);
    // On the pulls and bench screens the keys act on the list's selection, so a
    // hint beside a strip row would name work the key might not touch.
    let targets = match app.screen {
        Screen::Shelf => app
            .tasks
            .hint_targets(height, |reference| app.selected_is(reference)),
        Screen::Pulls | Screen::Bench => HintTargets::default(),
    };
    let look = Look {
        motion: &app.motion,
        spin_frame: app.spin_frame(),
    };
    let lines: Vec<Line> = shown
        .iter()
        .map(|row| {
            let rate = match &row.source {
                RowSource::Pull(job) => app.pulls.rate(job, &row.progress),
                RowSource::Task(_) | RowSource::HandOff => None,
            };
            line(row, inner.width as usize, targets.for_row(row), &look, rate)
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

/// A task's row at `width` cells: verb, subject, then its detail, with the
/// hints that act on this row against the right edge; `c` sits on a pull at
/// any running stage and opens the stop card, `w` and `l` on a done pull
/// while its model is selected.
fn line(
    row: &TaskRow,
    width: usize,
    hinted: RowHints,
    look: &Look,
    rate: Option<Rate>,
) -> Line<'static> {
    let verb = format!(
        " {} ",
        padded(row.label.verb, label_width(&TaskKind::VERBS, 0))
    );
    let subject = format!("{}  ", row.label.subject);
    let head = verb.width() + subject.width();
    let activity = row.kind().map_or("", TaskKind::activity);
    let keys: Vec<&str> = match &row.state {
        TaskState::Running | TaskState::Status(_) | TaskState::Downloading(_)
            if hinted.stoppable =>
        {
            vec!["c"]
        }
        TaskState::Done(_) if hinted.on_selected => vec!["w", "l"],
        TaskState::Stopped(_) if hinted.resumable => vec!["R"],
        TaskState::Failed(_) if hinted.dismissable => vec!["d"],
        _ => Vec::new(),
    };
    let hint = hints(&keys);
    let hint_width: usize = hint.iter().map(Span::width).sum();
    let room = width.saturating_sub(head + hint_width);
    let (verb_style, subject_style, detail) = match &row.state {
        TaskState::Running => (
            LIVE_VERB,
            INK,
            vec![
                Span::styled(format!("{} ", spinner(look.spin_frame)), ACCENT),
                Span::styled(text::clip(activity, room.saturating_sub(2)), DIM),
            ],
        ),
        TaskState::Status(status) => (
            LIVE_VERB,
            INK,
            vec![
                Span::styled(format!("{} ", spinner(look.spin_frame)), ACCENT),
                Span::styled(text::clip(status, room.saturating_sub(2)), DIM),
            ],
        ),
        TaskState::Downloading(progress) => (LIVE_VERB, INK, download(progress, room, rate, look)),
        TaskState::Done(summary) => (
            DIM,
            SOFT,
            vec![Span::styled(text::clip(summary, room), DIM)],
        ),
        TaskState::Stopped(how) => (DIM, SOFT, vec![Span::styled(text::clip(how, room), DIM)]),
        TaskState::Failed(reason) => (FAILED, INK, vec![Span::raw(text::clip(reason, room))]),
    };
    let mut spans = vec![
        Span::styled(verb, verb_style),
        Span::styled(subject, subject_style),
    ];
    spans.extend(detail);
    let used: usize = spans.iter().map(Span::width).sum();
    if !hint.is_empty() && used + hint_width <= width {
        spans.push(Span::raw(" ".repeat(width - used - hint_width)));
        spans.extend(hint);
    }
    Line::from(spans)
}

/// A bar and figures when the total is firm, bytes so far when it is not.
/// The bar takes what `room` leaves after the figures, within its bounds;
/// when that is under the floor the figures stand alone. The rate and the
/// time left follow the figures when the screen has measured them and
/// there is room.
fn download(
    progress: &InstallProgress,
    room: usize,
    rate: Option<Rate>,
    look: &Look,
) -> Vec<Span<'static>> {
    let done = text::bytes(progress.bytes_downloaded);
    let pace = rate.map(|rate| {
        let left = rate
            .left_ms
            .map(|left| format!(" · {} left", clock::millis(left)))
            .unwrap_or_default();
        format!(" · {}/s{left}", text::bytes(rate.bytes_per_second))
    });
    match (progress.fraction(), progress.total_bytes) {
        (Some(fraction), Some(total)) => {
            let percent = format!("{}%", (fraction * 100.0) as u64);
            let figures = format!("{done} of {}", text::bytes(total));
            let fixed = 2 + PERCENT_WIDTH + 2 + figures.width();
            let pace = pace
                .filter(|pace| room.saturating_sub(fixed + pace.width()) >= MIN_BAR_WIDTH as usize);
            let fixed = fixed + pace.as_ref().map_or(0, |pace| pace.width());
            let bar_width = room.saturating_sub(fixed).min(MAX_BAR_WIDTH as usize);
            if bar_width < MIN_BAR_WIDTH as usize {
                return vec![
                    Span::styled(percent, BOLD),
                    Span::styled(format!(" · {figures}"), SOFT),
                ];
            }
            let mut spans = shimmering_bar(fraction, bar_width, look.motion);
            spans.push(Span::styled(
                format!("  {}", right_aligned(&percent, PERCENT_WIDTH)),
                BOLD,
            ));
            spans.push(Span::styled(format!("  {figures}"), SOFT));
            if let Some(pace) = pace {
                spans.push(Span::styled(pace, SOFT));
            }
            spans
        }
        _ => vec![Span::styled(
            text::clip(&format!("{done} so far"), room),
            DIM,
        )],
    }
}

/// A download's bar, `fraction` of `width` cells filled with a thin rule
/// and a half cell where the fill ends mid-cell, a light band running along
/// the filled part while the clock moves.
fn shimmering_bar(fraction: f64, width: usize, motion: &Motion) -> Vec<Span<'static>> {
    let filled = fraction.clamp(0.0, 1.0) * width as f64;
    let full = filled.floor() as usize;
    let half = filled - full as f64 >= 0.5 && full < width;
    let band = motion.age(0).map(|age| {
        let phase = (age * 1000.0 % SHIMMER_MS) / SHIMMER_MS;
        phase * (full as f32 + 10.0) - 5.0
    });
    let mut spans = Vec::with_capacity(width);
    for cell in 0..width {
        if cell < full || (cell == full && half) {
            let lit = band.map_or(0.0, |band| {
                (1.0 - (cell as f32 - band).abs() / SHIMMER_HALF_WIDTH).clamp(0.0, 1.0)
            });
            let glyph = if cell < full { "━" } else { "╸" };
            spans.push(Span::styled(
                glyph,
                Style::new().fg(mix(SOFT_COLOR, WHITE, 0.9 * lit)),
            ));
        } else {
            spans.push(Span::styled("━", Style::new().fg(LINE_STRONG)));
        }
    }
    spans
}

/// The keys a row offers, set off from its detail by a gap, the last verb's
/// trailing gap left off so the keys end at the edge.
fn hints(keys: &[&str]) -> Vec<Span<'static>> {
    if keys.is_empty() {
        return Vec::new();
    }
    let mut spans = vec![Span::raw("  ")];
    spans.extend(key_spans(&keymap::pairs(keys)));
    if let Some(last) = spans.last_mut() {
        last.content = last.content.trim_end().to_owned().into();
    }
    spans
}

#[cfg(test)]
mod tests;
