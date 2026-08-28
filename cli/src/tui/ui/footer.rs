//! The key line: the keys that always apply on the left, then what the
//! selected model can do, each key dim and its verb plain; help and quit sit
//! against the right edge. A notice takes the line over while it lasts.
//! Designed for 100 columns and up, where four actions show; narrower, the
//! sort and expand keys go first, then the actions one at a time from the
//! right, then the core keys the same way down to the move key, so help and
//! quit always show.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{BOLD, DIM, key_spans, keys};
use crate::tui::app::App;
use crate::tui::keymap;

/// The keys that apply whatever is selected and are worth the room before
/// any action; the first is the floor.
const CORE: [&str; 5] = ["j/k", "/", "p", "s", "S"];
/// The keys shown after the core when there is room, shed before any action,
/// the last first.
const EXTRAS: [&str; 2] = ["enter", "o"];
/// The keys that close the line.
const ALWAYS: [&str; 2] = ["?", "q"];

/// A key and its verb.
type Pair = (&'static str, &'static str);
/// One footer worth trying: the fixed keys, then the actions.
type Candidate = (Vec<Pair>, Vec<Pair>);

/// Draw the key line, or the current notice, into `area`.
pub(super) fn draw(frame: &mut Frame, area: Rect, app: &App) {
    let line = match (app.notice(), app.chat_pane()) {
        (Some(notice), _) => Line::from(Span::styled(format!(" {notice}"), BOLD)),
        (None, Some(pane)) => chat_line(pane.streaming()),
        (None, None) => fitting_line(&app.actions(), area.width as usize, app.expanded),
    };
    frame.render_widget(Paragraph::new(line), area);
}

/// The keys of the chat pane: while a reply streams, escape reads as stop
/// and there is nothing to send.
fn chat_line(streaming: bool) -> Line<'static> {
    if streaming {
        keys(&[("↑/↓", "scroll"), ("esc", "stop")])
    } else {
        keys(&[("enter", "send"), ("↑/↓", "scroll"), ("esc", "close")])
    }
}

/// The extra pairs: `enter` says what it does to the detail now, `expand`
/// or `collapse`.
fn extra_pairs(expanded: bool) -> Vec<Pair> {
    keymap::pairs(&EXTRAS)
        .into_iter()
        .map(|(key, verb)| match (key, expanded) {
            ("enter", true) => (key, "collapse"),
            _ => (key, verb),
        })
        .collect()
}

/// Every footer worth trying, fullest first: the core with both extras and
/// every action, then the extras shed from the last, then the actions from
/// the right in the reverse of the order the app lists them, then the core
/// keys the same way down to the move key.
fn candidates(actions: &[&str], expanded: bool) -> Vec<Candidate> {
    let core = keymap::pairs(&CORE);
    let extras = extra_pairs(expanded);
    let actions = keymap::pairs(actions);
    let mut candidates = Vec::new();
    for kept in (0..=extras.len()).rev() {
        let mut fixed = core.clone();
        fixed.extend(&extras[..kept]);
        candidates.push((fixed, actions.clone()));
    }
    for kept in (0..actions.len()).rev() {
        candidates.push((core.clone(), actions[..kept].to_vec()));
    }
    for kept in (1..core.len()).rev() {
        candidates.push((core[..kept].to_vec(), Vec::new()));
    }
    candidates
}

