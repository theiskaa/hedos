//! The stop card: what each way of stopping a pull keeps, asked before a key
//! does anything to it.

use kernel::install::event::InstallProgress;
use ratatui::layout::Rect;
use ratatui::text::Line;

use super::label_column;
use crate::tui::stop::StopCard;
use crate::tui::text;
use crate::tui::ui::{BORDER_ROWS, field_line, keys, value_width};

/// The stop card's width: two label rows and two sentences, the reference
/// elided to fit.
pub(super) const STOP_WIDTH: u16 = 60;
/// The stop card's height: a blank, two rows, a blank, what each way out
/// keeps, a blank, the keys, and the border.
pub(super) const STOP_HEIGHT: u16 = 8 + BORDER_ROWS;

/// The pull, what it has landed, and what each way of stopping does to that.
pub(super) fn stop(card: &StopCard, inner: Rect) -> Vec<Line<'static>> {
    let row = |label, value: String| field_line(label, value, label_column());
    let reference_width = value_width(inner.width as usize, label_column());
    vec![
        Line::default(),
        row(
            "model",
            text::elide_middle(&card.reference, reference_width),
        ),
        row("on disk", landed(&card.progress)),
        Line::default(),
        Line::from(" pause keeps what has landed, to resume later"),
        Line::from(" cancel throws it away, not to the trash"),
        Line::default(),
        keys(&[("p", "pause"), ("x", "cancel"), ("esc", "keep going")]),
    ]
}

/// What has landed against the total, as far as either is known.
fn landed(progress: &InstallProgress) -> String {
    let so_far = progress.bytes_downloaded;
    if so_far <= 0 {
        return "nothing yet".to_owned();
    }
    match progress.total_bytes {
        Some(total) if !progress.total_is_partial => {
            format!("{} of {}", text::bytes(so_far), text::bytes(total))
        }
        _ => format!("{} so far", text::bytes(so_far)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::tui::testing::{text, texts};

    fn card(progress: InstallProgress) -> StopCard {
        StopCard {
            job: "1000-x".to_owned(),
            reference: "Qwen/Qwen2.5-1.5B-Instruct".to_owned(),
            progress,
        }
    }

    #[test]
    fn the_card_names_the_model_and_what_landed() {
        let lines = stop(
            &card(InstallProgress {
                bytes_downloaded: 3 << 30,
                total_bytes: Some(4 << 30),
                ..InstallProgress::default()
            }),
            Rect::new(0, 0, 58, 8),
        );
        assert_eq!(lines.len() as u16, STOP_HEIGHT - BORDER_ROWS);
        let shown = texts(&lines);
        assert!(shown[1].starts_with(" model") && shown[1].ends_with("Qwen/Qwen2.5-1.5B-Instruct"));
        assert!(shown[2].starts_with(" on disk") && shown[2].ends_with("3 GB of 4 GB"));
        assert!(shown.iter().any(|line| line.starts_with(" pause keeps")));
        assert!(shown.iter().any(|line| line.starts_with(" cancel throws")));
        assert_eq!(
            shown.last().map(|line| line.trim_end()),
            Some(" p pause  x cancel  esc keep going")
        );
        assert!(lines.iter().all(|line| line.width() <= 58));
    }

    #[test]
    fn the_figures_say_as_much_as_is_known() {
        assert_eq!(landed(&InstallProgress::default()), "nothing yet");
        assert_eq!(
            landed(&InstallProgress {
                bytes_downloaded: 1 << 20,
                ..InstallProgress::default()
            }),
            "1 MB so far"
        );
        assert_eq!(
            landed(&InstallProgress {
                bytes_downloaded: 1 << 20,
                total_bytes: Some(4 << 20),
                total_is_partial: true,
                ..InstallProgress::default()
            }),
            "1 MB so far"
        );
    }

    #[test]
    fn a_long_reference_is_elided_to_the_card() {
        let long = format!("org/{}", "a".repeat(80));
        let lines = stop(&card(InstallProgress::default()), Rect::new(0, 0, 40, 8));
        assert!(!text(&lines[1]).contains('…'));
        let mut shown = card(InstallProgress::default());
        shown.reference = long;
        let lines = stop(&shown, Rect::new(0, 0, 40, 8));
        let model = text(&lines[1]);
        assert!(model.contains('…') && model.starts_with(" model"));
        assert!(Line::from(model.as_str()).width() <= 40);
    }
}
