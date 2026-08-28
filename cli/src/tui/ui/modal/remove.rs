//! The remove card: what removing the model does, in the store's own terms.

use kernel::removal::ModelDeletionPreview;
use ratatui::layout::Rect;
use ratatui::text::Line;

use super::label_column;
use crate::tui::facts::Facts;
use crate::tui::text;
use crate::tui::ui::{BORDER_ROWS, DIM, field_line, keys, styled_field, value_width};

/// The remove card's width: three label rows and one sentence, the path
/// elided to fit.
pub(super) const REMOVE_WIDTH: u16 = 72;
/// The remove card's height: a blank, three rows, a blank, what happens and
/// what is left, a blank, the keys, and the border.
pub(super) const REMOVE_HEIGHT: u16 = 9 + BORDER_ROWS;

/// What removing the model does, in the store's own terms.
pub(super) fn remove(
    preview: &ModelDeletionPreview,
    facts: &Facts,
    inner: Rect,
) -> Vec<Line<'static>> {
    let row = |label, value: String| field_line(label, value, label_column());
    let path_width = value_width(inner.width as usize, label_column());
    let what = if preview.via_daemon {
        "removes the tag through the Ollama daemon · ollama rm".to_owned()
    } else if preview.paths.is_empty() {
        "nothing is left on disk; this forgets the record".to_owned()
    } else {
        format!(
            "deletes {} permanently, not to the trash",
            text::count(preview.paths.len(), "path")
        )
    };
    let mut lines = vec![
        Line::default(),
        row("store", preview.kind.as_str().to_owned()),
        row("on disk", text::bytes(preview.bytes_estimate)),
    ];
    if let Some(path) = preview.paths.first() {
        let shown = text::at_home(path);
        lines.push(row("path", text::elide_middle(&shown, path_width)));
    }
    lines.push(Line::default());
    lines.push(Line::from(format!(" {what}")));
    lines.push(Line::from(styled_field(
        "after",
        format!(
            "{} on disk",
            text::bytes((facts.disk_bytes() - preview.bytes_estimate).max(0))
        ),
        label_column(),
        DIM,
    )));
    lines.push(Line::default());
    lines.push(keys(&[("y", "remove"), ("n", "keep")]));
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::tui::testing::{deletion_preview, text, texts};

    #[test]
    fn the_remove_path_is_elided_to_the_card() {
        let root = "/var/lib/ollama/models/blobs/";
        let path = format!("{root}{}", "a".repeat(120 - root.len()));
        assert_eq!(path.len(), 120);
        let preview = deletion_preview(vec![path]);
        let inner = Rect::new(0, 0, 80, 9);
        let lines = remove(&preview, &Facts::default(), inner);
        assert_eq!(lines.len() as u16, REMOVE_HEIGHT - BORDER_ROWS);
        let path_line = lines
            .iter()
            .find(|line| text(line).contains("path"))
            .map(text)
            .unwrap_or_default();
        assert!(path_line.contains('…'));
        assert!(path_line.starts_with(" path"));
        assert!(path_line.ends_with("aaaa"));
        assert!(Line::from(path_line.as_str()).width() <= 80);
        let after = lines
            .iter()
            .find(|line| text(line).contains("after"))
            .map(text)
            .unwrap_or_default();
        assert!(after.starts_with(" after") && after.ends_with("on disk"));
        assert!(!after.contains(':'));
    }

    #[test]
    fn a_daemon_removal_names_the_daemon_and_its_command() {
        let preview = ModelDeletionPreview {
            via_daemon: true,
            ..deletion_preview(vec!["/p".to_owned()])
        };
        let lines = remove(&preview, &Facts::default(), Rect::new(0, 0, 70, 9));
        assert!(
            texts(&lines)
                .iter()
                .any(|line| line == " removes the tag through the Ollama daemon · ollama rm")
        );
        let forgotten = remove(
            &deletion_preview(Vec::new()),
            &Facts::default(),
            Rect::new(0, 0, 70, 9),
        );
        assert!(
            texts(&forgotten)
                .iter()
                .any(|line| line == " nothing is left on disk; this forgets the record")
        );
    }
}
