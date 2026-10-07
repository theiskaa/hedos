//! The session card beside the conversation: the model and whether it is
//! held, how much of its context the conversation fills, how fast the last
//! reply came in big figures, a bar for each reply's speed, and how long the
//! session has run.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use super::{Look, clock_time};
use crate::support::clock;
use crate::support::residency::Resident;
use crate::tui::chat::{ChatPane, Ending, Speaker};
use crate::tui::palette::{FAINT_COLOR, INK_COLOR, LINE_STRONG, OLIVE, mix};
use crate::tui::pixel::{self, DIGITS};
use crate::tui::text;
use crate::tui::ui::card::Card;
use crate::tui::ui::{BOLD, DIM, INK, SOFT, section};

/// The bar heights a speed can take.
const LEVELS: [&str; 8] = ["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];
/// Speeds are drawn against at least this many tokens a second, so one
/// slow reply does not fill the scale.
const SPEED_FLOOR: f64 = 60.0;

/// Whether the model is held, and when the facts saying so were read.
pub(super) struct Residency<'a> {
    pub resident: Option<&'a Resident>,
    pub collected_at: i64,
}

/// Draw the card into `area`.
pub(super) fn draw(frame: &mut Frame, area: Rect, pane: &ChatPane, look: &Look, held: Residency) {
    Card::new(vec![Span::styled("session", BOLD)]).render(area, frame.buffer_mut());
    let inner = Card::text_inner(area);
    if pane.judging() {
        draw_judge(frame, inner, pane, &held);
        return;
    }
    let width = inner.width as usize;
    let mut lines = heading(pane, &held, width);
    lines.push(section("CONTEXT", width));
    let record = &pane.record;
    let (used, estimated) = pane.context_used();
    match record.context_length.filter(|window| *window > 0) {
        Some(window) => {
            let filled = ((used as f64 / window as f64) * width as f64).ceil() as usize;
            let filled = filled.clamp(usize::from(used > 0), width);
            lines.push(Line::from(vec![
                Span::styled("━".repeat(filled), INK),
                Span::styled("━".repeat(width - filled), Style::new().fg(LINE_STRONG)),
            ]));
            lines.push(Line::from(vec![
                Span::styled(format!("{}{}", tilde(estimated), compact(used)), BOLD),
                Span::styled(format!(" of {} tokens", compact(window as u64)), SOFT),
            ]));
        }
        None => lines.push(Line::from(vec![
            Span::styled(format!("{}{}", tilde(estimated), compact(used)), BOLD),
            Span::styled(" tokens so far", SOFT),
        ])),
    }
    lines.push(Line::default());
    lines.push(section("LAST REPLY", width));
    let replies: Vec<_> = pane
        .turns
        .iter()
        .filter(|turn| turn.speaker == Speaker::Model)
        .collect();
    let figure_row = lines.len();
    let waiting = replies
        .last()
        .is_some_and(|last| last.ending == Ending::Open && last.text.is_empty());
    match replies.last() {
        None => lines.push(Line::from(Span::styled("nothing asked yet", DIM))),
        Some(last) if waiting => lines.push(Line::from(Span::styled(
            if last.cold {
                "loading into memory"
            } else {
                "waiting for the first token"
            },
            DIM,
        ))),
        Some(last) => {
            lines.extend([Line::default(), Line::default(), Line::default()]);
            let streaming = last.ending == Ending::Open;
            let ended = last.ended_ms.map(|ended| {
                format!(
                    " · {:.1}s",
                    ended.saturating_sub(last.at_ms) as f64 / 1000.0
                )
            });
            let (tokens, estimated) = last.tokens();
            lines.push(Line::from(Span::styled(
                text::clip(
                    &match (streaming, &last.ending) {
                        (true, _) => "streaming".to_owned(),
                        (_, Ending::Stopped) => {
                            format!("stopped · {}{tokens} tokens", tilde(estimated))
                        }
                        (_, Ending::Failed(_)) => "failed".to_owned(),
                        _ => format!(
                            "{}{}{}",
                            tilde(estimated),
                            text::count(tokens as usize, "token"),
                            ended.unwrap_or_default()
                        ),
                    },
                    width,
                ),
                SOFT,
            )));
            if let Some(first) = last.first_token_after() {
                lines.push(Line::from(Span::styled(
                    format!("first token {}", clock::millis(first as i64)),
                    DIM,
                )));
            }
        }
    }
    lines.push(Line::default());
    lines.push(section("SPEED", width));
    let rates: Vec<f64> = replies
        .iter()
        .filter_map(|turn| turn.rate().map(|(rate, _)| rate))
        .collect();
    if rates.is_empty() {
        lines.push(Line::from(Span::styled("—", DIM)));
    } else {
        let top = rates.iter().copied().fold(SPEED_FLOOR, f64::max);
        let average = rates.iter().sum::<f64>() / rates.len() as f64;
        let label = format!("{average:.0} avg");
        let room = width.saturating_sub(label.width() + 2) / 2;
        let shown = &rates[rates.len().saturating_sub(room)..];
        let mut spans = Vec::new();
        for (index, rate) in shown.iter().enumerate() {
            let level = ((rate / top) * 7.0).round().clamp(0.0, 7.0) as usize;
            let colour = if index + 1 == shown.len() {
                INK_COLOR
            } else {
                mix(FAINT_COLOR, INK_COLOR, 0.3)
            };
            spans.push(Span::styled(LEVELS[level], Style::new().fg(colour)));
            spans.push(Span::raw(" "));
        }
        let used: usize = spans.iter().map(Span::width).sum();
        spans.push(Span::raw(
            " ".repeat(width.saturating_sub(used + label.width())),
        ));
        spans.push(Span::styled(label, DIM));
        lines.push(Line::from(spans));
    }
    lines.push(Line::default());
    lines.push(section("SESSION", width));
    let turns = pane
        .turns
        .iter()
        .filter(|turn| turn.speaker == Speaker::User)
        .count();
    let session = match (turns, pane.since_wall_ms().and_then(clock_time)) {
        (0, _) => "new conversation".to_owned(),
        (turns, Some(since)) => format!("{} · since {since}", text::count(turns, "turn")),
        (turns, None) => text::count(turns, "turn"),
    };
    lines.push(Line::from(Span::styled(text::clip(&session, width), SOFT)));
    lines.push(Line::from(Span::styled("in this process", DIM)));
    let keys = Line::from(vec![
        Span::styled("⌃l", BOLD),
        Span::styled(" clear  ", DIM),
        Span::styled("esc", BOLD),
        Span::styled(if pane.streaming() { " stop" } else { " shelf" }, DIM),
    ]);
    let room = inner.height as usize;
    if lines.len() < room {
        lines.resize(room - 1, Line::default());
        lines.push(keys);
    }
    frame.render_widget(Paragraph::new(lines), inner);
    // The last reply's speed in big figures over the line that says what it
    // came to.
    if let Some(last) = replies.last().filter(|_| !waiting)
        && figure_row + 3 <= inner.height as usize
    {
        let rate = if last.ending == Ending::Open {
            pane.live_rate(look.motion.real_ms())
        } else {
            last.rate().map(|(rate, _)| rate)
        };
        let figure = rate.map_or_else(|| "0".to_owned(), |rate| format!("{rate:.0}"));
        let columns = pixel::columns(&figure, DIGITS, 1);
        let colour = if rate.is_some() {
            INK_COLOR
        } else {
            FAINT_COLOR
        };
        let y = inner.y + figure_row as u16;
        pixel::draw(frame.buffer_mut(), inner.x, y, &columns, |_| Some(colour));
        let x = inner.x + columns.len() as u16 + 2;
        if x + 5 <= inner.right() {
            frame.buffer_mut().set_stringn(x, y + 2, "tok/s", 5, DIM);
        }
    }
}

