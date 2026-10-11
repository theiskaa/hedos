//! `hedos audio <model> <prompt>`: make a song, music or a sound from a
//! description and write it to a file. The flags a model has no use for (lyrics
//! to a model that cannot sing) are refused before anything runs, with the
//! same check and wording as the gateway's `/v1/audio/generations`.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::PathBuf;

use clap::Args;
use kernel::capabilities::audio_payload;
use kernel::jobs::JobEvent;
use kernel::records::{Capability, JsonValue, ParamSpec, ParamType};

use crate::error::CliError;
use crate::support::interactive;
use crate::support::output::Out;
use crate::support::session::{self, Session};
use crate::support::spinner::Spinner;

/// Arguments for `audio`.
#[derive(Args)]
pub struct AudioArgs {
    /// The audio model (name, alias, or id). Omit to pick one interactively.
    model: Option<String>,
    /// What to make: a style, mood, instruments, a scene. Omit to type it.
    prompt: Option<String>,
    /// A file of lyrics, with [verse] and [chorus] tags on their own lines; `-`
    /// reads them from standard input.
    #[arg(long)]
    lyrics: Option<PathBuf>,
    /// No vocals.
    #[arg(long)]
    instrumental: bool,
    /// How long, in seconds, for a model that takes a length.
    #[arg(long)]
    duration: Option<f64>,
    /// The random seed.
    #[arg(long)]
    seed: Option<i64>,
    /// Number of diffusion steps.
    #[arg(long)]
    steps: Option<i64>,
    /// Guidance scale.
    #[arg(long)]
    guidance: Option<f64>,
    /// What to steer away from.
    #[arg(long)]
    negative: Option<String>,
    /// Any other parameter the model takes, as key=value (repeatable): YuE2's
    /// `cot=melody`, `cfg_scale=1.5`.
    #[arg(long = "set", value_name = "KEY=VALUE")]
    sets: Vec<String>,
    /// The file format: flac or wav.
    #[arg(long, default_value = "flac")]
    format: String,
    /// Where to write the file (default: the prompt as a file name, in the
    /// current dir).
    #[arg(short, long)]
    output: Option<PathBuf>,
}

/// Run the `audio` command.
pub async fn run(args: AudioArgs, out: &Out) -> Result<(), CliError> {
    let session = Session::open()?;
    let shelf = session.shelf_or_discover().await?;
    let warm = session.warm_set_anywhere(&shelf).await;
    if let Some(name) = args.model.as_deref()
        && let Ok(found) = session::resolve(name, &shelf, None)
        && !found.can(&Capability::audio())
        && let Some(pointer) = crate::support::elsewhere::pointer(found, Some(&Capability::audio()))
    {
        return Err(CliError::new(pointer));
    }
    let record = interactive::choose_model(
        out,
        args.model.as_deref(),
        &shelf,
        Some(&Capability::audio()),
        "make audio with",
        &warm,
    )?;

    let prompt = interactive::text_or_prompt(out, args.prompt.clone(), "prompt")?;
    let lyrics = match &args.lyrics {
        Some(path) => Some(read_lyrics(path)?),
        None => None,
    };
    let fields = request_fields(&args, &prompt, lyrics, &record.params)?;
    let payload = audio_payload(record.display_name(), &fields, &record.params)
        .map_err(|error| CliError::new(error.to_string()))?;
    let job_id = session
        .kernel
        .submit(&record.id, Capability::audio(), payload)
        .await?;
    let mut events = session.kernel.scheduler().events(&job_id);

    let mut artifacts: Vec<String> = Vec::new();
    let mut spinner = Spinner::start(out);
    let mut stage = String::new();
    while let Some(event) = events.recv().await {
        match event {
            JobEvent::Queued {
                reason: Some(reason),
            } => spinner.set(&reason),
            JobEvent::Status(status) => {
                spinner.set(&status);
                stage = status;
            }
            JobEvent::Progress(progress) => {
                let percent = (progress.fraction * 100.0) as i64;
                if stage.is_empty() {
                    spinner.set(&format!("{percent}%"));
                } else {
                    spinner.set(&format!("{stage} · {percent}%"));
                }
            }
            JobEvent::Done { result } => {
                artifacts = result;
                break;
            }
            JobEvent::Failed { message, .. } => {
                spinner.clear();
                return Err(CliError::new(message));
            }
            JobEvent::Cancelled => {
                spinner.clear();
                return Err(CliError::new("generation was cancelled"));
            }
            _ => {}
        }
    }
    spinner.clear();

    let artifact = artifacts
        .first()
        .ok_or_else(|| CliError::new(format!("{} produced no audio", record.display_name())))?;
    let bytes = session
        .kernel
        .artifact_data(artifact)
        .await?
        .ok_or_else(|| CliError::new("the audio artifact is missing"))?;
    let extension = session
        .kernel
        .artifact_extension(artifact)
        .await?
        .unwrap_or_else(|| args.format.clone());
    let path = args
        .output
        .unwrap_or_else(|| crate::support::paths::default_path(&prompt, &extension));
    std::fs::write(&path, bytes)?;

    out.line(&format!("wrote {}", path.display()));
    out.json(&serde_json::json!({
        "model": record.id,
        "path": path.display().to_string(),
    }));
    Ok(())
}

