//! The words a recommendation is shown with, shared by `hedos recommend`, the
//! `hedos pull` picker and the UI's pull screen.

use kernel::install::recommend::Note;
use kernel::machine::Machine;

use crate::support::text;

/// The machine a list is recommended for: `Apple M5 Pro · 51.8 GiB`, its chip
/// (when it names one) and what it can give a model.
pub(crate) fn machine_label(machine: &Machine) -> String {
    let budget = text::gib_short(i64::try_from(machine.models_budget_bytes()).unwrap_or(i64::MAX));
    match machine.chip.as_deref() {
        Some(chip) => format!("{chip} · {budget} GiB"),
        None => format!("{budget} GiB"),
    }
}

/// A note as a few words for a row: `needs Ollama`, `short on disk`.
pub(crate) fn note_label(note: &Note) -> String {
    match note {
        Note::Install { engine, .. } => {
            format!("needs {}", engine.label().unwrap_or(engine.as_str()))
        }
        Note::Disk { .. } => "short on disk".to_owned(),
    }
}

/// The notes of a row joined for its tail, empty when there are none.
pub(crate) fn notes_label(notes: &[Note]) -> String {
    notes.iter().map(note_label).collect::<Vec<_>>().join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel::machine::{Device, DeviceKind, Engine, OLLAMA_INSTALL_HINT, UV_INSTALL_HINT};

    #[test]
    fn notes_read_as_a_few_words() {
        let notes = [
            Note::Install {
                engine: Engine::Ollama,
                hint: OLLAMA_INSTALL_HINT,
            },
            Note::Disk { needs: 10, free: 5 },
        ];
        assert_eq!(notes_label(&notes), "needs Ollama, short on disk");
        assert_eq!(
            note_label(&Note::Install {
                engine: Engine::Torch,
                hint: UV_INSTALL_HINT
            }),
            "needs uv"
        );
        assert_eq!(notes_label(&[]), "");
    }

    #[test]
    fn a_machine_reads_as_its_chip_and_what_it_gives_a_model() {
        let mut machine = Machine::with_memory(64 << 30);
        assert_eq!(machine_label(&machine), "64 GiB");
        machine.chip = Some("Apple M5 Pro".to_owned());
        machine.devices = vec![Device {
            name: "Apple M5 Pro".to_owned(),
            kind: DeviceKind::Unified,
            memory_bytes: 53084 << 20,
        }];
        assert_eq!(machine_label(&machine), "Apple M5 Pro · 51.8 GiB");
    }
}
