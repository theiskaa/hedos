//! Which command a model belongs to, for a command handed a model it does not
//! serve: rather than "not found", say what the model does and where to ask it.

use kernel::records::{Capability, ModelRecord};

/// A one-shot generation command and the capability it serves.
struct Command {
    capability: fn() -> Capability,
    does: &'static str,
    name: &'static str,
}

/// The generation commands, in the order a model that does several is
/// pointed.
const COMMANDS: [Command; 4] = [
    Command {
        capability: Capability::audio,
        does: "makes audio from a description",
        name: "audio",
    },
    Command {
        capability: Capability::speak,
        does: "reads text aloud",
        name: "speak",
    },
    Command {
        capability: Capability::image,
        does: "makes images",
        name: "image",
    },
    Command {
        capability: Capability::transcribe,
        does: "transcribes audio",
        name: "transcribe",
    },
];

/// The sentence pointing `record` at the command that serves it, when one of
/// the generation commands other than `asked` does.
pub fn pointer(record: &ModelRecord, asked: Option<&Capability>) -> Option<String> {
    COMMANDS
        .iter()
        .find(|command| {
            let capability = (command.capability)();
            record.can(&capability) && asked != Some(&capability)
        })
        .map(|command| {
            format!(
                "{} {}; use `hedos {} {}`",
                record.display_name(),
                command.does,
                command.name,
                record.display_name()
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel::records::{Modality, ModelSource, SourceKind};

    fn model(name: &str, capabilities: Vec<Capability>) -> ModelRecord {
        ModelRecord::new(
            name,
            Modality::audio(),
            capabilities,
            ModelSource::new(SourceKind::folder(), "/m"),
        )
    }

    #[test]
    fn a_generator_is_pointed_at_audio_and_a_voice_at_speak() {
        let song = model("YuE2-3B", vec![Capability::audio()]);
        assert_eq!(
            pointer(&song, Some(&Capability::speak())).as_deref(),
            Some("YuE2-3B makes audio from a description; use `hedos audio YuE2-3B`")
        );
        let voice = model("Kokoro", vec![Capability::speak()]);
        assert_eq!(
            pointer(&voice, Some(&Capability::audio())).as_deref(),
            Some("Kokoro reads text aloud; use `hedos speak Kokoro`")
        );
    }

    #[test]
    fn a_model_the_asking_command_serves_or_none_serves_has_no_pointer() {
        let song = model("YuE2-3B", vec![Capability::audio()]);
        assert_eq!(pointer(&song, Some(&Capability::audio())), None);
        let unserved = model("YuE2-Vae", Vec::new());
        assert_eq!(pointer(&unserved, None), None);
    }
}
