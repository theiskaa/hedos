//! `hedos transcribe <model> <audio>` — transcribe an audio file to text.

use std::collections::BTreeMap;

use clap::Args;
use kernel::capabilities::CapabilityChunk;
use kernel::records::{Capability, JsonValue};
use serde_json::json;

use crate::error::CliError;
use crate::support::interactive;
use crate::support::output::Out;
use crate::support::session::Session;
use crate::support::spinner::Spinner;

/// Arguments for `transcribe`.
#[derive(Args)]
pub struct TranscribeArgs {
    /// The transcription model (name, alias, or id). Omit to pick one interactively.
    model: Option<String>,
    /// The WAV file to transcribe. Omit to type the path interactively.
    audio: Option<String>,
    /// Force the source language (e.g. `en`); default auto-detects.
    #[arg(long)]
    language: Option<String>,
    /// Translate to English instead of transcribing verbatim.
    #[arg(long)]
    translate: bool,
}

/// Run the `transcribe` command.
pub async fn run(args: TranscribeArgs, out: &Out) -> Result<(), CliError> {
    let session = Session::open()?;
    let shelf = session.shelf_or_discover().await?;
    let warm = session.warm_set();
    let record = interactive::choose_model(
        out,
        args.model.as_deref(),
        &shelf,
        Some(&Capability::transcribe()),
        "transcribe with",
        &warm,
    )?;

    let audio = interactive::text_or_prompt(out, args.audio, "audio file")?;
    // The path passes through untouched: the whisper runtime expands a leading `~`
    // and reads the file itself, naming the path if it cannot — so a tilde path
    // typed at the prompt (where no shell expands it) still resolves.
    let payload = transcribe_payload(&audio, args.language.as_deref(), args.translate);

    let mut stream = session
        .kernel
        .invoke(&record.id, Capability::transcribe(), payload)
        .await?;

    let mut transcript = String::new();
    let mut spinner = Spinner::start(out);
    while let Some(result) = stream.recv().await {
        match result? {
            CapabilityChunk::Text(text) | CapabilityChunk::Segment { text, .. } => {
                spinner.clear();
                out.raw(&text);
                transcript.push_str(&text);
            }
            CapabilityChunk::Status(status) => spinner.set(&status),
            _ => {}
        }
    }
    spinner.clear();

    if out.is_json() {
        out.json(&json!({ "model": record.id, "path": audio, "text": transcript }));
    } else {
        // Terminate the streamed transcript; an empty transcript is valid silence.
        out.raw("\n");
    }
    Ok(())
}

/// A `transcribe` payload: the audio path plus an optional forced language and
/// the translate flag; absent options fall to the runtime's defaults.
fn transcribe_payload(audio: &str, language: Option<&str>, translate: bool) -> JsonValue {
    let mut payload = BTreeMap::new();
    payload.insert("audio".to_owned(), JsonValue::String(audio.to_owned()));
    if let Some(language) = language {
        payload.insert(
            "language".to_owned(),
            JsonValue::String(language.to_owned()),
        );
    }
    if translate {
        payload.insert("translate".to_owned(), JsonValue::Bool(true));
    }
    JsonValue::Object(payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_options_are_carried() {
        let payload = transcribe_payload("/tmp/a.wav", Some("en"), true);
        let fields = payload.as_object().expect("payload is an object");
        assert_eq!(
            fields.get("audio"),
            Some(&JsonValue::String("/tmp/a.wav".to_owned()))
        );
        assert_eq!(
            fields.get("language"),
            Some(&JsonValue::String("en".to_owned()))
        );
        assert_eq!(fields.get("translate"), Some(&JsonValue::Bool(true)));
    }

    #[test]
    fn defaults_omit_language_and_translate() {
        let payload = transcribe_payload("/tmp/a.wav", None, false);
        let fields = payload.as_object().expect("payload is an object");
        assert!(fields.get("audio").is_some());
        assert!(fields.get("language").is_none());
        assert!(fields.get("translate").is_none());
    }
}
