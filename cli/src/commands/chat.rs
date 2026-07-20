//! `hedos chat <model>` — an interactive headless chat that reads turns from
//! stdin and streams replies to stdout.

use std::collections::BTreeMap;
use std::io::{BufRead, IsTerminal, Write};

use clap::Args;
use kernel::capabilities::CapabilityChunk;
use kernel::records::{Capability, JsonValue};

use crate::error::CliError;
use crate::support::interactive;
use crate::support::output::Out;
use crate::support::payload::message;
use crate::support::session::Session;

/// Arguments for `chat`.
#[derive(Args)]
pub struct ChatArgs {
    /// The model to chat with (name, alias, or id). Omit to pick one interactively.
    model: Option<String>,
    /// A system prompt for the conversation.
    #[arg(long)]
    system: Option<String>,
    /// Cap the number of generated tokens per reply.
    #[arg(long)]
    max_tokens: Option<i64>,
}

/// Run the `chat` command; reads turns until end-of-input (Ctrl-D).
pub async fn run(args: ChatArgs, out: &Out) -> Result<(), CliError> {
    let session = Session::open()?;
    let shelf = session.shelf_or_discover().await?;
    let warm = session.warm_set();
    let record = interactive::choose_model(
        out,
        args.model.as_deref(),
        &shelf,
        Some(&Capability::chat()),
        "chat with",
        &warm,
    )?;

    let tty = std::io::stdin().is_terminal() && !out.is_json();
    if tty {
        out.err(&format!(
            "chatting with {} — Ctrl-D to end",
            record.display_name()
        ));
    }

    let mut history: Vec<JsonValue> = Vec::new();
    loop {
        if tty {
            eprint!("› ");
            let _ = std::io::stderr().flush();
        }
        let mut line = String::new();
        if std::io::stdin().lock().read_line(&mut line)? == 0 {
            break; // Ctrl-D
        }
        let prompt = line.trim_end();
        if prompt.is_empty() {
            continue;
        }

        history.push(message("user", prompt));
        let payload = chat_payload(&history, args.max_tokens);
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

        let mut reply = String::new();
        while let Some(result) = stream.recv().await {
            if let CapabilityChunk::Text(chunk) = result? {
                out.raw(&chunk);
                reply.push_str(&chunk);
            }
        }
        if out.is_json() {
            out.json(&serde_json::json!({ "role": "assistant", "content": reply }));
        } else {
            out.raw("\n");
        }
        history.push(message("assistant", &reply));
    }
    Ok(())
}

/// A chat payload carrying the running `history` and an optional token cap.
fn chat_payload(history: &[JsonValue], max_tokens: Option<i64>) -> JsonValue {
    let mut payload = BTreeMap::new();
    payload.insert("messages".to_owned(), JsonValue::Array(history.to_vec()));
    if let Some(max_tokens) = max_tokens {
        payload.insert("max_tokens".to_owned(), JsonValue::Int(max_tokens));
    }
    JsonValue::Object(payload)
}
