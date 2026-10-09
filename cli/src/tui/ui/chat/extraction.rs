//! An extractor's answer as lines: what was found, one row per entity with its
//! kind, its text, a short bar for how sure the extractor is and the figure,
//! and its canonical form under it when that differs. Contacts are grouped
//! under who they are, then what belongs to none; an address is laid out as its
//! parts.

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::support::extract::{self, Component, Entity, Operation};
use crate::tui::palette::{BAR_EMPTY, BAR_FILLED, BRIGHT, FAINT_COLOR, INK_COLOR, mix};
use crate::tui::text;
use crate::tui::ui::{BOLD, CAUTION, DIM, INK, SELECTED_MARK, SOFT, TRACK};
use crate::tui::wrap;

/// The kind column: `address` and a cell of air; an address's parts take
/// what their longest label needs, up to the most.
const KIND_WIDTH: usize = 9;
const PART_WIDTH_MAX: usize = 15;
/// The confidence bar, and the figure after it.
const BAR_WIDTH: usize = 10;
const FIGURE_WIDTH: usize = 6;
/// A confidence under this reads as uncertain.
const UNSURE_BELOW: f64 = 0.5;

/// The answer in `reply` to `operation` at `width` cells. A reply that is no
/// extraction says so, with why.
pub(super) fn answer(reply: &str, operation: Operation, width: usize) -> Vec<Line<'static>> {
    let read = match extract::parse(reply) {
        Ok(read) => read,
        Err(reason) => {
            return wrap::wrap(&reason, width)
                .into_iter()
                .map(|piece| Line::from(Span::styled(piece, CAUTION)))
                .collect();
        }
    };
    let mut lines = Vec::new();
    match operation {
        Operation::Detect => {
            for entity in &read.entities {
                lines.extend(entity_rows(entity, width));
            }
        }
        Operation::Contacts => {
            for (index, contact) in read.contacts.iter().enumerate() {
                if index > 0 {
                    lines.push(Line::default());
                }
                let head = contact.head();
                lines.push(row(
                    vec![Span::styled(
                        format!("{SELECTED_MARK} "),
                        Style::new().fg(BRIGHT),
                    )],
                    (
                        head.map_or("contact", |head| head.kind.as_str()),
                        KIND_WIDTH,
                    ),
                    &head.map_or_else(
                        || "with no one named".to_owned(),
                        |head| one_line(&head.text),
                    ),
                    BOLD,
                    contact.confidence,
                    contact.review_recommended,
                    width,
                ));
                for member in contact.members() {
                    lines.extend(entity_rows(member, width));
                }
            }
            if !read.unassigned.is_empty() {
                if !lines.is_empty() {
                    lines.push(Line::default());
                }
                lines.push(Line::from(Span::styled("  in no contact", DIM)));
                for entity in &read.unassigned {
                    lines.extend(entity_rows(entity, width));
                }
            }
        }
        Operation::Address => {
            if let Some(address) = &read.address {
                for piece in wrap::wrap(&one_line(&address.text), width.saturating_sub(2)) {
                    lines.push(Line::from(vec![Span::raw("  "), Span::styled(piece, INK)]));
                }
                let label_width = address
                    .components
                    .iter()
                    .map(|part| part.label.width() + 1)
                    .max()
                    .unwrap_or(KIND_WIDTH)
                    .clamp(KIND_WIDTH, PART_WIDTH_MAX);
                lines.extend(
                    address
                        .components
                        .iter()
                        .map(|part| component_row(part, label_width, width)),
                );
                if address.components.is_empty() {
                    lines.push(Line::from(Span::styled("  not split into parts", DIM)));
                }
            }
        }
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::styled("  nothing found", DIM)));
    }
    lines
}

