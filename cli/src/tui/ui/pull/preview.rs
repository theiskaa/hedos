//! The preview of the row under the cursor: where it comes from, its name
//! arriving letter by letter, its facts and blurb, how it fits beside what
//! is loaded, the files or layers its plan fetches and where they land, and
//! one button: `enter` pull, which fills as the download runs once pressed.

use kernel::profiles::FitVerdict;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use super::Look;
use crate::tui::app::App;
use crate::tui::palette::{
    ACCENT_DIM, AMBER, BUTTON_GHOST, BUTTON_MUTED, INK_COLOR, LINE_STRONG, PAPER, READ, SOFT_COLOR,
    TRACK_DARK, WHITE, mix,
};
use crate::tui::pull::{Kind, Offer, OnShelf, Plan, PullModal};
use crate::tui::strip::TaskRow;
use crate::tui::tasks::TaskState;
use crate::tui::text;
use crate::tui::ui::card::Card;
use crate::tui::ui::{BOLD, CAUTION, DIM, INK, SOFT, section, spinner};

/// How fast the name arrives, and how long the gauge takes to grow.
const REVEAL_PER_MS: u64 = 4;
const GROW_MS: u64 = 220;
/// How long a pressed button flashes.
const FLASH_MS: u64 = 180;
/// How far the shimmer runs along a filling button, and how wide it lights.
const SHIMMER_MS: f32 = 1600.0;
const SHIMMER_HALF_WIDTH: f32 = 4.0;
/// The files a plan lists before the rest are summed.
const FILES_SHOWN: usize = 3;

/// Draw the preview into `area`; `strip` when it sits under the results,
/// where it keeps to the name, the fit and the button.
pub(super) fn draw(
    frame: &mut Frame,
    area: Rect,
    modal: &PullModal,
    app: &App,
    look: &Look,
    strip: bool,
) {
    let offer = modal.selected_offer();
    let right = offer.map_or_else(Vec::new, |offer| vec![Span::styled(provider(offer), DIM)]);
    Card::new(vec![Span::styled("preview", BOLD)])
        .right(right)
        .render(area, frame.buffer_mut());
    let inner = Card::text_inner(area);
    let Some(offer) = offer else {
        frame.render_widget(Paragraph::new(Span::styled("nothing selected", DIM)), inner);
        return;
    };
    let width = inner.width as usize;
    let task = app.tasks.pull_for(&offer.reference);
    let mut lines = if strip {
        vec![
            name_line(offer, modal, look, width),
            fit_line(offer, modal, width),
        ]
    } else {
        full(offer, modal, app, look, width)
    };
    let button_rows = 3;
    let caption_row = usize::from(!strip);
    let room = (inner.height as usize).saturating_sub(button_rows + caption_row);
    lines.truncate(room);
    lines.resize(room, Line::default());
    frame.render_widget(Paragraph::new(lines), inner);
    if inner.height as usize >= button_rows {
        let button_y = inner.y + room as u16;
        let caption = button(
            frame,
            Rect::new(inner.x, button_y, inner.width, 3),
            offer,
            modal,
            task,
            look,
        );
        if !strip && inner.height as usize > room + button_rows {
            frame.buffer_mut().set_stringn(
                inner.x,
                button_y + 3,
                text::clip(&caption, width),
                width,
                DIM,
            );
        }
    }
}

/// `ollama` or `hugging face`.
fn provider(offer: &Offer) -> &'static str {
    if offer.provider.as_str() == "huggingface" {
        "hugging face"
    } else {
        "ollama"
    }
}

/// The name, after its owner or registry, arriving letter by letter.
fn name_line(offer: &Offer, modal: &PullModal, look: &Look, width: usize) -> Line<'static> {
    let name = match offer.reference.rsplit_once('/') {
        Some((_, name)) => name,
        None => &offer.reference,
    };
    Line::from(
        look.motion
            .reveal(&text::clip(name, width), modal.selected_at, REVEAL_PER_MS)
            .into_iter()
            .map(|(piece, ramp)| Span::styled(piece, if ramp { DIM } else { BOLD }))
            .collect::<Vec<_>>(),
    )
}

