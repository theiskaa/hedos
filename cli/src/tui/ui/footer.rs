//! The key line: the keys that always apply on the left, then what the
//! selected model can do, each key bright and its verb dim; help and quit sit
//! against the right edge. A notice takes the line over while it lasts.
//! Designed for 100 columns and up, where four actions show; narrower, the
//! pulls, sort and expand keys go first, then the actions one at a time from
//! the right, then the core keys the same way down to the move key, so help
//! and quit always show.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{TRACK, key_spans, keys};
use crate::tui::app::{App, Screen};
use crate::tui::keymap::{self, Pair};
use crate::tui::motion::Motion;
use crate::tui::palette::{FAINT_COLOR, INK_COLOR, PAPER, SOFT_COLOR, mix};
use crate::tui::stop::StopCard;

/// The keys that apply whatever is selected and are worth the room before
/// any action; the first is the floor.
const CORE: [&str; 5] = ["j/k", "/", "p", "s", "S"];
/// The keys shown after the core when there is room, shed before any action,
/// the last first: the pulls screen goes before the sort, since the strip
/// already shows what is pulling.
const EXTRAS: [&str; 3] = ["enter", "o", "P"];
/// The keys that close the line.
const ALWAYS: [&str; 2] = ["?", "q"];
/// The pulls screen's keys that apply whatever is selected.
const PULLS_FIXED: [&str; 3] = ["j/k", "p", "esc"];
/// The bench screen's keys that apply whatever is selected.
const BENCH_FIXED: [&str; 3] = ["j/k", "a", "esc"];

/// One footer worth trying.
struct Candidate {
    /// The keys that apply whatever is selected.
    fixed: Vec<Pair>,
    /// What the selected model can do.
    actions: Vec<Pair>,
}

/// Draw the key line, or the current notice, into `area`.
pub(super) fn draw(frame: &mut Frame, area: Rect, app: &App) {
    let line = match (app.notice(), app.chat_pane()) {
        (Some(notice), _) => notice_line(notice, app.notice_at().unwrap_or(0), &app.motion),
        (None, Some(pane)) if pane.judging() => judge_line(pane.streaming()),
        (None, Some(pane)) => chat_line(pane.streaming()),
        (None, None) if app.pull_screen().is_some() => pull_line(
            app.pull_screen()
                .is_some_and(|modal| !modal.input.is_empty()),
        ),
        (None, None) => match app.screen {
            Screen::Shelf if app.filtering => keys(&[("enter", "keep"), ("esc", "clear")]),
            Screen::Shelf => fitting_line(
                &app.actions(),
                area.width as usize,
                app.selected_record().map(|_| app.expanded),
            ),
            Screen::Pulls => pulls_line(app, area.width as usize),
            Screen::Bench => bench_line(app, area.width as usize),
        },
    };
    frame.render_widget(Paragraph::new(line), area);
}

/// How fast a notice arrives, how long it stays, and how long it takes to
/// fade before it goes.
const NOTICE_PER_MS: u64 = 3;
const NOTICE_MS: f32 = 2000.0;
const NOTICE_FADE_MS: f32 = 300.0;

/// ` › m has no path`: the text arriving letter by letter, bold, and
/// fading into the ground over the moment before it goes.
fn notice_line(notice: &str, shown_at: u64, motion: &Motion) -> Line<'static> {
    let fade = motion.age(shown_at).map_or(1.0, |age| {
        ((NOTICE_MS - age * 1000.0) / NOTICE_FADE_MS).clamp(0.0, 1.0)
    });
    let ink = |colour| Style::new().fg(mix(PAPER, colour, fade));
    let mut spans = vec![
        Span::raw(" "),
        Span::styled("›", ink(SOFT_COLOR)),
        Span::raw(" "),
    ];
    spans.extend(
        motion
            .reveal(notice, shown_at, NOTICE_PER_MS)
            .into_iter()
            .map(|(piece, ramp)| {
                if ramp {
                    Span::styled(piece, ink(FAINT_COLOR))
                } else {
                    Span::styled(piece, ink(INK_COLOR).add_modifier(Modifier::BOLD))
                }
            }),
    );
    Line::from(spans)
}

/// The keys of the try screen: while a reply streams, escape reads as stop
/// and there is nothing to send. No closing keys: `?` and `q` type here.
fn chat_line(streaming: bool) -> Line<'static> {
    if streaming {
        keys(&[("esc", "stop"), ("↑/↓", "scroll")])
    } else {
        keys(&[
            ("enter", "send"),
            ("↑/↓", "scroll"),
            ("tab", "suggest"),
            ("⌃l", "clear"),
            ("esc", "shelf"),
        ])
    }
}

