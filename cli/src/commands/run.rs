//! `hedos run <model> <prompt>` — stream a single completion to stdout.

use std::collections::BTreeMap;

use clap::Args;
use kernel::capabilities::CapabilityChunk;
use kernel::records::{Capability, JsonValue};
use serde_json::json;

use crate::error::CliError;
use crate::support::output::Out;
use crate::support::session::{self, Session};

/// Arguments for `run`.
#[derive(Args)]
pub struct RunArgs {
    /// The model to run (name, alias, or id).
    model: String,
    /// The prompt to complete.
    prompt: String,
    /// A system prompt for this run.
    #[arg(long)]
    system: Option<String>,
    /// Cap the number of generated tokens.
    #[arg(long)]
    max_tokens: Option<i64>,
    /// Sampling temperature.
    #[arg(long)]
    temperature: Option<f64>,
}

/// Run the `run` command.
pub async fn run(args: RunArgs, out: &Out) -> Result<(), CliError> {
    let session = Session::open()?;
    let shelf = session.shelf(true).await?;
    let record = session::resolve(&args.model, &shelf, Some(&Capability::chat()))?;

    let payload = chat_payload(&args.prompt, args.max_tokens, args.temperature);
    let mut stream = session
        .kernel
        .invoke_with(
            &record.id,
            Capability::chat(),
            payload,
            args.system.as_deref(),
            None,
        )
        .await?;

    let mut text = String::new();
    while let Some(result) = stream.recv().await {
        match result? {
            CapabilityChunk::Text(chunk) => {
                out.raw(&chunk);
                text.push_str(&chunk);
            }
            CapabilityChunk::Status(status) => out.err(&status),
            _ => {}
        }
    }

    if out.is_json() {
        out.json(&json!({ "model": record.id, "text": text }));
    } else {
        // Terminate the streamed line.
        out.raw("\n");
    }
    Ok(())
}

/// A one-user-turn chat payload with optional sampling knobs.
fn chat_payload(prompt: &str, max_tokens: Option<i64>, temperature: Option<f64>) -> JsonValue {
    let mut message = BTreeMap::new();
    message.insert("role".to_owned(), JsonValue::String("user".to_owned()));
    message.insert("content".to_owned(), JsonValue::String(prompt.to_owned()));

    let mut payload = BTreeMap::new();
    payload.insert(
        "messages".to_owned(),
        JsonValue::Array(vec![JsonValue::Object(message)]),
    );
    if let Some(max_tokens) = max_tokens {
        payload.insert("max_tokens".to_owned(), JsonValue::Int(max_tokens));
    }
    if let Some(temperature) = temperature {
        payload.insert("temperature".to_owned(), JsonValue::Double(temperature));
    }
    JsonValue::Object(payload)
}
