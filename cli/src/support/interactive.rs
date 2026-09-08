//! Interactive prompts: a fuzzy model picker, free-text input, and yes/no
//! confirmation, built on `dialoguer`. Every entry point is gated on the session
//! being interactive — a real terminal on stdin and stdout and not `--json` — so
//! scripts, pipes, and machine mode never block on a prompt.

use std::collections::HashSet;
use std::io::IsTerminal;

use dialoguer::theme::ColorfulTheme;
use dialoguer::{Confirm, FuzzySelect, Input};
use kernel::records::{Capability, ModelRecord, ModelState};

use crate::error::CliError;
use crate::support::machine;
use crate::support::output::Out;
use crate::support::session;
use crate::support::shelf_table;

/// Whether prompting is possible: stdin is a terminal and the caller did not ask
/// for JSON. When false, callers fall back to a hard error rather than hang
/// waiting on input. Only stdin is checked — dialoguer draws on stderr, so a
/// redirected stdout (`hedos run model > out.txt`) can still prompt.
pub fn is_interactive(out: &Out) -> bool {
    !out.is_json() && std::io::stdin().is_terminal()
}

/// Resolve `arg` to a model, or — when it is absent and the session is
/// interactive — let the user pick from the capability-eligible models.
///
/// A model whose weights are gone is refused here rather than further down,
/// where the failure would be about whatever the missing files broke instead of
/// about the files being missing.
///
/// With no `arg` and no terminal, this is a hard error, so a missing positional
/// never silently blocks a pipe.
pub fn choose_model<'a>(
    out: &Out,
    arg: Option<&str>,
    shelf: &'a [ModelRecord],
    capability: Option<&Capability>,
    prompt: &str,
    warm: &HashSet<String>,
) -> Result<&'a ModelRecord, CliError> {
    let record = choose_any_model(out, arg, shelf, capability, prompt, warm)?;
    if record.state == ModelState::Missing {
        let name = record.display_name();
        return Err(CliError::new(format!(
            "{name}'s weights are gone — pull it again, or drop the record with `hedos rm {name}`"
        )));
    }
    Ok(record)
}

/// [`choose_model`] without the refusal, for the one command that acts on the
/// record rather than on the model it names: a shelf entry whose weights are
/// gone is exactly what removal is for.
pub fn choose_any_model<'a>(
    out: &Out,
    arg: Option<&str>,
    shelf: &'a [ModelRecord],
    capability: Option<&Capability>,
    prompt: &str,
    warm: &HashSet<String>,
) -> Result<&'a ModelRecord, CliError> {
    if let Some(query) = arg {
        return session::resolve(query, shelf, capability);
    }

    let candidates: Vec<&ModelRecord> = shelf
        .iter()
        .filter(|record| capability.is_none_or(|cap| record.capabilities.contains(cap)))
        .collect();
    if candidates.is_empty() {
        let serving =
            capability.map_or_else(String::new, |cap| format!(" serving {}", cap.as_str()));
        return Err(CliError::new(format!(
            "no models{serving} on the shelf — install one with `hedos pull <ref>`"
        )));
    }
    if !is_interactive(out) {
        return Err(CliError::new(
            "no model given — pass a name, or run in a terminal to pick one",
        ));
    }
    select_model(prompt, &candidates, warm)
}

/// Return `arg` when present, or — when it is absent and the session is
/// interactive — read it from an input prompt labelled `label`. With no `arg` and
/// no terminal, this is a hard error.
pub fn text_or_prompt(out: &Out, arg: Option<String>, label: &str) -> Result<String, CliError> {
    match arg {
        Some(text) => Ok(text),
        None if is_interactive(out) => input(label, false),
        None => Err(CliError::new(format!(
            "no {label} given — pass it as an argument"
        ))),
    }
}

