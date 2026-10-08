//! The results: what matched, grouped by kind while the list is the
//! catalog's recommendations, each row with where it comes from, its size,
//! a ten-cell gauge of how much of the machine it would take, and how often
//! it has been pulled when the hub says.

use kernel::profiles::FitVerdict;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use super::Look;
use crate::tui::app::App;
use crate::tui::palette::{AMBER, INK_COLOR, LINE_STRONG, SEL, SOFT_COLOR, SURFACE, mix};
use crate::tui::pull::{Kind, ListingRow, Offer, OnShelf, PullModal, Search};
use crate::tui::tasks::TaskState;
use crate::tui::text;
use crate::tui::ui::card::{Card, scroll_mark};
use crate::tui::ui::{ACCENT, BOLD, DIM, EYEBROW, INK, SELECTED_MARK, SOFT, spinner};

/// The fit gauge's cells, and the room its word takes after it.
const GAUGE: usize = 10;
const GAUGE_WORD: usize = 11;
/// The columns' widths: where from, the size, the pulls count.
const FROM: usize = 7;
const SIZE: usize = 8;
const PULLS: usize = 6;
/// How long the selection's tint takes to come up.
const SELECTION_FADE_MS: u64 = 120;

/// Which columns the card has room for, the name taking the rest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Columns {
    from: bool,
    gauge_word: bool,
    pulls: bool,
}

impl Columns {
    /// The fullest set that leaves the name at least 20 cells of `width`:
    /// the pulls go first, then where from, then the gauge's word.
    fn fitting(width: usize) -> Self {
        let need = |columns: &Columns| {
            2 + 20
                + usize::from(columns.from) * (FROM + 2)
                + SIZE
                + 2
                + GAUGE
                + usize::from(columns.gauge_word) * GAUGE_WORD
                + usize::from(columns.pulls) * (PULLS + 2)
        };
        [
            Columns {
                from: true,
                gauge_word: true,
                pulls: true,
            },
            Columns {
                from: true,
                gauge_word: true,
                pulls: false,
            },
            Columns {
                from: false,
                gauge_word: true,
                pulls: false,
            },
            Columns {
                from: false,
                gauge_word: false,
                pulls: false,
            },
        ]
        .into_iter()
        .find(|columns| need(columns) <= width)
        .unwrap_or(Columns {
            from: false,
            gauge_word: false,
            pulls: false,
        })
    }

    /// The name's width at `width`.
    fn name(&self, width: usize) -> usize {
        width.saturating_sub(
            2 + usize::from(self.from) * (FROM + 2)
                + SIZE
                + 2
                + GAUGE
                + usize::from(self.gauge_word) * GAUGE_WORD
                + usize::from(self.pulls) * (PULLS + 2),
        )
    }
}

