//! `hedos chat <model>` — an interactive headless chat that reads turns from
//! stdin and streams replies to stdout.

use std::io::{BufRead, IsTerminal, Write};

use clap::Args;
use kernel::capabilities::CapabilityChunk;
use kernel::records::{Capability, JsonValue, ModelRecord};

use crate::error::CliError;
use crate::support::interactive;
use crate::support::output::Out;
use crate::support::payload::{self, message};
use crate::support::session::Session;
use crate::support::signals;

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
    let warm = session.warm_set_anywhere(&shelf).await;
    let record = interactive::choose_model(
        out,
        args.model.as_deref(),
        &shelf,
        Some(&Capability::chat()),
        "chat with",
        &warm,
    )?;

    chat(
        &session,
        record,
        args.system.as_deref(),
        args.max_tokens,
        out,
    )
    .await
}

/// Chat with `record` on stdin/stdout until end-of-input. Shared by the
/// command and `hedos shelf`.
pub(crate) async fn chat(
    session: &Session,
    record: &ModelRecord,
    system: Option<&str>,
    max_tokens: Option<i64>,
    out: &Out,
) -> Result<(), CliError> {
    let tty = std::io::stdin().is_terminal() && !out.is_json();
    if tty {
        out.err(&format!(
            "chatting with {} — Ctrl-C stops a reply, Ctrl-D ends",
            record.display_name()
        ));
    }

    let mut history: Vec<JsonValue> = Vec::new();
    loop {
        if tty {
            eprint!("› ");
            let _ = std::io::stderr().flush();
        }
        let Some(line) = read_line().await? else {
            break; // Ctrl-D
        };
        let prompt = line.trim_end();
        if prompt.is_empty() {
            continue;
        }

        history.push(message("user", prompt));
        let payload = chat_payload(&history, max_tokens);
        let mut stream = session
            .kernel
            .invoke_with(&record.id, Capability::chat(), payload, system, None)
            .await?;

        let mut reply = String::new();
        loop {
            tokio::select! {
                received = stream.recv() => match received {
                    Some(result) => {
                        if let CapabilityChunk::Text(chunk) = result? {
                            out.raw(&chunk);
                            reply.push_str(&chunk);
                        }
                    }
                    None => break,
                },
                // Ctrl-C cuts the reply short and returns to the prompt.
                () = signals::wait_for_ctrl_c() => break,
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

/// One line from stdin, or `None` at end of input; read on a blocking thread
/// so a waiting prompt never holds a runtime worker.
async fn read_line() -> Result<Option<String>, CliError> {
    let line = tokio::task::spawn_blocking(|| {
        let mut line = String::new();
        std::io::stdin()
            .lock()
            .read_line(&mut line)
            .map(|read| (read > 0).then_some(line))
    })
    .await
    .map_err(|error| CliError::new(error.to_string()))??;
    Ok(line)
}

/// A chat payload carrying the running `history` and an optional token cap.
fn chat_payload(history: &[JsonValue], max_tokens: Option<i64>) -> JsonValue {
    JsonValue::Object(payload::chat(history.to_vec(), max_tokens))
}
