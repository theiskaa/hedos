//! The style vocabulary every screen draws from. It lives apart from the
//! panes because the bench draws with it too, from a plain command as well
//! as from the shelf, and a row must read the same wherever it is drawn.
//!
//! `DIM` is the quiet register, `BOLD` the loud one, `ACCENT` what is in
//! focus, names a mode, or is in motion, `EYEBROW` a heading over a run of
//! rows and the name of a pane, `COOL` where a model comes from and what
//! runs it, and the three state hues are `WARM` for what is loaded or up,
//! `CAUTION` for a warning, `FAILED` for what failed. `BACKDROP` flattens
//! the screen behind a card and `SELECTED_ROW` tints the selected row of a
//! list. Every colour is a fixed `Rgb` chosen against the orange accent, so
//! the panes read as one thing on any dark truecolor terminal instead of
//! taking whatever the palette's green and yellow happen to be; the
//! machine's memory bar is the one place the hues are swatches, not
//! meanings.

use ratatui::style::{Color, Modifier, Style};

/// The three hues the panes are built from: the orange everything is
/// chosen against, a sand a step down from it, and its muted complement.
pub(crate) const ORANGE: Color = Color::Rgb(232, 142, 68);
pub(crate) const SAND: Color = Color::Rgb(198, 168, 128);
pub(crate) const TEAL: Color = Color::Rgb(112, 166, 162);
/// The quiet register: borders, labels, keys, models that can't run here.
/// A warm grey rather than the DIM modifier, which lands anywhere from
/// unreadable to plain white depending on the terminal.
pub(crate) const DIM: Style = Style::new().fg(Color::Rgb(124, 116, 106));
/// The loud register: what the eye should land on first, from the wordmark
/// and warm models to the user's own words in the chat pane.
pub(crate) const BOLD: Style = Style::new().add_modifier(Modifier::BOLD);
/// What is in focus, names a mode, or is in motion: the expanded detail's
/// frame, an input mark, a running task's verb, the spinner, the download
/// bar, the chat's and the cards' titles. The wordmark is the one still
/// thing that wears it, being the orange the rest is built around.
pub(crate) const ACCENT: Style = Style::new().fg(ORANGE);
/// A heading over a run of rows, the name of a pane, and the koala beside
/// the wordmark: the shelf's column headers, the detail's MEMORY, the pull
/// listing's categories, the help's groups. The sand frames without
/// competing with what moves or has focus.
pub(crate) const EYEBROW: Style = Style::new().fg(SAND);
/// Where a model comes from and what runs it: the runtime and store
/// columns and rows. The teal reads as a fact and not a signal.
pub(crate) const COOL: Style = Style::new().fg(TEAL);
/// What is loaded or up: a warm model, a gateway that is on.
pub(crate) const WARM: Style = Style::new().fg(Color::Rgb(128, 196, 136));
/// A warning: a tight fit, a reply that was stopped.
pub(crate) const CAUTION: Style = Style::new().fg(Color::Rgb(230, 186, 96));
/// The selected row of a list: a warm dark tint under the row, so the
/// text keeps its hues where a reversed row would flatten them. Patched
/// over a row, never set, so a dim row stays dim under it.
pub(crate) const SELECTED_ROW: Style = Style::new().bg(Color::Rgb(58, 48, 40));
/// What failed, and nothing else.
pub(crate) const FAILED: Style = Style::new().fg(Color::Rgb(226, 108, 98));
/// The screen behind a card: every colour and emphasis flattened to one
/// near-black grey so the card is the only thing lit. A fixed value, since
/// palette greys land too bright on many terminals to read as a backdrop;
/// the selection's tint goes with the rest.
pub(crate) const BACKDROP: Style = Style::new()
    .fg(Color::Rgb(44, 44, 44))
    .bg(Color::Reset)
    .remove_modifier(Modifier::BOLD);
/// The gutter mark on the selected row of a list, in the cell its leading
/// space took; the one selection signal a terminal without truecolor keeps.
pub(crate) const SELECTED_MARK: &str = "▎";
/// Rows a bordered block spends on its top and bottom edges.
pub(crate) const BORDER_ROWS: u16 = 2;
/// Columns a bordered block spends on its left and right edges.
pub(crate) const BORDER_COLUMNS: u16 = 2;
/// The glyphs of a horizontal bar: filled, then empty.
pub(crate) const BAR_FILLED: &str = "█";
pub(crate) const BAR_EMPTY: &str = "░";
/// The text cursor shown while something is being typed.
pub(crate) const CURSOR: &str = "▏";
/// The glyphs of the spinner that turns while something is waited on, one
/// per tick.
pub(crate) const SPINNER: [&str; 6] = ["⠋", "⠙", "⠸", "⠴", "⠦", "⠇"];

/// The spinner's glyph on tick `ticks`.
pub(crate) fn spinner(ticks: u64) -> &'static str {
    SPINNER[(ticks % SPINNER.len() as u64) as usize]
}