/// The lyrics in `path`, or on standard input for `-`.
fn read_lyrics(path: &std::path::Path) -> Result<String, CliError> {
    if path.as_os_str() == "-" {
        let mut lyrics = String::new();
        std::io::stdin().read_to_string(&mut lyrics)?;
        return Ok(lyrics);
    }
    std::fs::read_to_string(path)
        .map_err(|error| CliError::new(format!("reading {}: {error}", path.display())))
}

/// The request the flags describe, named as the audio contract names them,
/// with each `--set` value read the way the model's `params` declare it.
fn request_fields(
    args: &AudioArgs,
    prompt: &str,
    lyrics: Option<String>,
    params: &[ParamSpec],
) -> Result<BTreeMap<String, JsonValue>, CliError> {
    let mut fields = BTreeMap::new();
    fields.insert("prompt".to_owned(), JsonValue::String(prompt.to_owned()));
    fields.insert("format".to_owned(), JsonValue::String(args.format.clone()));
    let flags = [
        ("lyrics", lyrics.map(JsonValue::String)),
        (
            "instrumental",
            args.instrumental.then_some(JsonValue::Bool(true)),
        ),
        ("duration", args.duration.map(JsonValue::Double)),
        ("seed", args.seed.map(JsonValue::Int)),
        ("steps", args.steps.map(JsonValue::Int)),
        ("guidance", args.guidance.map(JsonValue::Double)),
        (
            "negative_prompt",
            args.negative.clone().map(JsonValue::String),
        ),
    ];
    for (key, value) in flags {
        if let Some(value) = value {
            fields.insert(key.to_owned(), value);
        }
    }
    for set in &args.sets {
        let (key, value) = set
            .split_once('=')
            .filter(|(key, _)| !key.is_empty())
            .ok_or_else(|| CliError::new(format!("--set takes key=value, not {set}")))?;
        let spec = params.iter().find(|spec| spec.key == key);
        if fields
            .insert(key.to_owned(), set_value(spec, value))
            .is_some()
        {
            return Err(CliError::new(format!(
                "{key} is given twice; use its flag or --set, not both"
            )));
        }
    }
    Ok(fields)
}

/// A `--set` value as the parameter `spec` declares it: text for a text or
/// enum parameter (so a choice named `1` stays `"1"`), a number or a flag for
/// those. A value that does not parse stays text, and the model's own check
/// says what it should be; a key the model does not declare is read as the
/// JSON it most plainly is, to be refused by that same check.
fn set_value(spec: Option<&ParamSpec>, value: &str) -> JsonValue {
    let text = || JsonValue::String(value.to_owned());
    match spec.map(|spec| spec.param_type) {
        Some(ParamType::String | ParamType::Enum) => text(),
        Some(ParamType::Int) => value.parse().map_or_else(|_| text(), JsonValue::Int),
        Some(ParamType::Float) => value
            .parse::<f64>()
            .ok()
            .filter(|number| number.is_finite())
            .map_or_else(text, JsonValue::Double),
        Some(ParamType::Bool) => value.parse().map_or_else(|_| text(), JsonValue::Bool),
        None => scalar(value),
    }
}