/// The judge's keys: the fields are walked with tab; what enter does in
/// each is said in the composer.
fn judge_line(weighing: bool) -> Line<'static> {
    if weighing {
        keys(&[("esc", "stop"), ("↑/↓", "scroll")])
    } else {
        keys(&[
            ("tab", "next field"),
            ("↑/↓", "scroll"),
            ("⌃l", "clear"),
            ("esc", "shelf"),
        ])
    }
}

/// The pull screen's keys: no closing keys, since every letter types into
/// the search; `esc` clears what was typed before it leaves.
fn pull_line(typed: bool) -> Line<'static> {
    let mut spans = vec![Span::raw(" ")];
    spans.extend(key_spans(&[
        ("↑/↓", "move"),
        ("tab", "kind"),
        ("enter", "pull"),
    ]));
    spans.push(Span::styled("│ ", TRACK));
    spans.extend(key_spans(&[("esc", if typed { "clear" } else { "back" })]));
    Line::from(spans)
}

/// The pulls screen's keys: moving, pulling and the way back, then what the
/// selected pull answers to; narrower, the actions go from the right, then
/// the fixed keys the same way down to the move key, which is the floor.
fn pulls_line(app: &App, width: usize) -> Line<'static> {
    screen_line(
        &keymap::PULLS.pairs(&PULLS_FIXED),
        &keymap::PULLS.pairs(&pulls_actions(app)),
        width,
    )
}

/// The bench screen's keys: moving, benching everything, and the way back,
/// then what the bench going answers to.
fn bench_line(app: &App, width: usize) -> Line<'static> {
    screen_line(
        &keymap::BENCH.pairs(&BENCH_FIXED),
        &keymap::BENCH.pairs(&bench_actions(app)),
        width,
    )
}

/// A screen's key line: the fixed keys, then its actions; narrower, the actions
/// go from the right, then the fixed keys the same way down to the move key,
/// which is the floor.
fn screen_line(fixed: &[Pair], actions: &[Pair], width: usize) -> Line<'static> {
    let candidates = (0..=actions.len())
        .rev()
        .map(|kept| (fixed, &actions[..kept]))
        .chain((1..fixed.len()).rev().map(|kept| (&fixed[..kept], &[][..])));
    candidates
        .map(|(fixed, actions)| footer_line(fixed, actions, width))
        .find(|line| line.width() < width)
        .unwrap_or_else(|| footer_line(&fixed[..1], &[], width))
}

/// The keys the bench answers to right now: stopping it only while it runs,
/// and re-measuring one row only while it does not.
fn bench_actions(app: &App) -> Vec<&'static str> {
    if app.bench.running() {
        vec!["c"]
    } else if app.bench.selected_row().is_some() {
        vec!["b"]
    } else {
        Vec::new()
    }
}

/// The keys the selected pull answers to, in footer order.
fn pulls_actions(app: &App) -> Vec<&'static str> {
    let Some(row) = app.pulls.selected_row() else {
        return Vec::new();
    };
    let mut actions = Vec::new();
    // The one rule the key itself uses, so the footer never offers a stop the
    // screen would refuse.
    if StopCard::can_stop(row) {
        actions.push("c");
    }
    if row.pull_state.is_resumable() {
        actions.push("R");
    }
    if row.pull_state.is_terminal() {
        actions.push("x");
    }
    actions.push("Y");
    actions
}

/// The extra pairs: `enter` says what it does to the detail now, `expand`
/// or `collapse`, by `expanded`; with nothing selected it does nothing and
/// is left out.
fn extra_pairs(expanded: Option<bool>) -> Vec<Pair> {
    keymap::pairs(&EXTRAS)
        .into_iter()
        .filter_map(|(key, verb)| match (key, expanded) {
            ("enter", None) => None,
            ("enter", Some(true)) => Some((key, "collapse")),
            _ => Some((key, verb)),
        })
        .collect()
}

/// Every footer worth trying, fullest first: the core with both extras and
/// every action, then the extras shed from the last, then the actions from
/// the right in the reverse of the order the app lists them, then the core
/// keys the same way down to the move key.
fn candidates(actions: &[&str], expanded: Option<bool>) -> Vec<Candidate> {
    let core = keymap::pairs(&CORE);
    let extras = extra_pairs(expanded);
    let actions = keymap::pairs(actions);
    let mut candidates = Vec::new();
    for kept in (0..=extras.len()).rev() {
        let mut fixed = core.clone();
        fixed.extend(&extras[..kept]);
        candidates.push(Candidate {
            fixed,
            actions: actions.clone(),
        });
    }
    for kept in (0..actions.len()).rev() {
        candidates.push(Candidate {
            fixed: core.clone(),
            actions: actions[..kept].to_vec(),
        });
    }
    for kept in (1..core.len()).rev() {
        candidates.push(Candidate {
            fixed: core[..kept].to_vec(),
            actions: Vec::new(),
        });
    }
    candidates
}

