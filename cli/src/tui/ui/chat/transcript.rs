//! The conversation as lines. Your words sit right-aligned in a raised
//! bubble with the time over it, its corners rounded by quarter blocks.
//! A reply runs left at reading width under the model's name, which carries
//! how the reply is going on the right: loading, thinking, its speed while
//! it streams, its figures once it ends, or that it was stopped or failed.

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use super::{Look, clock_time, judgment};
use crate::tui::chat::{ChatPane, Ending, Speaker, Turn};
use crate::tui::markup::{self, Block, Emphasis};
use crate::tui::palette::{BUBBLE, CODE_GROUND, CODE_INK, READ};
use crate::tui::text;
use crate::tui::ui::{ACCENT, BOLD, CAUTION, DIM, FAILED, INK, SOFT, spinner};
use crate::tui::wrap;

/// The widest a reply runs, however wide the card: a line of prose reads
/// best around this length.
const READING_WIDTH: usize = 84;
/// The widest your words wrap to inside their bubble.
const BUBBLE_TEXT: usize = 50;
/// Cells of air either side of the words in a bubble.
const BUBBLE_PAD: usize = 2;
/// How often the streaming cursor blinks, in milliseconds.
const CURSOR_BLINK_MS: u64 = 417;

/// Every turn as lines at `width` cells, a blank line before the first and
/// after each.
pub(super) fn lines(pane: &ChatPane, width: usize, look: &Look) -> Vec<Line<'static>> {
    let mut lines = vec![Line::default()];
    let now = look.motion.real_ms();
    for turn in &pane.turns {
        match turn.speaker {
            Speaker::User => lines.extend(user(turn, width)),
            Speaker::Model => lines.extend(reply(turn, pane, width, now, look)),
        }
        lines.push(Line::default());
    }
    lines
}

/// Your words: the time over a right-aligned bubble. A judge's ask holds
/// the parts of its question instead, and its label names the kind.
fn user(turn: &Turn, width: usize) -> Vec<Line<'static>> {
    let wrap_at = BUBBLE_TEXT
        .min(width.saturating_sub(2 * BUBBLE_PAD + 1))
        .max(1);
    let parts = match &turn.asked {
        Some(asked) => judgment::ask(asked),
        None => vec![(turn.text.clone(), INK)],
    };
    let rows: Vec<(String, Style)> = parts
        .into_iter()
        .flat_map(|(text, style)| {
            wrap::wrap(&text, wrap_at)
                .into_iter()
                .map(move |row| (row, style))
        })
        .collect();
    let inner = rows.iter().map(|(row, _)| row.width()).max().unwrap_or(0);
    let bubble = (inner + 2 * BUBBLE_PAD).min(width);
    let indent = " ".repeat(width.saturating_sub(bubble));
    let edge = Style::new().fg(BUBBLE);
    let mut lines = Vec::new();
    let mut label = "you".to_owned();
    if let Some(asked) = &turn.asked {
        label.push_str(&format!(" · {}", judgment::kind(asked)));
    }
    if let Some(time) = clock_time(turn.wall_ms) {
        label.push_str(&format!(" · {time}"));
    }
    lines.push(Line::from(vec![
        Span::raw(" ".repeat(width.saturating_sub(label.width()))),
        Span::styled(label, DIM),
    ]));
    let rim = |left: &str, fill: &str, right: &str| {
        Line::from(vec![
            Span::raw(indent.clone()),
            Span::styled(
                format!("{left}{}{right}", fill.repeat(bubble.saturating_sub(2))),
                edge,
            ),
        ])
    };
    lines.push(rim("▗", "▄", "▖"));
    for (row, style) in rows {
        let pad = bubble.saturating_sub(row.width() + BUBBLE_PAD);
        lines.push(Line::from(vec![
            Span::raw(indent.clone()),
            Span::styled(
                format!("{}{row}{}", " ".repeat(BUBBLE_PAD), " ".repeat(pad)),
                style.bg(BUBBLE),
            ),
        ]));
    }
    lines.push(rim("▝", "▀", "▘"));
    lines
}