/// A value as the JSON it most plainly is: a whole number, a number, true or
/// false, else text.
fn scalar(value: &str) -> JsonValue {
    if let Ok(number) = value.parse::<i64>() {
        JsonValue::Int(number)
    } else if let Some(number) = value
        .parse::<f64>()
        .ok()
        .filter(|number| number.is_finite())
    {
        JsonValue::Double(number)
    } else if let Ok(flag) = value.parse::<bool>() {
        JsonValue::Bool(flag)
    } else {
        JsonValue::String(value.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct Cli {
        #[command(flatten)]
        args: AudioArgs,
    }

    fn args(line: &[&str]) -> AudioArgs {
        let mut argv = vec!["hedos-audio"];
        argv.extend(line);
        Cli::parse_from(argv).args
    }

    #[test]
    fn flags_become_the_contracts_fields() {
        let args = args(&[
            "yue2",
            "pop",
            "--instrumental",
            "--seed",
            "7",
            "--set",
            "cot=melody",
            "--set",
            "cfg_scale=1.5",
            "--format",
            "wav",
        ]);
        let fields = request_fields(&args, "pop", None, &[]).unwrap();
        assert_eq!(fields.get("instrumental"), Some(&JsonValue::Bool(true)));
        assert_eq!(fields.get("seed"), Some(&JsonValue::Int(7)));
        assert_eq!(fields.get("cot"), Some(&JsonValue::String("melody".into())));
        assert_eq!(fields.get("cfg_scale"), Some(&JsonValue::Double(1.5)));
        assert_eq!(fields.get("format"), Some(&JsonValue::String("wav".into())));
        assert!(!fields.contains_key("duration"));
    }

    #[test]
    fn a_key_given_twice_or_without_a_value_is_refused() {
        let twice = args(&["m", "p", "--seed", "1", "--set", "seed=2"]);
        assert!(request_fields(&twice, "p", None, &[]).is_err());
        let bare = args(&["m", "p", "--set", "cot"]);
        assert!(request_fields(&bare, "p", None, &[]).is_err());
    }

    #[test]
    fn set_values_read_as_their_plainest_json() {
        assert_eq!(scalar("3"), JsonValue::Int(3));
        assert_eq!(scalar("2.5"), JsonValue::Double(2.5));
        assert_eq!(scalar("true"), JsonValue::Bool(true));
        assert_eq!(scalar("melody"), JsonValue::String("melody".into()));
        assert_eq!(scalar("nan"), JsonValue::String("nan".into()));
    }

    #[test]
    fn set_values_follow_the_models_declared_types() {
        let spec = |key: &str, param_type| ParamSpec {
            key: key.to_owned(),
            param_type,
            default_value: None,
            range: None,
            values: None,
        };
        let params = [
            spec("mode", ParamType::Enum),
            spec("title", ParamType::String),
            spec("bars", ParamType::Int),
            spec("swing", ParamType::Float),
            spec("loop", ParamType::Bool),
        ];
        let args = args(&[
            "m",
            "p",
            "--set",
            "mode=1",
            "--set",
            "title=true",
            "--set",
            "bars=8",
            "--set",
            "swing=2",
            "--set",
            "loop=false",
        ]);
        let fields = request_fields(&args, "p", None, &params).unwrap();
        assert_eq!(fields.get("mode"), Some(&JsonValue::String("1".into())));
        assert_eq!(fields.get("title"), Some(&JsonValue::String("true".into())));
        assert_eq!(fields.get("bars"), Some(&JsonValue::Int(8)));
        assert_eq!(fields.get("swing"), Some(&JsonValue::Double(2.0)));
        assert_eq!(fields.get("loop"), Some(&JsonValue::Bool(false)));
        assert_eq!(
            set_value(Some(&params[2]), "eight"),
            JsonValue::String("eight".into()),
            "left as text for the model's check to name"
        );
    }
}