/// What the answer comes to, for the session card: how many things were
/// found and what they are.
pub(super) fn headline(reply: &str, operation: Operation) -> Option<(usize, String)> {
    let read = extract::parse(reply).ok()?;
    Some(match operation {
        Operation::Detect => (read.entities.len(), kinds(&read.entities)),
        Operation::Contacts => (
            read.contacts.len(),
            format!(
                "{} · {} {}",
                text::count(read.contacts.len(), "contact"),
                read.entity_count(),
                if read.entity_count() == 1 {
                    "entity"
                } else {
                    "entities"
                }
            ),
        ),
        Operation::Address => {
            let parts = read
                .address
                .as_ref()
                .map_or(0, |address| address.components.len());
            (parts, text::count(parts, "part"))
        }
    })
}

/// `2 person · 1 email`: how many of each kind, in the order first found.
fn kinds(entities: &[Entity]) -> String {
    let mut counted: Vec<(&str, usize)> = Vec::new();
    for entity in entities {
        match counted.iter_mut().find(|(kind, _)| *kind == entity.kind) {
            Some((_, count)) => *count += 1,
            None => counted.push((&entity.kind, 1)),
        }
    }
    if counted.is_empty() {
        return "nothing found".to_owned();
    }
    counted
        .iter()
        .map(|(kind, count)| format!("{count} {kind}"))
        .collect::<Vec<_>>()
        .join(" · ")
}

/// An entity's row, and its canonical form under the text when that differs.
fn entity_rows(entity: &Entity, width: usize) -> Vec<Line<'static>> {
    let mut lines = vec![row(
        vec![Span::raw("  ")],
        (&entity.kind, KIND_WIDTH),
        &one_line(&entity.text),
        INK,
        entity.confidence,
        entity.review_recommended,
        width,
    )];
    if let Some(normalized) = entity
        .normalized
        .as_deref()
        .filter(|normalized| *normalized != entity.text)
    {
        let room = width.saturating_sub(2 + KIND_WIDTH);
        lines.push(Line::from(vec![
            Span::raw(" ".repeat(2 + KIND_WIDTH)),
            Span::styled(text::clip(normalized, room), SOFT),
        ]));
    }
    lines
}

/// An address part's row, its label in a column `label_width` cells wide.
fn component_row(part: &Component, label_width: usize, width: usize) -> Line<'static> {
    row(
        vec![Span::raw("  ")],
        (&part.label.replace('_', " "), label_width),
        &one_line(&part.text),
        INK,
        part.confidence,
        false,
        width,
    )
}

/// One row: `lead`, the kind in a column as wide as `kind` says, the text
/// clipped to the room, then a bar for `confidence` and its figure, marked when
/// it is worth a look.
fn row(
    lead: Vec<Span<'static>>,
    (kind, kind_width): (&str, usize),
    found: &str,
    style: Style,
    confidence: f64,
    review: bool,
    width: usize,
) -> Line<'static> {
    let lead_width: usize = lead.iter().map(Span::width).sum();
    let room = width.saturating_sub(lead_width + kind_width + 2 + BAR_WIDTH + FIGURE_WIDTH);
    let found = text::clip(found, room.max(1));
    let pad = room.saturating_sub(found.width());
    let share = confidence.clamp(0.0, 1.0);
    let filled = ((share * BAR_WIDTH as f64).round() as usize).min(BAR_WIDTH);
    let unsure = review || share < UNSURE_BELOW;
    let mut spans = lead;
    spans.extend([
        Span::styled(
            format!("{:<kind_width$}", text::clip(kind, kind_width - 1)),
            DIM,
        ),
        Span::styled(found, style),
        Span::raw(" ".repeat(pad + 2)),
        Span::styled(
            BAR_FILLED.repeat(filled),
            Style::new().fg(mix(FAINT_COLOR, INK_COLOR, 0.45)),
        ),
        Span::styled(BAR_EMPTY.repeat(BAR_WIDTH - filled), TRACK),
        Span::styled(
            format!("{:>FIGURE_WIDTH$}", extract::confidence(share)),
            if unsure { CAUTION } else { SOFT },
        ),
    ]);
    Line::from(spans)
}

