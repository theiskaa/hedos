//! The key line: the keys that always apply on the left, what the selected
//! model can do on the right, each key dim and its verb plain. A notice takes
//! the line over while it lasts. Designed for 100 columns and up; narrower,
//! the actions go first, then the serve key, so help and quit always show.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{BOLD, DIM, key_spans};
use crate::tui::app::App;

/// The keys that apply whatever is selected.
const FIXED: [(&str, &str); 5] = [
    ("j/k", "move"),
    ("/", "filter"),
    ("p", "pull"),
    ("s", "scan"),
    ("S", "serve"),
];
/// The keys that close the line.
const ALWAYS: [(&str, &str); 2] = [("?", "help"), ("q", "quit")];

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
    let mut spans = vec![Span::raw(" ")];
    spans.extend(key_spans(&[
        ("enter", "send"),
        ("↑/↓", "scroll"),
        ("esc", escape),
    ]));
    Line::from(spans)
}

/// The fullest footer that fits in `width`: everything, then without the copy
/// key, then without the model's actions, then without the serve key.
fn fitting_line(actions: &[(&str, &str)], width: usize) -> Line<'static> {
    let without_copy: Vec<(&str, &str)> = actions
        .iter()
        .copied()
        .filter(|(key, _)| *key != "y")
        .collect();
    let candidates = [
        footer_line(&FIXED, actions),
        footer_line(&FIXED, &without_copy),
        footer_line(&FIXED, &[]),
        footer_line(&FIXED[..FIXED.len() - 1], &[]),
    ];
    let last = candidates.len() - 1;
    candidates
        .into_iter()
        .enumerate()
        .find(|(index, line)| line.width() <= width || *index == last)
        .map(|(_, line)| line)
        .unwrap_or_default()
}

/// `fixed`, a divider, `actions`, and the closing keys.
fn footer_line(fixed: &[(&str, &str)], actions: &[(&str, &str)]) -> Line<'static> {
    let mut spans = vec![Span::raw(" ")];
    spans.extend(key_spans(fixed));
    if !actions.is_empty() {
        spans.push(Span::styled("│ ", DIM));
        spans.extend(key_spans(actions));
    }
    spans.extend(key_spans(&ALWAYS));
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [(&str, &str); 6] = [
        ("w", "warm"),
        ("l", "launch"),
        ("t", "try"),
        ("T", "chat"),
        ("x", "remove"),
        ("y", "copy path"),
    ];

    fn text(line: &Line) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn the_footer_sheds_from_the_right_until_it_fits() {
        let wide = text(&fitting_line(&ALL, 200));
        assert!(wide.contains("y copy path") && wide.ends_with("q quit  "));
        let medium = text(&fitting_line(&ALL, 110));
        assert!(!medium.contains("copy path") && medium.contains("x remove"));
        let narrow = text(&fitting_line(&ALL, 80));
        assert!(!narrow.contains('│') && narrow.contains("S serve") && narrow.contains("q quit"));
        let tiny = text(&fitting_line(&ALL, 50));
        assert!(!tiny.contains("S serve") && tiny.contains("q quit"));
    }
}
