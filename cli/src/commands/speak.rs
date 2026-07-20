//! `hedos speak <model> <text>` — synthesize speech and write a WAV file.

use std::collections::BTreeMap;
use std::path::PathBuf;

use clap::Args;
use kernel::capabilities::CapabilityChunk;
use kernel::records::{Capability, JsonValue};
use runtime::sidecar::DEFAULT_SAMPLE_RATE;

use crate::error::CliError;
use crate::support::interactive;
use crate::support::output::Out;
use crate::support::session::Session;
use crate::support::spinner::Spinner;

/// Arguments for `speak`.
#[derive(Args)]
pub struct SpeakArgs {
    /// The speech model (name, alias, or id). Omit to pick one interactively.
    model: Option<String>,
    /// The text to speak. Omit to type it interactively.
    text: Option<String>,
    /// The voice to use (default: the model's first bundled voice).
    #[arg(long)]
    voice: Option<String>,
    /// Playback speed multiplier.
    #[arg(long, default_value_t = 1.0)]
    speed: f64,
    /// Where to write the `.wav` (default: `<model>.wav` in the current dir).
    #[arg(short, long)]
    output: Option<PathBuf>,
}

/// Run the `speak` command.
pub async fn run(args: SpeakArgs, out: &Out) -> Result<(), CliError> {
    let session = Session::open()?;
    let shelf = session.shelf_or_discover().await?;
    let warm = session.warm_set();
    let record = interactive::choose_model(
        out,
        args.model.as_deref(),
        &shelf,
        Some(&Capability::speak()),
        "speak with",
        &warm,
    )?;

    let text = interactive::text_or_prompt(out, args.text, "text")?;
    let voice = choose_voice(out, &session, &record.id, args.voice).await?;
    let payload = speak_payload(&text, voice.as_deref(), args.speed);
    let mut stream = session
        .kernel
        .invoke(&record.id, Capability::speak(), payload)
        .await?;

    let mut pcm: Vec<u8> = Vec::new();
    let mut sample_rate = DEFAULT_SAMPLE_RATE;
    let mut spinner = Spinner::start(out);
    while let Some(result) = stream.recv().await {
        match result? {
            CapabilityChunk::Audio(frame) => {
                if pcm.is_empty() {
                    sample_rate = frame.sample_rate;
                }
                pcm.extend_from_slice(&frame.data);
            }
            CapabilityChunk::Status(status) => spinner.set(&status),
            _ => {}
        }
    }
    spinner.clear();
    if pcm.is_empty() {
        return Err(CliError::new(format!(
            "{} produced no audio",
            record.display_name()
        )));
    }

    let rate = u32::try_from(sample_rate).unwrap_or(DEFAULT_SAMPLE_RATE as u32);
    let wav = runtime::audio::wav_from_pcm(&pcm, rate);
    let path = args
        .output
        .unwrap_or_else(|| crate::support::paths::default_path(&text, "wav"));
    std::fs::write(&path, wav)?;

    out.line(&format!("wrote {}", path.display()));
    out.json(&serde_json::json!({
        "model": record.id,
        "path": path.display().to_string(),
        "voice": voice,
    }));
    Ok(())
}

/// Resolve the voice: an explicit `--voice`, else the model's only voice, else —
/// when several exist and the session is interactive — a picker. Falls back to the
/// first voice when non-interactive so scripts keep their prior behavior.
async fn choose_voice(
    out: &Out,
    session: &Session,
    model_id: &str,
    requested: Option<String>,
) -> Result<Option<String>, CliError> {
    if let Some(voice) = requested {
        return Ok(Some(voice));
    }
    let voices = session.kernel.voices(model_id).await?;
    if voices.len() > 1 && interactive::is_interactive(out) {
        let index = interactive::select_index("voice", &voices)?;
        return Ok(voices.into_iter().nth(index));
    }
    Ok(voices.into_iter().next())
}

/// A `speak` payload: text plus an optional voice and a speed multiplier.
fn speak_payload(text: &str, voice: Option<&str>, speed: f64) -> JsonValue {
    let mut payload = BTreeMap::new();
    payload.insert("text".to_owned(), JsonValue::String(text.to_owned()));
    if let Some(voice) = voice {
        payload.insert("voice".to_owned(), JsonValue::String(voice.to_owned()));
    }
    payload.insert("speed".to_owned(), JsonValue::Double(speed));
    JsonValue::Object(payload)
}
