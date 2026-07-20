//! `hedos image <model> <prompt>` — generate an image and write a PNG file.

use std::collections::BTreeMap;
use std::path::PathBuf;

use clap::Args;
use kernel::jobs::JobEvent;
use kernel::records::{Capability, JsonValue};

use crate::error::CliError;
use crate::support::output::Out;
use crate::support::session::{self, Session};

/// Arguments for `image`.
#[derive(Args)]
pub struct ImageArgs {
    /// The image model (name, alias, or id).
    model: String,
    /// The prompt to render.
    prompt: String,
    /// Number of diffusion steps.
    #[arg(long)]
    steps: Option<i64>,
    /// The random seed.
    #[arg(long)]
    seed: Option<i64>,
    /// Where to write the `.png` (default: `<model>.png` in the current dir).
    #[arg(short, long)]
    output: Option<PathBuf>,
}

/// Run the `image` command.
pub async fn run(args: ImageArgs, out: &Out) -> Result<(), CliError> {
    let session = Session::open()?;
    let shelf = session.shelf_or_discover().await?;
    let record = session::resolve(&args.model, &shelf, Some(&Capability::image()))?;

    let payload = image_payload(&args.prompt, args.steps, args.seed);
    let job_id = session
        .kernel
        .submit(&record.id, Capability::image(), payload)
        .await?;
    let mut events = session.kernel.scheduler().events(&job_id);

    let mut artifacts: Vec<String> = Vec::new();
    while let Some(event) = events.recv().await {
        match event {
            JobEvent::Progress(progress) => {
                out.progress(&format!("{}%", (progress.fraction * 100.0) as i64));
            }
            JobEvent::Status(status) => out.progress(&status),
            JobEvent::Done { result } => {
                artifacts = result;
                break;
            }
            JobEvent::Failed { message } => {
                out.progress_done();
                return Err(CliError::new(message));
            }
            JobEvent::Cancelled => {
                out.progress_done();
                return Err(CliError::new("generation was cancelled"));
            }
            _ => {}
        }
    }
    out.progress_done();

    let artifact = artifacts
        .first()
        .ok_or_else(|| CliError::new(format!("{} produced no image", record.display_name())))?;
    let bytes = session
        .kernel
        .artifact_data(artifact)
        .await?
        .ok_or_else(|| CliError::new("the image artifact is missing"))?;
    let path = args
        .output
        .unwrap_or_else(|| crate::support::paths::default_path(record.display_name(), "png"));
    std::fs::write(&path, bytes)?;

    out.line(&format!("wrote {}", path.display()));
    out.json(&serde_json::json!({
        "model": record.id,
        "path": path.display().to_string(),
    }));
    Ok(())
}

/// An `image` payload: the prompt plus optional steps and seed.
fn image_payload(prompt: &str, steps: Option<i64>, seed: Option<i64>) -> JsonValue {
    let mut payload = BTreeMap::new();
    payload.insert("prompt".to_owned(), JsonValue::String(prompt.to_owned()));
    if let Some(steps) = steps {
        payload.insert("steps".to_owned(), JsonValue::Int(steps));
    }
    if let Some(seed) = seed {
        payload.insert("seed".to_owned(), JsonValue::Int(seed));
    }
    JsonValue::Object(payload)
}