/// The fullest of the [`candidates`] that fits in `width` with its right
/// margin; the move key alone is the floor and runs on below it.
fn fitting_line(actions: &[&str], width: usize, expanded: bool) -> Line<'static> {
    let candidates = candidates(actions, expanded);
    let (fixed, actions) = candidates
        .iter()
        .find(|(fixed, actions)| footer_line(fixed, actions, width).width() < width)
        .or(candidates.last())
        .map_or((&[][..], &[][..]), |(fixed, actions)| {
            (fixed.as_slice(), actions.as_slice())
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
        spans.push(Span::styled("│ ", DIM));
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

    use crate::tui::testing::line_text as text;

    #[test]
    fn every_footer_key_is_bound() {
        for key in CORE.iter().chain(&EXTRAS).chain(&ALWAYS).chain(&ALL) {
            assert!(keymap::binding(key).is_some(), "{key} is not bound");
        }
    }

    /// What a candidate is made of, for reading a failure.
    fn signature(candidate: &Candidate) -> String {
        text(&footer_line(&candidate.0, &candidate.1, 0))
    }

    /// Each candidate takes over exactly where the one before it stops
    /// fitting: at one cell over its own width it shows, at its own width
    /// the next one does. The numbers come from the pairs, not by hand.
    #[test]
    fn the_footer_sheds_from_the_right_until_it_fits() {
        let steps = candidates(&ALL, false);
        assert_eq!(steps.len(), 1 + EXTRAS.len() + ALL.len() + CORE.len() - 1);
        let widths: Vec<usize> = steps
            .iter()
            .map(|(fixed, actions)| footer_line(fixed, actions, 0).width())
            .collect();
        assert!(
            widths.windows(2).all(|pair| pair[0] > pair[1]),
            "{widths:?}"
        );
        for (index, (fixed, actions)) in steps.iter().enumerate() {
            let edge = widths[index];
            let shown = fitting_line(&ALL, edge + 1, false);
            assert!(shown.width() < edge + 1);
            assert_eq!(
                text(&shown),
                text(&footer_line(fixed, actions, edge + 1)),
                "at {} the footer is not {:?}",
                edge + 1,
                signature(&steps[index])
            );
            assert!(text(&shown).ends_with("? help  q quit"));
            if let Some((next_fixed, next_actions)) = steps.get(index + 1) {
                assert_eq!(
                    text(&fitting_line(&ALL, edge, false)),
                    text(&footer_line(next_fixed, next_actions, edge)),
                    "at {edge} the footer is not {:?}",
                    signature(&steps[index + 1])
                );
            }
        }
        let signatures: Vec<String> = steps.iter().map(signature).collect();
        assert!(signatures[0].starts_with(
            " j/k move  / filter  p pull  s scan  S serve  enter expand  o sort  │ w warm"
        ));
        assert!(signatures[0].contains("y copy path"));
        assert!(!signatures[1].contains("o sort") && signatures[1].contains("enter expand"));
        assert!(!signatures[2].contains("enter expand") && signatures[2].contains("y copy path"));
        assert!(!signatures[3].contains("y copy path") && signatures[3].contains("x remove"));
        let fixed_only = &signatures[2 + ALL.len()];
        assert_eq!(
            fixed_only,
            " j/k move  / filter  p pull  s scan  S serve  ? help  q quit"
        );
        assert_eq!(
            signatures.last().map(String::as_str),
            Some(" j/k move  ? help  q quit")
        );
        let hundred = text(&fitting_line(&ALL, 100, false));
        assert!(hundred.contains("│ w warm  l launch  t try  T chat  "));
        assert!(!hundred.contains("x remove") && !hundred.contains("enter"));
        let floor = fitting_line(&ALL, widths[widths.len() - 1], false);
        assert_eq!(text(&floor), " j/k move  ? help  q quit");
        assert!(floor.width() >= widths[widths.len() - 1]);
    }

    #[test]
    fn enter_says_collapse_while_the_detail_is_open() {
        let expanded = text(&fitting_line(&ALL, 200, true));
        assert!(expanded.contains("enter collapse") && !expanded.contains("expand"));
        let collapsed = text(&fitting_line(&ALL, 200, false));
        assert!(collapsed.contains("enter expand"));
    }

    #[test]
    fn escape_reads_as_stop_while_a_reply_streams() {
        let streaming = text(&chat_line(true));
        assert_eq!(streaming.trim_end(), " ↑/↓ scroll  esc stop");
        assert!(!streaming.contains("send"));
        let idle = text(&chat_line(false));
        assert_eq!(idle.trim_end(), " enter send  ↑/↓ scroll  esc close");
    }
}
