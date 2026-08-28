//! The header: one line of numbers when space is short, the koala beside the
//! wordmark when there is room.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use kernel::profiles::FitVerdict;
use kernel::records::{ModelRecord, ModelState};

use super::machine::gateway_state;
use super::{BOLD, DIM, wordmark};
use crate::support::banner::{KOALA, KOALA_WIDTH};
use crate::support::shelf_table::verdict;
use crate::tui::app::App;
use crate::tui::layout::TALL_HEADER_ROWS;
use crate::tui::text;
use unicode_width::UnicodeWidthStr;

/// Draw the header into `area`, tall or one-line by its height. The one-line
/// header carries the machine's memory and gateway only when
/// `machine_shown` is false, since the block says both.
pub(super) fn draw(frame: &mut Frame, area: Rect, app: &App, machine_shown: bool) {
    if area.height >= TALL_HEADER_ROWS {
        draw_tall(frame, area, app);
    } else {
        frame.render_widget(
            Paragraph::new(summary_line(app, machine_shown, area.width as usize)),
            area,
        );
    }
}

/// ` hedos v1.3.0  12 models · 3 warm · 1 too big`, plus the free memory
/// and the gateway state when no machine block shows them, held to `width`
/// cells: the counts of what won't run go first, then the line is clipped,
/// so the gateway state survives a narrow terminal.
fn summary_line(app: &App, machine_shown: bool, width: usize) -> Line<'static> {
    let mark = wordmark();
    let room = width.saturating_sub(mark.iter().map(Span::width).sum::<usize>() + 2);
    let machine = if machine_shown {
        String::new()
    } else {
        format!(
            " · {} GiB free · gateway {}",
            text::gib(app.facts.free_bytes()),
            gateway_state(&app.facts),
        )
    };
    let mut summary = format!("{}{machine}", shelf_line(app, true));
    if summary.width() > room {
        summary = format!("{}{machine}", shelf_line(app, false));
    }
    let mut spans = mark.to_vec();
    spans.push(Span::raw("  "));
    spans.push(Span::styled(text::clip(&summary, room), DIM));
    Line::from(spans)
}

/// The koala beside the wordmark, what hedos is for, and what the machine
/// block does not already say: the shelf in numbers. The panel sits centered
/// on the koala's rows.
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

    let panel_lines = [
        Line::from(wordmark().to_vec()),
        Line::from(Span::styled(" run and serve local models headlessly", DIM)),
        Line::default(),
        Line::from(Span::styled(format!(" {}", shelf_line(app, true)), DIM)),
    ];
    let above = 1 + (KOALA.len() - panel_lines.len()) / 2;
    let mut lines: Vec<Line> = std::iter::repeat_n(Line::default(), above)
        .chain(panel_lines)
        .collect();
    lines.truncate(area.height as usize);
    frame.render_widget(Paragraph::new(lines), panel);
}

/// `12 models · 3 warm · 1 too big · 2 gone`, the last two only when they
/// count something and `wont_run` asks for them. A record whose weights are
/// gone counts once, as gone: the shelf row shows it no verdict either.
fn shelf_line(app: &App, wont_run: bool) -> String {
    let mut parts = vec![text::count(app.records.len(), "model")];
    parts.push(format!("{} warm", warm_count(app)));
    if !wont_run {
        return parts.join(" · ");
    }
    let gone = |record: &&ModelRecord| record.state == ModelState::Missing;
    let too_big = app
        .records
        .iter()
        .filter(|record| {
            !gone(record)
                && verdict(record.footprint_mb, app.facts.memory_bytes)
                    == Some(FitVerdict::TooLarge)
        })
        .count();
    if too_big > 0 {
        parts.push(format!("{too_big} too big"));
    }
    let gone = app.records.iter().filter(gone).count();
    if gone > 0 {
        parts.push(format!("{gone} gone"));
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

#[cfg(test)]
mod tests {
    use super::*;

    use crate::tui::facts::Facts;
    use crate::tui::testing::{facts_with_memory, record, text};

    fn facts() -> Facts {
        facts_with_memory(64)
    }

    #[test]
    fn the_short_header_carries_the_machine_only_when_the_block_is_gone() {
        let app = App::new(vec![record("a"), record("b")], facts());
        let with_block = text(&summary_line(&app, true, 200));
        assert!(
            with_block.ends_with("  2 models · 0 warm"),
            "{with_block:?}"
        );
        assert!(!with_block.contains("GiB") && !with_block.contains("gateway"));
        let alone = text(&summary_line(&app, false, 200));
        assert!(
            alone.ends_with("  2 models · 0 warm · 64 GiB free · gateway off"),
            "{alone:?}"
        );
    }

    #[test]
    fn a_gone_record_counts_once_as_gone() {
        let mut gone = record("m");
        gone.footprint_mb = Some(200 * 1024);
        gone.state = ModelState::Missing;
        let mut too_big = record("n");
        too_big.footprint_mb = Some(200 * 1024);
        let app = App::new(vec![gone, too_big, record("o")], facts());
        assert_eq!(
            shelf_line(&app, true),
            "3 models · 0 warm · 1 too big · 1 gone"
        );
        assert_eq!(shelf_line(&app, false), "3 models · 0 warm");
        let summary = text(&summary_line(&app, true, 200));
        assert!(
            summary.ends_with("3 models · 0 warm · 1 too big · 1 gone"),
            "{summary:?}"
        );
    }

    #[test]
    fn the_short_header_keeps_the_gateway_at_eighty_columns() {
        let mut gone = record("m");
        gone.state = ModelState::Missing;
        let mut too_big = record("n");
        too_big.footprint_mb = Some(200 * 1024);
        let mut records = vec![gone, too_big];
        records.extend((0..10).map(|index| record(&format!("model-{index}"))));
        let facts = Facts {
            gateway_port: Some(11434),
            ..facts()
        };
        let app = App::new(records, facts);
        let wide = text(&summary_line(&app, false, 200));
        assert!(
            wide.ends_with("12 models · 0 warm · 1 too big · 1 gone · 64 GiB free · gateway on :11434 · 0 req/min"),
            "{wide:?}"
        );
        assert!(wide.width() > 80, "{wide:?}");
        let narrow = summary_line(&app, false, 80);
        assert!(narrow.width() <= 80, "{:?}", text(&narrow));
        let narrow = text(&narrow);
        assert!(
            narrow.ends_with("12 models · 0 warm · 64 GiB free · gateway on :11434 · 0 req/min"),
            "{narrow:?}"
        );
        let tiny = summary_line(&app, false, 40);
        assert!(tiny.width() <= 40);
        assert!(text(&tiny).ends_with('…'));
    }
}