/// Where a reply is.
enum Phase {
    /// Asked of a cold model, nothing back yet.
    Loading,
    /// Asked of a warm one, nothing back yet.
    Thinking,
    Streaming,
    Ended,
}

fn phase(turn: &Turn) -> Phase {
    match (&turn.ending, turn.text.is_empty()) {
        (Ending::Open, true) if turn.cold => Phase::Loading,
        (Ending::Open, true) => Phase::Thinking,
        (Ending::Open, false) => Phase::Streaming,
        _ => Phase::Ended,
    }
}

/// A reply: the model's name with its status on the right, then its text
/// at reading width, with the streaming cursor at the end while it comes in,
/// and a failure's reason under it.
fn reply(turn: &Turn, pane: &ChatPane, width: usize, now: u64, look: &Look) -> Vec<Line<'static>> {
    let reading = width.min(READING_WIDTH);
    let name = crate::support::text::printable(pane.record.display_name()).into_owned();
    let mut status = status(turn, pane, now, look);
    let status_width: usize = status.iter().map(Span::width).sum();
    let name = text::clip(&name, reading.saturating_sub(status_width + 2).max(1));
    let gap = reading.saturating_sub(name.width() + status_width);
    let mut role = vec![
        Span::styled(name, SOFT.patch(BOLD)),
        Span::raw(" ".repeat(gap)),
    ];
    role.append(&mut status);
    let mut lines = vec![Line::from(role)];
    let phase = phase(turn);
    if let Some(asked) = &turn.asked {
        // A judgment arrives whole, so there is nothing to show of it until
        // it has ended.
        match &turn.ending {
            Ending::Done(_) => lines.extend(judgment::answer(&turn.text, asked, reading)),
            Ending::Failed(reason) => {
                for piece in wrap::wrap(&format!("failed: {reason}"), reading) {
                    lines.push(Line::from(Span::styled(piece, FAILED)));
                }
            }
            Ending::Open | Ending::Stopped => {}
        }
        return lines;
    }
    if matches!(phase, Phase::Loading | Phase::Thinking) {
        return lines;
    }
    let mut body = Vec::new();
    for block in markup::blocks(&turn.text, reading) {
        match block {
            Block::Prose(rows) => {
                for row in rows {
                    body.push(Line::from(
                        row.into_iter()
                            .map(|run| match run.emphasis {
                                Emphasis::Plain => Span::styled(run.text, Style::new().fg(READ)),
                                Emphasis::Bold => Span::styled(run.text, BOLD),
                                Emphasis::Code => Span::styled(run.text, INK.bg(BUBBLE)),
                            })
                            .collect::<Vec<_>>(),
                    ));
                }
            }
            Block::Code { lang, lines, open } => {
                body.extend(code_panel(&lang, &lines, open, reading))
            }
        }
    }
    if matches!(phase, Phase::Streaming) {
        let on = look
            .motion
            .age(0)
            .is_none_or(|age| ((age * 1000.0) as u64 / CURSOR_BLINK_MS).is_multiple_of(2));
        if let Some(last) = body.last_mut() {
            last.spans
                .push(Span::styled(if on { "▍" } else { " " }, ACCENT));
        } else {
            body.push(Line::from(Span::styled("▍", ACCENT)));
        }
    }
    lines.extend(body);
    if let Ending::Failed(reason) = &turn.ending {
        for piece in wrap::wrap(&format!("failed: {reason}"), reading) {
            lines.push(Line::from(Span::styled(piece, FAILED)));
        }
    }
    lines
}

