//! The try-a-prompt modal's state: one line of text for the selected model.

use kernel::records::ModelRecord;

/// The prompt modal.
#[derive(Debug, Clone, PartialEq)]
pub struct PromptModal {
    pub record: ModelRecord,
    pub input: String,
}

impl PromptModal {
    /// An empty prompt for `record`.
    pub fn open(record: ModelRecord) -> Self {
        Self {
            record,
            input: String::new(),
        }
    }

    /// Add `c` to the prompt.
    pub fn type_char(&mut self, c: char) {
        self.input.push(c);
    }

    /// Drop the last character.
    pub fn backspace(&mut self) {
        self.input.pop();
    }

    /// The prompt to send, unless nothing was typed.
    pub fn submit(&self) -> Option<String> {
        let prompt = self.input.trim();
        (!prompt.is_empty()).then(|| prompt.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel::records::{Capability, Modality, ModelSource, SourceKind};

    #[test]
    fn a_blank_prompt_is_not_sent() {
        let record = ModelRecord::new(
            "m",
            Modality::text(),
            vec![Capability::chat()],
            ModelSource::new(SourceKind::ollama(), "m"),
        );
        let mut modal = PromptModal::open(record);
        assert_eq!(modal.submit(), None);
        for c in "  hi ".chars() {
            modal.type_char(c);
        }
        assert_eq!(modal.submit(), Some("hi".to_owned()));
        modal.backspace();
        modal.backspace();
        assert_eq!(modal.submit(), Some("h".to_owned()));
        modal.backspace();
        assert_eq!(modal.submit(), None);
    }
}