/// The card for a judge: its last answer in big figures with the outcome
/// it chose, and how many asks there have been. A judge has no reply
/// speed or context to fill: every ask stands alone.
fn draw_judge(frame: &mut Frame, inner: Rect, pane: &ChatPane, held: &Residency) {
    let width = inner.width as usize;
    let mut lines = heading(pane, held, width);
    lines.push(section("LAST JUDGMENT", width));
    let last = pane
        .turns
        .iter()
        .rev()
        .find(|turn| turn.speaker == Speaker::Model);
    let figure_row = lines.len();
    let headline = last.and_then(|turn| {
        let asked = turn.asked.as_deref()?;
        matches!(turn.ending, Ending::Done(_))
            .then(|| super::judgment::headline(&turn.text, asked))
            .flatten()
    });
    match (last, &headline) {
        (None, _) => lines.push(Line::from(Span::styled("nothing asked yet", DIM))),
        (Some(turn), _) if turn.ending == Ending::Open => lines.push(Line::from(Span::styled(
            if turn.cold {
                "loading into memory"
            } else {
                "weighing"
            },
            DIM,
        ))),
        (Some(turn), Some((_, label))) => {
            lines.extend([Line::default(), Line::default(), Line::default()]);
            lines.push(Line::from(Span::styled(text::clip(label, width), BOLD)));
            if let Some(ended) = turn.ended_ms {
                lines.push(Line::from(Span::styled(
                    format!(
                        "judged in {:.1}s",
                        ended.saturating_sub(turn.at_ms) as f64 / 1000.0
                    ),
                    DIM,
                )));
            }
        }
        (Some(turn), None) => lines.push(Line::from(Span::styled(
            match &turn.ending {
                Ending::Stopped => "stopped",
                Ending::Failed(_) => "failed",
                _ => "no judgment came back",
            },
            DIM,
        ))),
    }
    lines.push(Line::default());
    lines.push(section("SESSION", width));
    let asks = pane
        .turns
        .iter()
        .filter(|turn| turn.speaker == Speaker::User)
        .count();
    let session = match (asks, pane.since_wall_ms().and_then(clock_time)) {
        (0, _) => "nothing asked".to_owned(),
        (asks, Some(since)) => format!("{} · since {since}", text::count(asks, "ask")),
        (asks, None) => text::count(asks, "ask"),
    };
    lines.push(Line::from(Span::styled(text::clip(&session, width), SOFT)));
    lines.push(Line::from(Span::styled("each ask stands alone", DIM)));
    let room = inner.height as usize;
    if lines.len() < room {
        lines.resize(room - 1, Line::default());
        lines.push(Line::from(vec![
            Span::styled("⌃l", BOLD),
            Span::styled(" clear  ", DIM),
            Span::styled("esc", BOLD),
            Span::styled(if pane.streaming() { " stop" } else { " shelf" }, DIM),
        ]));
    }
    frame.render_widget(Paragraph::new(lines), inner);
    if let Some((share, _)) = headline
        && figure_row + 3 <= inner.height as usize
    {
        let figure = format!("{:.0}", (share * 100.0).clamp(0.0, 100.0));
        let columns = pixel::columns(&figure, DIGITS, 1);
        let y = inner.y + figure_row as u16;
        pixel::draw(frame.buffer_mut(), inner.x, y, &columns, |_| {
            Some(INK_COLOR)
        });
        let x = inner.x + columns.len() as u16 + 1;
        if x < inner.right() {
            frame.buffer_mut().set_stringn(x, y + 2, "%", 1, DIM);
        }
    }
}