/// The fullest of the [`candidates`] that fits in `width` with its right
/// margin; the move key alone is the floor and runs on below it.
fn fitting_line(actions: &[&str], width: usize, expanded: Option<bool>) -> Line<'static> {
    let candidates = candidates(actions, expanded);
    let (fixed, actions) = candidates
        .iter()
        .find(|candidate| footer_line(&candidate.fixed, &candidate.actions, width).width() < width)
        .or(candidates.last())
        .map_or((&[][..], &[][..]), |candidate| {
            (candidate.fixed.as_slice(), candidate.actions.as_slice())
        });
    footer_line(fixed, actions, width)
}

/// `fixed`, a divider, `actions`, then the closing keys pushed one cell in
/// from the right edge of `width`, mirroring the leading space, so a line
/// that fits is one cell under `width`; wider than that, the line runs on.
fn footer_line(fixed: &[Pair], actions: &[Pair], width: usize) -> Line<'static> {
    let mut spans = vec![Span::raw(" ")];
    spans.extend(key_spans(fixed));
    if !actions.is_empty() {
        spans.push(Span::styled("│ ", TRACK));
        spans.extend(key_spans(actions));
    }
    let mut closing = key_spans(&keymap::pairs(&ALWAYS));
    if let Some(last) = closing.last_mut() {
        last.content = last.content.trim_end().to_owned().into();
    }
    let left: usize = spans.iter().map(Span::width).sum();
    let right = closing.iter().map(Span::width).sum::<usize>() + 1;
    let pad = width.saturating_sub(left + right);
    spans.push(Span::raw(" ".repeat(pad)));
    spans.extend(closing);
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [&str; 6] = ["w", "l", "t", "T", "x", "y"];

    use crate::tui::testing::text;

    #[test]
    fn every_footer_key_is_bound() {
        for key in CORE.iter().chain(&EXTRAS).chain(&ALWAYS).chain(&ALL) {
            assert!(keymap::binding(key).is_some(), "{key} is not bound");
        }
        assert_eq!(keymap::PULLS.pairs(&PULLS_FIXED).len(), PULLS_FIXED.len());
    }

    #[test]
    fn the_pulls_line_offers_what_the_selected_pull_answers_to_and_sheds_to_fit() {
        use kernel::install::pulls::PullState;

        use crate::tui::facts::Facts;
        use crate::tui::tasks::TaskState;
        use crate::tui::testing::{downloading, job_row};

        let mut app = App::new(Vec::new(), Facts::default());
        let empty = text(&pulls_line(&app, 100));
        assert!(empty.starts_with(" j/k move  p pull  esc shelf   ") && !empty.contains('│'));
        assert!(empty.ends_with("? help  q quit"));
        assert_eq!(Line::from(empty.as_str()).width(), 99);
        app.pulls.sync(&[downloading("going")]);
        app.pulls.select_newest_live();
        let live = text(&pulls_line(&app, 100));
        assert!(live.contains("│ c stop  Y copy id") && !live.contains("R resume"));
        // A pull being registered reads no control file, so the key that would
        // reach it is not offered rather than offered and then refused.
        let mut registering = downloading("registering");
        crate::support::pulls::testing::mark_registering(&mut registering.status);
        app.pulls.sync(&[registering]);
        let deaf = text(&pulls_line(&app, 100));
        assert!(!deaf.contains("c stop"), "{deaf}");

        app.pulls.sync(&[job_row(
            "paused",
            PullState::Paused,
            TaskState::Stopped("paused".to_owned()),
        )]);
        let stopped = text(&pulls_line(&app, 100));
        assert!(stopped.contains("│ R resume  Y copy id") && !stopped.contains("c stop"));
        app.pulls.sync(&[job_row(
            "done",
            PullState::Done,
            TaskState::Done("pulled done".to_owned()),
        )]);
        let ended = text(&pulls_line(&app, 100));
        assert!(ended.contains("│ x forget  Y copy id") && !ended.contains("R resume"));

        // 60 columns: the last action goes and the first stays; 48: the
        // actions go, the way back stays; 40: the way back goes too; help
        // and quit stay throughout.
        let one_action = text(&pulls_line(&app, 60));
        assert!(one_action.contains("│ x forget  ") && !one_action.contains("copy id"));
        let narrow = text(&pulls_line(&app, 48));
        assert!(Line::from(narrow.as_str()).width() < 48, "{narrow:?}");
        assert!(
            narrow.starts_with(" j/k move  p pull  esc shelf")
                && !narrow.contains('│')
                && narrow.ends_with("? help  q quit")
        );
        let without_back = text(&pulls_line(&app, 40));
        assert!(without_back.starts_with(" j/k move  p pull  ") && !without_back.contains("esc"));
        let floor = text(&pulls_line(&app, 30));
        assert!(floor.starts_with(" j/k move  ") && floor.ends_with("? help  q quit"));
        assert!(!floor.contains("esc"));
    }

    /// What a candidate is made of, for reading a failure.
    fn signature(candidate: &Candidate) -> String {
        text(&footer_line(&candidate.fixed, &candidate.actions, 0))
    }

    /// Each candidate takes over exactly where the one before it stops
    /// fitting: at one cell over its own width it shows, at its own width
    /// the next one does. The numbers come from the pairs, not by hand.
    #[test]
    fn the_footer_sheds_from_the_right_until_it_fits() {
        let steps = candidates(&ALL, Some(false));
        assert_eq!(steps.len(), 1 + EXTRAS.len() + ALL.len() + CORE.len() - 1);
        let widths: Vec<usize> = steps
            .iter()
            .map(|candidate| footer_line(&candidate.fixed, &candidate.actions, 0).width())
            .collect();
        assert!(
            widths.windows(2).all(|pair| pair[0] > pair[1]),
            "{widths:?}"
        );
        for (index, candidate) in steps.iter().enumerate() {
            let edge = widths[index];
            let shown = fitting_line(&ALL, edge + 1, Some(false));
            assert!(shown.width() < edge + 1);
            assert_eq!(
                text(&shown),
                text(&footer_line(&candidate.fixed, &candidate.actions, edge + 1)),
                "at {} the footer is not {:?}",
                edge + 1,
                signature(&steps[index])
            );
            assert!(text(&shown).ends_with("? help  q quit"));
            if let Some(next) = steps.get(index + 1) {
                assert_eq!(
                    text(&fitting_line(&ALL, edge, Some(false))),
                    text(&footer_line(&next.fixed, &next.actions, edge)),
                    "at {edge} the footer is not {:?}",
                    signature(&steps[index + 1])
                );
            }
        }
        let signatures: Vec<String> = steps.iter().map(signature).collect();
        assert!(signatures[0].starts_with(
            " j/k move  / filter  p pull  s scan  S serve  enter expand  o sort  P pulls  │ w warm"
        ));
        assert!(signatures[0].contains("y copy path"));
        assert!(!signatures[1].contains("P pulls") && signatures[1].contains("o sort"));
        assert!(!signatures[2].contains("o sort") && signatures[2].contains("enter expand"));
        assert!(!signatures[3].contains("enter expand") && signatures[3].contains("y copy path"));
        assert!(!signatures[4].contains("y copy path") && signatures[4].contains("x remove"));
        // Past the extras shed one by one and every action shed the same
        // way, the core stands alone.
        let fixed_only = &signatures[EXTRAS.len() + ALL.len()];
        assert_eq!(
            fixed_only,
            " j/k move  / filter  p pull  s scan  S serve  ? help  q quit"
        );
        assert_eq!(
            signatures.last().map(String::as_str),
            Some(" j/k move  ? help  q quit")
        );
        let hundred = text(&fitting_line(&ALL, 100, Some(false)));
        assert!(hundred.contains("│ w warm  l launch  t try  T chat  "));
        assert!(!hundred.contains("x remove") && !hundred.contains("enter"));
        let floor = fitting_line(&ALL, widths[widths.len() - 1], Some(false));
        assert_eq!(text(&floor), " j/k move  ? help  q quit");
        assert!(floor.width() >= widths[widths.len() - 1]);
    }

    #[test]
    fn a_notice_arrives_letter_by_letter_and_settles_whole() {
        let mut live = Motion::from_env_value_for_tests(None);
        live.set(4);
        let arriving = text(&notice_line("ab", 0, &live));
        assert_eq!(arriving, " › :.");
        let settled = notice_line("ab", 0, &Motion::settled());
        assert_eq!(text(&settled), " › ab");
        assert!(settled.spans[3].style.add_modifier.contains(Modifier::BOLD));
        live.set(1990);
        let fading = notice_line("ab", 0, &live);
        assert_ne!(
            fading.spans[3].style.fg,
            Some(INK_COLOR),
            "it fades before it goes"
        );
    }

    #[test]
    fn enter_says_collapse_while_the_detail_is_open() {
        let expanded = text(&fitting_line(&ALL, 200, Some(true)));
        assert!(expanded.contains("enter collapse") && !expanded.contains("expand"));
        let collapsed = text(&fitting_line(&ALL, 200, Some(false)));
        assert!(collapsed.contains("enter expand"));
    }

    #[test]
    fn escape_reads_as_stop_while_a_reply_streams() {
        let streaming = text(&chat_line(true));
        assert_eq!(streaming.trim_end(), " esc stop  ↑/↓ scroll");
        assert!(!streaming.contains("send"));
        let idle = text(&chat_line(false));
        assert_eq!(
            idle.trim_end(),
            " enter send  ↑/↓ scroll  tab suggest  ⌃l clear  esc shelf"
        );
    }
}