/// The right side of a reply's name: how the reply is going.
fn status(turn: &Turn, pane: &ChatPane, now: u64, look: &Look) -> Vec<Span<'static>> {
    let elapsed = |from: u64| format!("{:.1}s", now.saturating_sub(from) as f64 / 1000.0);
    let turning = || Span::styled(format!("{} ", spinner(look.spin_frame)), ACCENT);
    if turn.asked.is_some() {
        return match (&turn.ending, turn.cold) {
            (Ending::Open, true) => vec![
                turning(),
                Span::styled(
                    format!("loading into memory · {}", elapsed(turn.at_ms)),
                    DIM,
                ),
            ],
            (Ending::Open, false) => vec![
                turning(),
                Span::styled(format!("weighing · {}", elapsed(turn.at_ms)), DIM),
            ],
            (Ending::Stopped, _) => vec![Span::styled("stopped", CAUTION)],
            (Ending::Failed(_), _) => vec![Span::styled("failed", FAILED)],
            (Ending::Done(_), _) => vec![Span::styled(
                turn.ended_ms.map_or_else(String::new, |ended| {
                    format!(
                        "judged in {:.1}s",
                        ended.saturating_sub(turn.at_ms) as f64 / 1000.0
                    )
                }),
                DIM,
            )],
        };
    }
    match (phase(turn), &turn.ending) {
        (Phase::Loading, _) => vec![
            turning(),
            Span::styled(
                format!("loading into memory · {}", elapsed(turn.at_ms)),
                DIM,
            ),
        ],
        (Phase::Thinking, _) => vec![
            turning(),
            Span::styled(format!("thinking · {}", elapsed(turn.at_ms)), DIM),
        ],
        (Phase::Streaming, _) => {
            let rate = pane
                .live_rate(now)
                .map_or_else(String::new, |rate| format!("{rate:.0} tok/s"));
            vec![turning(), Span::styled(rate, SOFT)]
        }
        (Phase::Ended, Ending::Stopped) => vec![
            Span::styled("stopped", CAUTION),
            Span::styled(format!(" · {}", tokens(turn)), DIM),
        ],
        (Phase::Ended, Ending::Failed(_)) => vec![Span::styled("failed", FAILED)],
        (Phase::Ended, _) => {
            let mut parts = vec![tokens(turn)];
            if let Some((rate, estimated)) = turn.rate() {
                parts.push(format!(
                    "{}{rate:.0} tok/s",
                    if estimated { "~" } else { "" }
                ));
            }
            if let Some(ended) = turn.ended_ms {
                parts.push(format!(
                    "{:.1}s",
                    ended.saturating_sub(turn.at_ms) as f64 / 1000.0
                ));
            }
            vec![Span::styled(parts.join(" · "), DIM)]
        }
    }
}

/// `38 tokens`, with a `~` when they were counted from the text.
fn tokens(turn: &Turn) -> String {
    let (count, estimated) = turn.tokens();
    format!(
        "{}{}",
        if estimated { "~" } else { "" },
        text::count(count as usize, "token")
    )
}

