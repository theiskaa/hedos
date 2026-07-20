//! `hedos warm <model>` — load a model into residency with a tiny request.

use std::collections::BTreeMap;

use clap::Args;
use kernel::records::{Capability, JsonValue};

use crate::error::CliError;
use crate::support::interactive;
use crate::support::output::Out;
use crate::support::session::Session;

/// Arguments for `warm`.
#[derive(Args)]
pub struct WarmArgs {
    /// The model to warm (name, alias, or id). Omit to pick one interactively.
    model: Option<String>,
}

/// Run the `warm` command.
pub async fn run(args: WarmArgs, out: &Out) -> Result<(), CliError> {
    let session = Session::open()?;
    let shelf = session.shelf_or_discover().await?;
    let warm = session.warm_set();
    let record =
        interactive::choose_model(out, args.model.as_deref(), &shelf, None, "warm", &warm)?;

    let (capability, payload) = warm_request(record)
        .ok_or_else(|| CliError::new(format!("{} can't be warmed", record.display_name())))?;
    let mut stream = session
        .kernel
        .invoke(&record.id, capability, payload)
        .await?;
    // Drain the (tiny) response to complete the load.
    while let Some(result) = stream.recv().await {
        result?;
    }

    let resident = session.kernel.governor().is_resident(&record.id);
    let held_by_daemon = session
        .kernel
        .resident_models()
        .iter()
        .any(|entry| entry.model_id.as_deref() == Some(record.id.as_str()));
    out.line(&format!(
        "{} is {}",
        record.display_name(),
        if resident || held_by_daemon {
            "warm"
        } else {
            "loaded (residency not tracked for this runtime)"
        },
    ));
    out.json(&serde_json::json!({ "model": record.id, "resident": resident || held_by_daemon }));
    Ok(())
}

/// The smallest request that loads `record`: a one-token chat/complete, or a
/// dot of speech. `None` if the model serves none of those.
fn warm_request(record: &kernel::records::ModelRecord) -> Option<(Capability, JsonValue)> {
    let has = |cap: &Capability| record.capabilities.contains(cap);
    if has(&Capability::chat()) {
        let mut payload = BTreeMap::new();
        payload.insert(
            "messages".to_owned(),
            JsonValue::Array(vec![crate::support::payload::message("user", "hi")]),
        );
        payload.insert("max_tokens".to_owned(), JsonValue::Int(1));
        Some((Capability::chat(), JsonValue::Object(payload)))
    } else if has(&Capability::complete()) {
        let mut payload = BTreeMap::new();
        payload.insert("prompt".to_owned(), JsonValue::String("hi".to_owned()));
        payload.insert("max_tokens".to_owned(), JsonValue::Int(1));
        Some((Capability::complete(), JsonValue::Object(payload)))
    } else if has(&Capability::speak()) {
        let mut payload = BTreeMap::new();
        payload.insert("text".to_owned(), JsonValue::String(".".to_owned()));
        Some((Capability::speak(), JsonValue::Object(payload)))
    } else {
        None
    }
}