/// Draw the results card into `area`.
pub(super) fn draw(frame: &mut Frame, area: Rect, modal: &PullModal, app: &App, look: &Look) {
    let query = modal.input.trimmed();
    let right = if !query.is_empty() {
        format!("matching “{}”", text::clip(query, 20))
    } else if modal.kind() == Kind::All {
        format!(
            "recommended for {} GiB",
            text::gib(modal.memory_bytes() as i64).trim_end_matches(".0")
        )
    } else {
        format!("{} models", modal.kind().label())
    };
    Card::new(vec![Span::styled("results", BOLD)])
        .right(vec![Span::styled(right, DIM)])
        .render(area, frame.buffer_mut());
    let inner = Card::inner(area);
    let width = inner.width as usize;
    let columns = Columns::fitting(width);
    if modal.matches.is_empty() {
        let searching = matches!(modal.search(), Search::Due(_) | Search::Asked);
        let note = if searching {
            format!("{} searching hugging face", spinner(look.spin_frame))
        } else if query.is_empty() {
            "nothing to offer for this kind".to_owned()
        } else {
            format!(
                "nothing matches “{}” · try a name:tag or owner/repo",
                text::clip(query, 24)
            )
        };
        let note = text::clip(&note, width.saturating_sub(2));
        let x = inner.x + (inner.width.saturating_sub(note.width() as u16)) / 2;
        let y = inner.y + inner.height / 2;
        if inner.height > 0 {
            frame.buffer_mut().set_stringn(x, y, &note, width, DIM);
        }
        return;
    }
    let mut lines = vec![header(&columns, width)];
    let rows = modal.rows();
    let room = (inner.height as usize).saturating_sub(1);
    let selected_row = rows
        .iter()
        .position(|row| *row == ListingRow::Match(modal.selected))
        .unwrap_or(0);
    let first = selected_row.saturating_sub(room.saturating_sub(1));
    let lifted = look.motion.eased(modal.selected_at, SELECTION_FADE_MS);
    for row in rows.iter().skip(first).take(room) {
        lines.push(match row {
            ListingRow::Blank => Line::default(),
            ListingRow::Eyebrow(category) => Line::from(Span::styled(
                format!("    {}", Kind::Of(*category).label().to_uppercase()),
                EYEBROW,
            )),
            ListingRow::Match(index) => {
                let offer = &modal.matches[*index];
                let selected = *index == modal.selected;
                let mut line = offer_line(offer, selected, &columns, width, modal, app, look);
                if selected {
                    let used = line.width();
                    line.spans
                        .push(Span::raw(" ".repeat(width.saturating_sub(used))));
                    line = line.patch_style(Style::new().bg(mix(SURFACE, SEL, lifted)));
                }
                line
            }
        });
    }
    frame.render_widget(Paragraph::new(lines), inner);
    scroll_mark(frame.buffer_mut(), area, first, room, rows.len());
}

/// The column headings, faint.
fn header(columns: &Columns, width: usize) -> Line<'static> {
    let mut spans = vec![Span::styled(
        format!(
            "    {}",
            text_padded("NAME", columns.name(width).saturating_sub(2))
        ),
        EYEBROW,
    )];
    if columns.from {
        spans.push(Span::styled(format!("{:<FROM$}  ", "FROM"), EYEBROW));
    }
    spans.push(Span::styled(format!("{:>SIZE$}  ", "SIZE"), EYEBROW));
    spans.push(Span::styled(
        format!(
            "{:<width$}",
            "FIT",
            width = GAUGE + usize::from(columns.gauge_word) * GAUGE_WORD
        ),
        EYEBROW,
    ));
    if columns.pulls {
        spans.push(Span::styled(format!("  {:>PULLS$}", "PULLS"), EYEBROW));
    }
    Line::from(spans)
}

fn text_padded(text: &str, width: usize) -> String {
    crate::support::text::padded(&text::clip(text, width), width)
}

/// One offer's row: the mark, the name, where from, the size, the gauge,
/// the pulls.
fn offer_line(
    offer: &Offer,
    selected: bool,
    columns: &Columns,
    width: usize,
    modal: &PullModal,
    app: &App,
    look: &Look,
) -> Line<'static> {
    let memory = modal.memory_bytes();
    let verdict = modal.fit(offer);
    let bytes = modal.size(offer);
    let present = offer.shelf == Some(OnShelf::Present);
    let quiet = present || verdict == Some(FitVerdict::TooLarge);
    let task = app.tasks.pull_for(&offer.reference);
    let downloading = task.is_some_and(|row| {
        matches!(
            row.state,
            TaskState::Downloading(_) | TaskState::Running | TaskState::Status(_)
        )
    });
    let mark = if downloading {
        Span::styled(spinner(look.spin_frame).to_owned(), ACCENT)
    } else {
        match offer.shelf {
            Some(OnShelf::Present) => Span::styled("●", SOFT),
            Some(OnShelf::Gone) => Span::styled("✕", DIM),
            None => Span::styled("○", DIM),
        }
    };
    let name_width = columns.name(width).saturating_sub(2);
    // Two cells of air keep a long name off the column after it.
    let name = text::clip(&offer.reference, name_width.saturating_sub(2));
    let name_style = if quiet {
        SOFT
    } else if selected {
        BOLD
    } else {
        INK
    };
    let mut spans = vec![
        Span::styled(if selected { SELECTED_MARK } else { " " }, ACCENT),
        Span::raw(" "),
        mark,
        Span::raw(" "),
        Span::styled(crate::support::text::padded(&name, name_width), name_style),
    ];
    if columns.from {
        let from = if offer.provider.as_str() == "huggingface" {
            "hf"
        } else {
            offer.provider.as_str()
        };
        spans.push(Span::styled(format!("{:<FROM$}  ", from), SOFT));
    }
    let size = bytes.map_or_else(String::new, text::bytes);
    spans.push(Span::styled(
        format!("{:>SIZE$}  ", size),
        if quiet { SOFT } else { INK },
    ));
    spans.extend(gauge(
        offer,
        bytes,
        verdict,
        task,
        columns.gauge_word,
        memory,
    ));
    if columns.pulls {
        let pulls = offer.downloads.map_or_else(String::new, text::compact);
        spans.push(Span::styled(format!("  {:>PULLS$}", pulls), DIM));
    }
    Line::from(spans)
}

