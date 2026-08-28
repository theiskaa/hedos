//! The key line: the keys that always apply on the left, then what the
//! selected model can do, each key dim and its verb plain; help and quit sit
//! against the right edge. A notice takes the line over while it lasts.
//! Designed for 100 columns and up; narrower, the actions go one at a time
//! from the right, then the serve key, then the scan key, so help and quit
//! always show.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{BOLD, DIM, key_spans, keys};
use crate::tui::app::App;
use crate::tui::keymap;

/// The keys that apply whatever is selected.
const FIXED: [&str; 5] = ["j/k", "/", "p", "s", "S"];
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
/// the app lists them, then the serve key goes, then the scan key.
fn fitting_line(actions: &[&str], width: usize) -> Line<'static> {
    let shed_actions = (0..=actions.len())
        .rev()
        .map(|kept| footer_line(&FIXED, &actions[..kept], width));
    let shed_serve = std::iter::once_with(|| footer_line(&FIXED[..FIXED.len() - 1], &[], width));
    shed_actions
        .chain(shed_serve)
        .find(|line| line.width() < width)
        .unwrap_or_else(|| footer_line(&FIXED[..FIXED.len() - 2], &[], width))
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

    #[test]
    fn the_footer_sheds_from_the_right_until_it_fits() {
        let at = |width: usize| {
            let line = fitting_line(&ALL, width);
            let shown = text(&line);
            assert!(line.width() < width, "{shown:?} has no margin at {width}");
            assert!(shown.ends_with("? help  q quit"), "{shown:?}");
            shown
        };
        let wide = at(200);
        assert!(wide.starts_with(
            " j/k move  / filter  p pull  s scan  S serve  │ w warm  l launch  t try  T chat  x remove  y copy path"
        ));
        assert_eq!(wide.chars().count(), 199);
        let medium = at(110);
        assert!(!medium.contains("copy path") && medium.contains("x remove"));
        let ninety = at(90);
        assert!(ninety.contains("l launch  t try") && !ninety.contains("T chat"));
        let one_action = at(80);
        assert!(one_action.contains("│ w warm") && !one_action.contains("l launch"));
        let seventy = at(70);
        assert!(!seventy.contains('│') && seventy.contains("S serve"));
        let sixty = at(60);
        assert!(!sixty.contains("S serve") && sixty.contains("s scan"));
        let tiny = at(50);
        assert!(!tiny.contains("s scan") && tiny.contains("p pull"));
    }

    #[test]
    fn escape_reads_as_stop_while_a_reply_streams() {
        assert!(text(&chat_line(true)).contains("esc stop"));
        assert!(text(&chat_line(false)).contains("esc close"));
    }
}
