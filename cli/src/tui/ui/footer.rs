//! The key line: the keys that always apply on the left, then what the
//! selected model can do, each key dim and its verb plain; help and quit sit
//! against the right edge. A notice takes the line over while it lasts.
//! Designed for 100 columns and up; narrower, the actions go one at a time
//! from the right, then the fixed keys the same way down to the move key,
//! so help and quit always show.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{BOLD, DIM, key_spans, keys};
use crate::tui::app::App;
use crate::tui::keymap;

/// The keys that apply whatever is selected.
const FIXED: [&str; 7] = ["j/k", "enter", "/", "o", "p", "s", "S"];
/// The keys that close the line.
const ALWAYS: [&str; 2] = ["?", "q"];

/// Draw the key line, or the current notice, into `area`.
pub(super) fn draw(frame: &mut Frame, area: Rect, app: &App) {
    let line = match (app.notice(), app.chat_pane()) {
        (Some(notice), _) => Line::from(Span::styled(format!(" {notice}"), BOLD)),
        (None, Some(pane)) => chat_line(pane.streaming()),
        (None, None) => fitting_line(&app.actions(), area.width as usize),
    };
    frame.render_widget(Paragraph::new(line), area);
}

/// The keys of the chat pane; escape reads as stop while a reply streams.
fn chat_line(streaming: bool) -> Line<'static> {
    let escape = if streaming { "stop" } else { "close" };
    keys(&[("enter", "send"), ("↑/↓", "scroll"), ("esc", escape)])
}

/// The fullest footer that fits in `width` with its right margin: the
/// actions shed one at a time from the right, in the reverse of the order
/// the app lists them, then the fixed keys the same way, the move key the
/// floor.
fn fitting_line(actions: &[&str], width: usize) -> Line<'static> {
    let shed_actions = (0..=actions.len())
        .rev()
        .map(|kept| footer_line(&FIXED, &actions[..kept], width));
    let shed_fixed = (1..FIXED.len())
        .rev()
        .map(|kept| footer_line(&FIXED[..kept], &[], width));
    shed_actions
        .chain(shed_fixed)
        .find(|line| line.width() < width)
        .unwrap_or_else(|| footer_line(&FIXED[..1], &[], width))
}

/// `fixed`, a divider, `actions`, then the closing keys pushed one cell in
/// from the right edge of `width`, mirroring the leading space, so a line
/// that fits is one cell under `width`; wider than that, the line runs on.
fn footer_line(fixed: &[&str], actions: &[&str], width: usize) -> Line<'static> {
    let mut spans = vec![Span::raw(" ")];
    spans.extend(key_spans(&keymap::pairs(fixed)));
    if !actions.is_empty() {
        spans.push(Span::styled("│ ", DIM));
        spans.extend(key_spans(&keymap::pairs(actions)));
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
        for key in FIXED.iter().chain(&ALWAYS).chain(&ALL) {
            assert!(keymap::binding(key).is_some(), "{key} is not bound");
        }
    }

    /// The full line fits down to 141 columns; each action shed takes its
    /// `key verb  ` cells off (five fit down to 128, four to 118, three to
    /// 110, two to 103, one to 93), then the divider goes with the last one
    /// and the fixed keys alone fit down to 83, then `S serve  ` goes (74),
    /// `s scan  ` (66), `p pull  ` (58), `o sort  ` (50), `/ filter  ` (40)
    /// and `enter expand  ` (26); `j/k move` is the floor and runs on below
    /// that.
    #[test]
    fn the_footer_sheds_from_the_right_until_it_fits() {
        let at = |width: usize| {
            let line = fitting_line(&ALL, width);
            let shown = text(&line);
            assert!(line.width() < width, "{shown:?} has no margin at {width}");
            assert!(shown.ends_with("? help  q quit"), "{shown:?}");
            shown
        };
        const FIXED_LINE: &str =
            " j/k move  enter expand  / filter  o sort  p pull  s scan  S serve";
        let wide = at(200);
        assert!(wide.starts_with(&format!(
            "{FIXED_LINE}  │ w warm  l launch  t try  T chat  x remove  y copy path"
        )));
        assert_eq!(wide.chars().count(), 199);
        assert!(at(141).contains("y copy path"));
        let five = at(140);
        assert!(!five.contains("copy path") && five.contains("x remove"));
        assert!(at(128).contains("x remove"));
        let four = at(127);
        assert!(!four.contains("x remove") && four.contains("T chat"));
        assert!(at(118).contains("T chat"));
        let three = at(117);
        assert!(!three.contains("T chat") && three.contains("t try"));
        assert!(at(110).contains("t try"));
        let two = at(109);
        assert!(!two.contains("t try") && two.contains("l launch"));
        assert!(at(103).contains("l launch"));
        let one = at(102);
        assert!(!one.contains("l launch") && one.contains("│ w warm"));
        assert!(at(93).contains("│ w warm"));
        let fixed_only = at(92);
        assert!(fixed_only.starts_with(FIXED_LINE) && !fixed_only.contains('│'));
        assert!(at(83).starts_with(FIXED_LINE));
        let no_serve = at(82);
        assert!(!no_serve.contains("S serve") && no_serve.contains("s scan"));
        assert!(at(74).contains("s scan"));
        let no_scan = at(73);
        assert!(!no_scan.contains("s scan") && no_scan.contains("p pull"));
        assert!(at(66).contains("p pull"));
        let no_pull = at(65);
        assert!(!no_pull.contains("p pull") && no_pull.contains("o sort"));
        assert!(at(58).contains("o sort"));
        let no_sort = at(57);
        assert!(!no_sort.contains("o sort") && no_sort.contains("/ filter"));
        assert!(at(50).contains("/ filter"));
        let no_filter = at(49);
        assert!(!no_filter.contains("/ filter") && no_filter.contains("enter expand"));
        assert!(at(40).contains("enter expand"));
        let move_only = at(39);
        assert!(!move_only.contains("enter expand") && move_only.starts_with(" j/k move"));
        assert_eq!(at(26), " j/k move  ? help  q quit");
        let floor = fitting_line(&ALL, 20);
        assert_eq!(text(&floor), " j/k move  ? help  q quit");
        assert!(floor.width() >= 20);
    }

    #[test]
    fn escape_reads_as_stop_while_a_reply_streams() {
        assert!(text(&chat_line(true)).contains("esc stop"));
        assert!(text(&chat_line(false)).contains("esc close"));
    }
}