/// `text` on one line: its breaks read as commas, as an address written over
/// several lines reads on one.
fn one_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::tui::testing::texts;

    const DETECT: &str = r#"{"model":"tessera","operation":"detect","entities":[{"kind":"person","text":"Jordan Lee","start":0,"end":10,"confidence":0.93,"review_recommended":false,"source":"model"},{"kind":"phone","text":"(701) 555-0142","start":12,"end":26,"confidence":0.42,"review_recommended":true,"source":"rules","normalized":"+17015550142"}]}"#;
    const CONTACTS: &str = r#"{"model":"tessera","operation":"contacts","contacts":[{"start":0,"end":40,"confidence":0.71,"review_recommended":false,"person":{"kind":"person","text":"Jordan Lee","start":0,"end":10,"confidence":0.93,"review_recommended":false,"source":"model"},"addresses":[],"emails":[{"kind":"email","text":"jordan@acme.example","start":12,"end":31,"confidence":0.99,"review_recommended":false,"source":"rules","normalized":"jordan@acme.example"}],"phones":[]}],"unassigned":[{"kind":"org","text":"Acme","start":33,"end":37,"confidence":0.6,"review_recommended":false,"source":"model"}]}"#;
    const ADDRESS: &str = r#"{"model":"tessera","operation":"address","address":{"kind":"address","text":"123 Main St\nBismarck, ND 58501","start":0,"end":31,"confidence":0.88,"review_recommended":false,"source":"model","components":[{"label":"house_number","text":"123","start":0,"end":3,"confidence":0.99},{"label":"road","text":"Main St","start":4,"end":11,"confidence":0.97}]}}"#;

    #[test]
    fn detected_entities_read_as_rows_with_their_canonical_form() {
        let lines = texts(&answer(DETECT, Operation::Detect, 72));
        assert!(
            lines[0].contains("person")
                && lines[0].contains("Jordan Lee")
                && lines[0].ends_with("0.93")
        );
        assert!(lines[1].contains("(701) 555-0142") && lines[1].ends_with("0.42"));
        assert_eq!(
            lines[2].trim(),
            "+17015550142",
            "the canonical form under the text"
        );
        assert_eq!(
            headline(DETECT, Operation::Detect),
            Some((2, "1 person · 1 phone".to_owned()))
        );
    }

    #[test]
    fn contacts_are_grouped_under_who_they_are_then_what_belongs_to_none() {
        let lines = texts(&answer(CONTACTS, Operation::Contacts, 72));
        assert!(lines[0].starts_with(SELECTED_MARK) && lines[0].contains("Jordan Lee"));
        assert!(lines[1].starts_with("  email") && lines[1].contains("jordan@acme.example"));
        assert!(lines.iter().any(|line| line.trim() == "in no contact"));
        assert!(lines.last().is_some_and(|line| line.contains("Acme")));
        assert_eq!(
            headline(CONTACTS, Operation::Contacts),
            Some((1, "1 contact · 3 entities".to_owned()))
        );
    }

    #[test]
    fn an_address_reads_on_one_line_then_as_its_parts() {
        let lines = texts(&answer(ADDRESS, Operation::Address, 72));
        assert_eq!(lines[0].trim(), "123 Main St, Bismarck, ND 58501");
        assert!(lines[1].contains("house number") && lines[1].contains("123"));
        assert!(lines[2].contains("road") && lines[2].contains("Main St"));
    }

    #[test]
    fn a_reply_that_is_no_extraction_says_so() {
        let lines = texts(&answer("not json", Operation::Detect, 72));
        assert!(lines[0].starts_with("the reply was not an extraction"));
        let empty = r#"{"model":"tessera","operation":"detect","entities":[]}"#;
        assert_eq!(
            texts(&answer(empty, Operation::Detect, 72)),
            ["  nothing found"]
        );
    }
}