/// The card's head: the model, its runtime and size, and whether it is
/// held.
fn heading(pane: &ChatPane, held: &Residency, width: usize) -> Vec<Line<'static>> {
    let record = &pane.record;
    vec![
        Line::default(),
        Line::from(Span::styled(
            text::clip(
                &crate::support::text::printable(record.display_name()),
                width,
            ),
            BOLD,
        )),
        Line::from(Span::styled(
            text::clip(
                &[
                    text::short_runtime(crate::support::shelf_table::runtime_label(record)),
                    &record.serving_size().map_or_else(String::new, text::bytes),
                ]
                .into_iter()
                .filter(|part| !part.is_empty() && *part != "—")
                .collect::<Vec<_>>()
                .join(" · "),
                width,
            ),
            SOFT,
        )),
        residency(held, width),
        Line::default(),
    ]
}

/// `● warm · unloads in 4m`, or `○ cold · loads on send`.
fn residency(held: &Residency, width: usize) -> Line<'static> {
    match held.resident {
        Some(resident) => {
            let mut spans = vec![
                Span::styled("● ", Style::new().fg(OLIVE)),
                Span::styled("warm", INK),
            ];
            if let Some(seconds) = resident.expires_in_seconds_at(held.collected_at) {
                spans.push(Span::styled(
                    text::clip(
                        &format!(" · unloads in {}", clock::duration(seconds)),
                        width.saturating_sub(6),
                    ),
                    SOFT,
                ));
            }
            Line::from(spans)
        }
        None => Line::from(vec![
            Span::styled("○ ", DIM),
            Span::styled("cold", SOFT),
            Span::styled(" · loads on send", DIM),
        ]),
    }
}

/// `~` in front of a figure counted from the text.
fn tilde(estimated: bool) -> &'static str {
    if estimated { "~" } else { "" }
}

/// `420`, `8.2k`, `128k`.
fn compact(count: u64) -> String {
    match count {
        0..1000 => count.to_string(),
        1000..10_000 => format!("{:.1}k", count as f64 / 1000.0).replace(".0k", "k"),
        _ => format!("{}k", count / 1000),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_read_short() {
        assert_eq!(compact(420), "420");
        assert_eq!(compact(8192), "8.2k");
        assert_eq!(compact(4000), "4k");
        assert_eq!(compact(131_072), "131k");
    }
}