/// Present `candidates` as an aligned, fuzzy-filterable list and return the chosen
/// record. Escape or interrupt cancels. The caller must have confirmed the session
/// is interactive.
pub fn select_model<'a>(
    prompt: &str,
    candidates: &[&'a ModelRecord],
    warm: &HashSet<String>,
) -> Result<&'a ModelRecord, CliError> {
    // Models that resolved a runtime come first, so the servable one is the
    // default rather than a look-alike that can't actually run.
    let mut ordered: Vec<&ModelRecord> = candidates.to_vec();
    ordered.sort_by_cached_key(|record| {
        (
            record.runtime.id.is_none(),
            record.display_name().to_lowercase(),
        )
    });

    let labels = shelf_table::picker_labels(&ordered, warm, machine::memory_budget_bytes());
    let index = select_index(prompt, &labels)?;
    ordered
        .get(index)
        .copied()
        .ok_or_else(|| CliError::new("nothing selected"))
}

/// Present `items` as a fuzzy-filterable list and return the chosen index. Escape
/// or interrupt cancels.
pub fn select_index(prompt: &str, items: &[String]) -> Result<usize, CliError> {
    FuzzySelect::with_theme(&ColorfulTheme::default())
        .with_prompt(prompt)
        .items(items)
        .default(0)
        .interact_opt()
        .map_err(to_error)?
        .ok_or_else(cancelled)
}

/// Read a line of free text for `prompt`. Empty input is allowed when
/// `allow_empty` is set; otherwise the prompt repeats.
pub fn input(prompt: &str, allow_empty: bool) -> Result<String, CliError> {
    Input::<String>::with_theme(&ColorfulTheme::default())
        .with_prompt(prompt)
        .allow_empty(allow_empty)
        .interact_text()
        .map_err(to_error)
}

/// Ask a yes/no question, returning the answer with `default` pre-selected.
pub fn confirm(prompt: &str, default: bool) -> Result<bool, CliError> {
    Confirm::with_theme(&ColorfulTheme::default())
        .with_prompt(prompt)
        .default(default)
        .interact()
        .map_err(to_error)
}

/// The error for a user-cancelled prompt (Escape or Ctrl-C at the picker).
fn cancelled() -> CliError {
    CliError::new("cancelled")
}

/// Map a `dialoguer` error into a CLI error.
fn to_error(error: dialoguer::Error) -> CliError {
    CliError::new(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel::records::{Modality, ModelSource, SourceKind};

    fn shelf(state: ModelState) -> Vec<ModelRecord> {
        let mut record = ModelRecord::new(
            "gemma",
            Modality::text(),
            vec![Capability::chat()],
            ModelSource::new(SourceKind::file(), "/models/gemma.gguf"),
        );
        record.state = state;
        vec![record]
    }

    /// A named model resolves without touching the terminal, so these run the
    /// refusal and nothing else.
    fn chosen(state: ModelState) -> Result<String, CliError> {
        let shelf = shelf(state);
        choose_model(
            &Out::new(false),
            Some("gemma"),
            &shelf,
            None,
            "run",
            &HashSet::new(),
        )
        .map(|record| record.id.clone())
    }

    #[test]
    fn a_model_whose_weights_are_gone_is_refused_with_the_reason() {
        let error = chosen(ModelState::Missing).expect_err("the weights are gone");
        assert!(error.message.contains("weights are gone"), "{error:?}");
        assert!(
            error.message.contains("hedos rm gemma"),
            "and says what to do: {error:?}"
        );
    }

    #[test]
    fn a_model_still_on_disk_is_chosen() {
        assert!(chosen(ModelState::Ready).is_ok());
        assert!(chosen(ModelState::Unresolved).is_ok());
    }

    #[test]
    fn removal_reaches_a_model_whose_weights_are_gone() {
        let shelf = shelf(ModelState::Missing);
        let record = choose_any_model(
            &Out::new(false),
            Some("gemma"),
            &shelf,
            None,
            "remove",
            &HashSet::new(),
        )
        .expect("a gone record is exactly what removal is for");
        assert_eq!(record.display_name(), "gemma");
    }
}
