//! The header: one line of numbers when space is short, the koala beside the
//! wordmark when there is room.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::machine::{free_of_total, gateway_state};
use super::{BOLD, DIM, field};
use crate::support::banner::{KOALA, KOALA_WIDTH};
use crate::tui::app::App;
use crate::tui::layout::TALL_HEADER_ROWS;
use crate::tui::text;

/// Width of the labels in the koala panel.
const LABEL_WIDTH: usize = 8;

/// Draw the header into `area`, tall or one-line by its height.
pub(super) fn draw(frame: &mut Frame, area: Rect, app: &App) {
    if area.height >= TALL_HEADER_ROWS {
        draw_tall(frame, area, app);
    } else {
        frame.render_widget(Paragraph::new(summary_line(app)), area);
    }
}

fn summary_line(app: &App) -> Line<'static> {
    Line::from(vec![
        Span::styled(" hedos", BOLD),
        Span::styled(format!(" v{}", env!("CARGO_PKG_VERSION")), DIM),
        Span::raw("  "),
        Span::styled(
            format!(
                "{} · {} warm · {} GiB free · gateway {}",
                text::count(app.records.len(), "model"),
                app.warm_count(),
                text::gib(app.facts.free_bytes()),
                gateway_state(&app.facts),
            ),
            DIM,
        ),
    ])
}

/// The koala beside the wordmark and what the machine block does not already
/// say: the shelf in numbers.
fn draw_tall(frame: &mut Frame, area: Rect, app: &App) {
    let [koala, panel] =
        Layout::horizontal([Constraint::Length(KOALA_WIDTH + 5), Constraint::Min(0)]).areas(area);
    // A blank row above the koala keeps it off the terminal's top edge.
    let koala_lines: Vec<Line> = std::iter::once(Line::default())
        .chain(
            KOALA
                .iter()
                .map(|row| Line::from(Span::styled(format!("  {row}"), BOLD))),
        )
        .collect();
    frame.render_widget(Paragraph::new(koala_lines), koala);

    let row = |label, value: String| Line::from(field(label, value, LABEL_WIDTH));
    let mut lines = vec![
        Line::default(),
        Line::default(),
        Line::from(vec![
            Span::styled(" hedos", BOLD),
            Span::styled(format!(" v{}", env!("CARGO_PKG_VERSION")), DIM),
        ]),
        Line::from(Span::styled(" ἕδος · a seat, an abode, a foundation", DIM)),
        Line::default(),
        row("shelf", shelf_line(app)),
        Line::from(
            field("memory", "", LABEL_WIDTH)
                .into_iter()
                .chain(free_of_total(&app.facts))
                .collect::<Vec<_>>(),
        ),
        row("gateway", gateway_state(&app.facts)),
    ];
    lines.truncate(area.height as usize);
    frame.render_widget(Paragraph::new(lines), panel);
}

fn shelf_line(app: &App) -> String {
    let mut parts = vec![text::count(app.records.len(), "model")];
    parts.push(format!("{} warm", app.warm_count()));
    let too_big = app.too_big_count();
    if too_big > 0 {
        parts.push(format!("{too_big} too big for this machine"));
    }
    parts.join(" · ")
}