/// `fits · needs 7 of 64 GiB`, the verdict bold in its colour.
fn fit_line(offer: &Offer, modal: &PullModal, width: usize) -> Line<'static> {
    let memory = modal.budget_bytes(offer);
    let (word, style) = match (offer.shelf, modal.fit(offer)) {
        (Some(OnShelf::Present), _) => ("on the shelf", SOFT),
        (_, Some(FitVerdict::TooLarge)) => ("too big", DIM),
        (_, Some(FitVerdict::TightFit)) => ("tight", CAUTION),
        (_, Some(_)) => ("fits", INK),
        (_, None) => ("size unknown", DIM),
    };
    let mut spans = vec![Span::styled(word.to_owned(), style.patch(BOLD))];
    if let Some(bytes) = modal.size(offer) {
        spans.push(Span::styled(
            text::clip(
                &format!(
                    " · needs {} of {} GiB",
                    text::gib_short(bytes),
                    text::gib_short(memory as i64)
                ),
                width.saturating_sub(word.width()),
            ),
            SOFT,
        ));
    }
    Line::from(spans)
}

/// The preview's rows beside the results.
fn full(
    offer: &Offer,
    modal: &PullModal,
    app: &App,
    look: &Look,
    width: usize,
) -> Vec<Line<'static>> {
    let owner = match offer.reference.rsplit_once('/') {
        Some((owner, _)) => format!("{owner}/"),
        None if offer.provider.as_str() == "ollama" => "registry.ollama.ai/library/".to_owned(),
        None => String::new(),
    };
    let facts = match (offer.category, offer.bytes) {
        (Some(category), Some(bytes)) => {
            format!("{} · {}", Kind::Of(category).label(), text::bytes(bytes))
        }
        (Some(category), None) => Kind::Of(category).label().to_owned(),
        (None, _) => offer.note.clone(),
    };
    let mut lines = vec![
        Line::default(),
        Line::from(Span::styled(text::clip(&owner, width), DIM)),
        name_line(offer, modal, look, width),
        Line::from(Span::styled(text::clip(&facts, width), SOFT)),
        Line::default(),
    ];
    if offer.category.is_some() {
        for piece in crate::tui::wrap::wrap(&offer.note, width)
            .into_iter()
            .take(3)
        {
            lines.push(Line::from(Span::styled(piece, Style::new().fg(READ))));
        }
        lines.push(Line::default());
    }
    lines.push(section("FIT", width));
    lines.push(fit_line(offer, modal, width));
    lines.extend(gauge(offer, modal, app, look, width));
    lines.push(Line::default());
    lines.push(section(
        if offer.provider.as_str() == "huggingface" {
            "FILES"
        } else {
            "LAYERS"
        },
        width,
    ));
    lines.extend(plan_lines(offer, modal, look, width));
    lines
}

/// The machine's memory as a full-width gauge: what is loaded, what this
/// model would take, the rest; and a legend under it.
fn gauge(
    offer: &Offer,
    modal: &PullModal,
    app: &App,
    look: &Look,
    width: usize,
) -> Vec<Line<'static>> {
    let memory = modal.budget_bytes(offer);
    let Some(bytes) = modal.size(offer).filter(|_| memory > 0 && width > 0) else {
        return Vec::new();
    };
    let loaded = app.facts.resident_bytes().max(0);
    let per = memory as f64 / width as f64;
    let held = ((loaded as f64 / per).round() as usize).min(width);
    let grown = f64::from(look.motion.eased(modal.selected_at, GROW_MS));
    let mine = ((bytes as f64 / per * grown).round() as usize)
        .max(1)
        .min(width - held);
    let colour = match modal.fit(offer) {
        Some(FitVerdict::TightFit) => AMBER,
        Some(FitVerdict::TooLarge) => mix(LINE_STRONG, SOFT_COLOR, 0.4),
        _ => INK_COLOR,
    };
    let left = memory as i64 - loaded - bytes;
    vec![
        Line::from(vec![
            Span::styled("━".repeat(held), Style::new().fg(ACCENT_DIM)),
            Span::styled("━".repeat(mine), Style::new().fg(colour)),
            Span::styled(
                "━".repeat(width - held - mine),
                Style::new().fg(LINE_STRONG),
            ),
        ]),
        Line::from(vec![
            Span::styled("■ ", Style::new().fg(ACCENT_DIM)),
            Span::styled(format!("loaded {}   ", text::gib(loaded)), DIM),
            Span::styled("■ ", Style::new().fg(colour)),
            Span::styled(format!("this {}   ", text::gib(bytes)), DIM),
            Span::styled(
                if left >= 0 {
                    format!("· {} left", text::gib(left))
                } else {
                    format!("· over by {}", text::gib(-left))
                },
                if left >= 0 { DIM } else { SOFT },
            ),
        ]),
    ]
}

