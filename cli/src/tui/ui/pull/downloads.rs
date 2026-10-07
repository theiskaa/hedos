//! The downloads in flight under the results, so several can be started and
//! the screen still says how each is going.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::Look;
use crate::tui::app::App;
use crate::tui::palette::{LINE_STRONG, SOFT_COLOR, WHITE, mix};
use crate::tui::tasks::TaskState;
use crate::tui::text;
use crate::tui::ui::card::Card;
use crate::tui::ui::{BOLD, CAUTION, DIM, FAILED, INK, SOFT};

/// The bar's width, and how far its shimmer runs.
const BAR: usize = 22;
const SHIMMER_MS: f32 = 1600.0;
/// The name's width.
const NAME: usize = 30;

/// Draw the card into `area`.
pub(super) fn draw(frame: &mut Frame, area: Rect, app: &App, look: &Look) {
    let rows: Vec<_> = app.tasks.pull_rows().collect();
    let live = rows
        .iter()
        .filter(|row| {
            matches!(
                row.state,
                TaskState::Downloading(_) | TaskState::Running | TaskState::Status(_)
            )
        })
        .count();
    Card::new(vec![
        Span::styled("downloads", BOLD),
        Span::styled(format!(" · {live}"), DIM),
    ])
    .render(area, frame.buffer_mut());
    let inner = Card::text_inner(area);
    let width = inner.width as usize;
    if rows.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled(
                text::clip("nothing downloading · a pull runs on after you quit", width),
                DIM,
            )),
            inner,
        );
        return;
    }
    let shown = rows.len().saturating_sub(inner.height as usize);
    let lines: Vec<Line> = rows[shown..]
        .iter()
        .map(|row| {
            let mut spans = vec![Span::styled(
                format!(
                    "{}  ",
                    crate::support::text::padded(&text::clip(&row.label.subject, NAME), NAME)
                ),
                if live > 0 && matches!(row.state, TaskState::Downloading(_)) {
                    INK
                } else {
                    SOFT
                },
            )];
            match &row.state {
                TaskState::Downloading(progress) => {
                    let fraction = progress.fraction().unwrap_or(0.0);
                    let full = (fraction * BAR as f64) as usize;
                    let band = look.motion.age(0).map(|age| {
                        (age * 1000.0 % SHIMMER_MS) / SHIMMER_MS * (full as f32 + 10.0) - 5.0
                    });
                    for cell in 0..BAR {
                        let colour = if cell < full {
                            let lit = band.map_or(0.0, |band| {
                                (1.0 - (cell as f32 - band).abs() / 3.0).clamp(0.0, 1.0)
                            });
                            mix(SOFT_COLOR, WHITE, 0.9 * lit)
                        } else {
                            LINE_STRONG
                        };
                        spans.push(Span::styled("━", Style::new().fg(colour)));
                    }
                    spans.push(Span::styled(
                        format!("  {:>4}", format!("{}%", (fraction * 100.0) as u64)),
                        BOLD,
                    ));
                    if let Some(total) = progress.total_bytes {
                        spans.push(Span::styled(
                            format!(
                                "  {} of {}",
                                text::bytes(progress.bytes_downloaded),
                                text::bytes(total)
                            ),
                            SOFT,
                        ));
                    }
                }
                TaskState::Running => spans.push(Span::styled("starting", DIM)),
                TaskState::Status(status) | TaskState::Done(status) => {
                    spans.push(Span::styled(status.clone(), DIM));
                }
                TaskState::Stopped(status) => spans.push(Span::styled(status.clone(), CAUTION)),
                TaskState::Failed(status) => spans.push(Span::styled(status.clone(), FAILED)),
            }
            Line::from(spans)
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}
