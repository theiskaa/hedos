//! The header: one line of numbers when space is short, the koala beside the
//! wordmark when there is room.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use kernel::profiles::FitVerdict;

use super::machine::{free_of_total, gateway_state};
use super::{BOLD, DIM, field_line, label, wordmark};
use crate::support::banner::{KOALA, KOALA_WIDTH};
use crate::support::shelf_table::verdict;
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
    let mut spans = wordmark().to_vec();
    spans.push(Span::raw("  "));
    spans.push(Span::styled(
        format!(
            "{} · {} warm · {} GiB free · gateway {}",
            text::count(app.records.len(), "model"),
            warm_count(app),
            text::gib(app.facts.free_bytes()),
            gateway_state(&app.facts),
        ),
        DIM,
    ));
    Line::from(spans)
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

    let row = |label, value: String| field_line(label, value, LABEL_WIDTH);
    let mut lines = vec![
        Line::default(),
        Line::default(),
        Line::from(wordmark().to_vec()),
        Line::default(),
        row("shelf", shelf_line(app)),
        Line::from(
            vec![label("memory", LABEL_WIDTH)]
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
    parts.push(format!("{} warm", warm_count(app)));
    let too_big = app
        .records
        .iter()
        .filter(|record| {
            verdict(record.footprint_mb, app.facts.memory_bytes) == Some(FitVerdict::TooLarge)
        })
        .count();
    if too_big > 0 {
        parts.push(format!("{too_big} too big for this machine"));
    }
    parts.join(" · ")
}

/// How many models on the shelf are held in memory.
fn warm_count(app: &App) -> usize {
    app.records
        .iter()
        .filter(|record| app.facts.is_warm(&record.id))
        .count()
}