/// What the plan says: the files or layers it fetches, where they land,
/// and what is still to download; or that it is on its way, gated, or
/// could not be made.
fn plan_lines(offer: &Offer, modal: &PullModal, look: &Look, width: usize) -> Vec<Line<'static>> {
    match modal.plan(offer) {
        None if offer.shelf == Some(OnShelf::Present) => {
            vec![Line::from(Span::styled("on the shelf already", DIM))]
        }
        None => vec![Line::from(Span::styled(
            "planned once the cursor rests here",
            DIM,
        ))],
        Some(Plan::Pending(_)) => vec![Line::from(vec![
            Span::styled(format!("{} ", spinner(look.spin_frame)), INK),
            Span::styled("planning", SOFT),
        ])],
        Some(Plan::Gated) => vec![Line::from(Span::styled(
            "gated · add a Hugging Face token first",
            CAUTION,
        ))],
        Some(Plan::Failed(reason)) => crate::tui::wrap::wrap(reason, width)
            .into_iter()
            .take(2)
            .map(|piece| Line::from(Span::styled(piece, DIM)))
            .collect(),
        Some(Plan::Ready(plan)) => {
            let mut lines = Vec::new();
            for file in plan.files.iter().take(FILES_SHOWN) {
                let size = file.bytes.map_or_else(String::new, text::bytes);
                let path = text::clip(
                    file.path.rsplit('/').next().unwrap_or(&file.path),
                    width.saturating_sub(size.width() + 2),
                );
                let pad = width.saturating_sub(path.width() + size.width());
                lines.push(Line::from(vec![
                    Span::styled(path, SOFT),
                    Span::raw(" ".repeat(pad)),
                    Span::styled(size, SOFT),
                ]));
            }
            if plan.files.len() > FILES_SHOWN {
                let rest: i64 = plan.files[FILES_SHOWN..]
                    .iter()
                    .filter_map(|file| file.bytes)
                    .sum();
                let more = format!("+ {} more", plan.files.len() - FILES_SHOWN);
                let size = text::bytes(rest);
                let pad = width.saturating_sub(more.width() + size.width());
                lines.push(Line::from(vec![
                    Span::styled(more, DIM),
                    Span::raw(" ".repeat(pad)),
                    Span::styled(size, DIM),
                ]));
            }
            lines.push(Line::from(vec![
                Span::styled("to   ", DIM),
                Span::styled(
                    text::elide_middle(&text::at_home(&plan.destination), width.saturating_sub(5)),
                    SOFT,
                ),
            ]));
            if let (Some(total), Some(remaining)) = (plan.total_bytes, plan.remaining_bytes)
                && remaining < total
            {
                lines.push(Line::from(vec![
                    Span::styled("get  ", DIM),
                    Span::styled(
                        format!("{} more · the rest is on disk", text::bytes(remaining)),
                        SOFT,
                    ),
                ]));
            }
            lines
        }
    }
}