/// The fit gauge: ten cells of the machine's memory, this model's share lit
/// in its verdict's colour, and the verdict beside it; or a download's
/// progress, or that it is on the shelf.
fn gauge(
    offer: &Offer,
    bytes: Option<i64>,
    verdict: Option<FitVerdict>,
    task: Option<&crate::tui::strip::TaskRow>,
    word: bool,
    memory: u64,
) -> Vec<Span<'static>> {
    let pad = |spans: Vec<Span<'static>>| {
        let used: usize = spans.iter().map(Span::width).sum();
        let room = GAUGE + usize::from(word) * GAUGE_WORD;
        let mut spans = spans;
        spans.push(Span::raw(" ".repeat(room.saturating_sub(used))));
        spans
    };
    if let Some(row) = task
        && let TaskState::Downloading(progress) = &row.state
        && let Some(fraction) = progress.fraction()
    {
        let lit = (fraction * GAUGE as f64).round() as usize;
        let mut spans = vec![
            Span::styled("━".repeat(lit), Style::new().fg(INK_COLOR)),
            Span::styled("━".repeat(GAUGE - lit), Style::new().fg(LINE_STRONG)),
        ];
        if word {
            spans.push(Span::styled(
                format!(" {}%", (fraction * 100.0) as u64),
                BOLD,
            ));
        }
        return pad(spans);
    }
    if offer.shelf == Some(OnShelf::Present) {
        return pad(vec![Span::styled("✓ on shelf", SOFT)]);
    }
    let Some(bytes) = bytes.filter(|_| memory > 0) else {
        return pad(vec![
            Span::styled("━".repeat(GAUGE), Style::new().fg(LINE_STRONG)),
            Span::styled(if word { " …" } else { "" }, DIM),
        ]);
    };
    let share = ((bytes.max(0) as f64 / memory as f64) * GAUGE as f64).ceil() as usize;
    let lit = share.clamp(1, GAUGE);
    let (glyph, colour, label) = match verdict {
        Some(FitVerdict::TooLarge) => ("╍", mix(LINE_STRONG, SOFT_COLOR, 0.4), "too big"),
        Some(FitVerdict::TightFit) => ("━", AMBER, "tight"),
        _ => ("━", INK_COLOR, "fits"),
    };
    let label = if offer.shelf == Some(OnShelf::Gone) {
        "pull again"
    } else {
        label
    };
    let mut spans = vec![
        Span::styled(glyph.repeat(lit), Style::new().fg(colour)),
        Span::styled("━".repeat(GAUGE - lit), Style::new().fg(LINE_STRONG)),
    ];
    if word {
        let style = match verdict {
            Some(FitVerdict::TightFit) => Style::new().fg(AMBER),
            Some(FitVerdict::TooLarge) => DIM,
            _ => SOFT,
        };
        spans.push(Span::styled(format!(" {label}"), style));
    }
    pad(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_drop_the_pulls_first_then_where_from() {
        assert!(Columns::fitting(90).pulls);
        assert!(!Columns::fitting(69).pulls && Columns::fitting(69).from);
        assert!(!Columns::fitting(56).from && Columns::fitting(56).gauge_word);
        assert_eq!(
            Columns::fitting(10),
            Columns {
                from: false,
                gauge_word: false,
                pulls: false
            }
        );
    }
}