/// A fenced block on a darker panel `width` cells wide: a row with its
/// language at the right, its lines as written (a line too long is cut with
/// `…`), and a closing row once the fence has arrived.
fn code_panel(lang: &str, lines: &[String], open: bool, width: usize) -> Vec<Line<'static>> {
    let ground = Style::new().bg(CODE_GROUND);
    let row = |spans: Vec<Span<'static>>| {
        let used: usize = spans.iter().map(Span::width).sum();
        let mut spans = spans;
        spans.push(Span::styled(" ".repeat(width.saturating_sub(used)), ground));
        Line::from(spans)
    };
    let label = if lang.is_empty() { "code" } else { lang };
    let mut out = vec![row(vec![
        Span::styled(" ".repeat(width.saturating_sub(label.width() + 2)), ground),
        Span::styled(label.to_owned(), DIM.bg(CODE_GROUND)),
    ])];
    for line in lines {
        out.push(row(vec![Span::styled(
            format!("  {}", text::clip(line, width.saturating_sub(4))),
            Style::new().fg(CODE_INK).bg(CODE_GROUND),
        )]));
    }
    if !open {
        out.push(row(Vec::new()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    use kernel::capabilities::GenerationStats;

    use crate::tui::motion::Motion;
    use crate::tui::testing::{record, text, texts};

    fn look(motion: &Motion) -> Look<'_> {
        Look {
            motion,
            spin_frame: 0,
        }
    }

    fn pane_with(reply: &str, ending: Ending) -> ChatPane {
        let mut pane = ChatPane::open(record("m"));
        for c in "hi there".chars() {
            pane.edit(crate::tui::event::Key::Char(c));
        }
        let (_, generation) = pane.submit(0, 0, true).expect("sent");
        pane.append(generation, reply, 100);
        match ending {
            Ending::Open => {}
            Ending::Done(stats) => {
                pane.done(generation, stats, 1_100);
            }
            Ending::Stopped => pane.stop(600),
            Ending::Failed(reason) => {
                pane.failed(generation, reason, 900);
            }
        }
        pane
    }

    #[test]
    fn your_words_sit_right_in_a_rounded_bubble() {
        let motion = Motion::settled();
        let pane = pane_with("ok", Ending::Done(None));
        let lines = texts(&lines(&pane, 40, &look(&motion)));
        assert_eq!(lines[1], format!("{}you", " ".repeat(37)));
        assert_eq!(lines[2], format!("{}▗{}▖", " ".repeat(28), "▄".repeat(10)));
        assert_eq!(lines[3], format!("{}  hi there  ", " ".repeat(28)));
        assert_eq!(lines[4], format!("{}▝{}▘", " ".repeat(28), "▀".repeat(10)));
    }

    #[test]
    fn a_reply_says_how_it_is_going() {
        let motion = Motion::settled();
        let status_of = |pane: &ChatPane| text(&lines(pane, 60, &look(&motion))[6]);
        let mut thinking = ChatPane::open(record("m"));
        thinking.edit(crate::tui::event::Key::Char('x'));
        thinking.submit(0, 0, true);
        assert!(
            status_of(&thinking).ends_with("thinking · 0.0s"),
            "{:?}",
            status_of(&thinking)
        );
        let mut cold = ChatPane::open(record("m"));
        cold.edit(crate::tui::event::Key::Char('x'));
        cold.submit(0, 0, false);
        assert!(status_of(&cold).ends_with("loading into memory · 0.0s"));
        let stopped = pane_with("abcd", Ending::Stopped);
        assert!(
            status_of(&stopped).ends_with("stopped · ~1 token"),
            "{:?}",
            status_of(&stopped)
        );
        let stats = GenerationStats {
            completion_tokens: Some(38),
            eval_ms: Some(1000),
            ..GenerationStats::default()
        };
        let done = pane_with("ok", Ending::Done(Some(stats)));
        assert!(
            status_of(&done).ends_with("38 tokens · 38 tok/s · 1.1s"),
            "{:?}",
            status_of(&done)
        );
        let failed = pane_with("", Ending::Failed("gone".to_owned()));
        let all = texts(&lines(&failed, 60, &look(&motion)));
        assert!(all[6].ends_with("failed"));
        assert!(all.contains(&"failed: gone".to_owned()));
    }

    #[test]
    fn a_code_block_is_a_panel_with_its_language_and_clipped_lines() {
        let motion = Motion::settled();
        let reply = format!("run this:\n```sh\n{}\n```", "x".repeat(100));
        let pane = pane_with(&reply, Ending::Done(None));
        let lines = lines(&pane, 40, &look(&motion));
        let read = texts(&lines);
        let label = read
            .iter()
            .position(|line| line.trim_end().ends_with("sh"))
            .expect("a label row");
        assert!(read[label + 1].starts_with("  xxx") && read[label + 1].trim_end().ends_with('…'));
        for line in &lines {
            assert!(line.width() <= 40, "{:?} runs past the card", text(line));
        }
        assert_eq!(lines[label + 1].spans[0].style.bg, Some(CODE_GROUND));
    }

    #[test]
    fn inline_code_wears_a_chip_and_replies_keep_to_reading_width() {
        let motion = Motion::settled();
        let pane = pane_with(
            &format!("use `hedos pull` {}", "word ".repeat(40)),
            Ending::Done(None),
        );
        let lines = lines(&pane, 120, &look(&motion));
        let chip = lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .find(|span| span.content.contains("hedos pull"))
            .expect("the code run");
        assert_eq!(chip.style.bg, Some(BUBBLE));
        for line in lines.iter().skip(6) {
            assert!(line.width() <= READING_WIDTH, "{:?}", text(line));
        }
    }

    #[test]
    fn a_streaming_reply_ends_in_the_cursor() {
        let motion = Motion::settled();
        let pane = pane_with("partial", Ending::Open);
        let read = texts(&lines(&pane, 60, &look(&motion)));
        assert!(read.iter().any(|line| line == "partial▍"), "{read:?}");
    }
}