/// The button, three rows of half blocks with the label on the middle one;
/// what it says under it comes back for the caption.
fn button(
    frame: &mut Frame,
    area: Rect,
    offer: &Offer,
    modal: &PullModal,
    task: Option<&TaskRow>,
    look: &Look,
) -> String {
    let width = area.width as usize;
    let size = offer
        .bytes
        .or_else(|| match modal.plan(offer) {
            Some(Plan::Ready(plan)) => plan.total_bytes,
            _ => None,
        })
        .map(text::bytes);
    if let Some(row) = task {
        match &row.state {
            TaskState::Downloading(progress) => {
                let fraction = progress.fraction().unwrap_or(0.0);
                let filled = fraction * width as f64;
                let band = look.motion.age(0).map(|age| {
                    (age * 1000.0 % SHIMMER_MS) / SHIMMER_MS * (filled as f32 + 10.0) - 5.0
                });
                let ground = |cell: usize| {
                    if (cell as f64) < filled {
                        let lit = band.map_or(0.0, |band| {
                            (1.0 - (cell as f32 - band).abs() / SHIMMER_HALF_WIDTH).clamp(0.0, 1.0)
                        });
                        mix(SOFT_COLOR, WHITE, 0.6 * lit)
                    } else {
                        TRACK_DARK
                    }
                };
                let label = match progress.total_bytes {
                    Some(total) => format!(
                        "pulling {}% · {} of {}",
                        (fraction * 100.0) as u64,
                        text::bytes(progress.bytes_downloaded),
                        text::bytes(total)
                    ),
                    None => format!(
                        "pulling · {} so far",
                        text::bytes(progress.bytes_downloaded)
                    ),
                };
                pill(frame, area, &ground, &label, |cell| {
                    if (cell as f64) < filled {
                        PAPER
                    } else {
                        INK_COLOR
                    }
                });
                return "it runs on if you quit · P on the shelf shows every pull".to_owned();
            }
            TaskState::Running | TaskState::Status(_) => {
                pill(frame, area, &|_| BUTTON_GHOST, "starting…", |_| {
                    SOFT_COLOR
                });
                return "it runs on if you quit".to_owned();
            }
            TaskState::Done(_) => {
                pill(
                    frame,
                    area,
                    &|_| BUTTON_MUTED,
                    "✓ pulled · on the shelf",
                    |_| INK_COLOR,
                );
                return "esc, then select it to try it".to_owned();
            }
            TaskState::Failed(_) | TaskState::Stopped(_) => {}
        }
    }
    let muted = |frame: &mut Frame, label: &str| {
        pill(frame, area, &|_| BUTTON_MUTED, label, |_| SOFT_COLOR);
    };
    if offer.shelf == Some(OnShelf::Present) {
        muted(frame, "already on the shelf");
        return "esc, then select it on the shelf".to_owned();
    }
    let memory = modal.budget_bytes(offer);
    if modal.fit(offer) == Some(FitVerdict::TooLarge) {
        muted(
            frame,
            &format!("too big for {} GiB", text::gib_short(memory as i64)),
        );
        return "a smaller quantization of it may fit".to_owned();
    }
    match modal.plan(offer) {
        Some(Plan::Gated) => {
            muted(frame, "gated · needs a token");
            "add a Hugging Face token, then try again".to_owned()
        }
        Some(Plan::Failed(_)) => {
            muted(frame, "could not plan it");
            "move away and back to try again".to_owned()
        }
        Some(Plan::Ready(_)) => {
            let pressed = modal
                .pressed_at
                .is_some_and(|at| look.motion.progress(at, FLASH_MS) < 1.0);
            let ground = if pressed { WHITE } else { INK_COLOR };
            let label = match size {
                Some(size) => format!("enter   pull {size}"),
                None => "enter   pull".to_owned(),
            };
            pill(frame, area, &|_| ground, &label, |_| PAPER);
            "nothing moves until you press enter".to_owned()
        }
        Some(Plan::Pending(_)) if modal.armed(offer) => {
            pill(
                frame,
                area,
                &|_| BUTTON_GHOST,
                "starts once planned",
                |_| INK_COLOR,
            );
            "the pull starts as soon as its plan lands".to_owned()
        }
        Some(Plan::Pending(_)) | None => {
            let label = match size {
                Some(size) => format!("enter   pull {size}"),
                None => "enter   pull".to_owned(),
            };
            pill(frame, area, &|_| BUTTON_GHOST, &label, |_| SOFT_COLOR);
            "nothing moves until you press enter".to_owned()
        }
    }
}

/// A pill: `▗▄▄▖` over the label on its ground, `▝▀▀▘` under it, each cell's
/// ground from `ground`, the label centred and coloured cell by cell.
fn pill(
    frame: &mut Frame,
    area: Rect,
    ground: &dyn Fn(usize) -> Color,
    label: &str,
    ink: impl Fn(usize) -> Color,
) {
    let width = area.width as usize;
    if width < 2 || area.height < 3 {
        return;
    }
    let buf = frame.buffer_mut();
    for cell in 0..width {
        let x = area.x + cell as u16;
        let (top, bottom) = match cell {
            0 => ("▗", "▝"),
            _ if cell == width - 1 => ("▖", "▘"),
            _ => ("▄", "▀"),
        };
        let colour = ground(cell);
        buf[(x, area.y)].set_symbol(top).set_fg(colour);
        buf[(x, area.y + 2)].set_symbol(bottom).set_fg(colour);
        buf[(x, area.y + 1)].set_symbol(" ").set_bg(colour);
    }
    let label = text::clip(label, width.saturating_sub(4));
    let start = (width.saturating_sub(label.width())) / 2;
    let mut cell = start;
    for grapheme in unicode_segmentation::UnicodeSegmentation::graphemes(label.as_str(), true) {
        let x = area.x + cell as u16;
        buf[(x, area.y + 1)]
            .set_symbol(grapheme)
            .set_fg(ink(cell))
            .set_style(Style::new().add_modifier(ratatui::style::Modifier::BOLD));
        cell += grapheme.width().max(1);
    }
}
