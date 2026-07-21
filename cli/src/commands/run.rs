//! `hedos run <model> <prompt>` — stream a single completion to stdout.

use std::path::{Path, PathBuf};

use clap::Args;
use kernel::capabilities::{AttachmentKind, CapabilityChunk, ChatAttachment};
use kernel::records::{Capability, JsonValue};
use serde_json::json;

use crate::error::CliError;
use crate::support::interactive;
use crate::support::output::Out;
use crate::support::payload;
use crate::support::session::Session;
use crate::support::spinner::Spinner;

/// Arguments for `run`.
#[derive(Args)]
pub struct RunArgs {
    /// The model to run (name, alias, or id). Omit to pick one interactively.
    model: Option<String>,
    /// The prompt to complete. Omit to type it interactively.
    prompt: Option<String>,
    /// A system prompt for this run.
    #[arg(long)]
    system: Option<String>,
    /// Cap the number of generated tokens.
    #[arg(long)]
    max_tokens: Option<i64>,
    /// Sampling temperature.
    #[arg(long)]
    temperature: Option<f64>,
    /// Attach a local image for a vision (`see`) model to read. Repeatable.
    #[arg(long = "image", value_name = "PATH")]
    images: Vec<PathBuf>,
}

/// Run the `run` command.
pub async fn run(args: RunArgs, out: &Out) -> Result<(), CliError> {
    let session = Session::open()?;
    let shelf = session.shelf_or_discover().await?;
    let warm = session.warm_set();

    // A named model resolves against the whole shelf, then an image run checks
    // `see` explicitly — so a named non-vision model gets a precise reason instead
    // of being silently swapped for a look-alike that can see. With no model named,
    // an image run offers only vision models in the picker.
    let pick = if args.model.is_some() || args.images.is_empty() {
        Capability::chat()
    } else {
        Capability::see()
    };
    let record = interactive::choose_model(
        out,
        args.model.as_deref(),
        &shelf,
        Some(&pick),
        "run",
        &warm,
    )?;
    if !args.images.is_empty() && !record.capabilities.contains(&Capability::see()) {
        return Err(CliError::new(format!(
            "{} cannot read images — it has no `see` capability; pick a vision model with `hedos ls`",
            record.display_name()
        )));
    }

    let attachments = read_images(&args.images)?;
    let prompt = interactive::text_or_prompt(out, args.prompt, "prompt")?;
    let payload = chat_payload(&prompt, args.max_tokens, args.temperature, attachments);
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
    let mut spinner = Spinner::start(out);
    while let Some(result) = stream.recv().await {
        match result? {
            CapabilityChunk::Text(chunk) => {
                // Clear the spinner before the reply starts, so they don't collide.
                spinner.clear();
                out.raw(&chunk);
                text.push_str(&chunk);
            }
            CapabilityChunk::Status(status) => spinner.set(&status),
            _ => {}
        }
    }
    spinner.clear();

    if out.is_json() {
        out.json(&json!({ "model": record.id, "text": text }));
    } else {
        // Terminate the streamed line.
        out.raw("\n");
    }
    Ok(())
}

/// A one-user-turn chat payload with optional sampling knobs and image attachments.
fn chat_payload(
    prompt: &str,
    max_tokens: Option<i64>,
    temperature: Option<f64>,
    images: Vec<ChatAttachment>,
) -> JsonValue {
    // The kernel builder renders a zero-attachment message as a plain
    // `{role, content}`, so this one call covers the text and image cases alike.
    let user = payload::user_message_with_images(prompt, images);
    let mut payload = payload::chat(vec![user], max_tokens);
    if let Some(temperature) = temperature {
        payload.insert("temperature".to_owned(), JsonValue::Double(temperature));
    }
    JsonValue::Object(payload)
}

/// Read each `--image` path into an image attachment, failing fast and naming the
/// offending path if one cannot be read.
fn read_images(paths: &[PathBuf]) -> Result<Vec<ChatAttachment>, CliError> {
    paths
        .iter()
        .map(|path| {
            let data = std::fs::read(path).map_err(|error| {
                CliError::new(format!("cannot read image {}: {error}", path.display()))
            })?;
            Ok(ChatAttachment {
                kind: AttachmentKind::Image,
                data,
                mime_type: image_mime(path).to_owned(),
                name: path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(str::to_owned),
            })
        })
        .collect()
}

/// The MIME type for an image path from its extension, defaulting to `image/png`
/// for an unknown or absent one. The chat wire carries only the raw base64 bytes,
/// so this is inert there, but `ChatAttachment` records it.
fn image_mime(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        _ => "image/png",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_mime_maps_known_extensions_and_defaults_to_png() {
        assert_eq!(image_mime(Path::new("a.jpg")), "image/jpeg");
        assert_eq!(image_mime(Path::new("a.JPEG")), "image/jpeg");
        assert_eq!(image_mime(Path::new("a.webp")), "image/webp");
        assert_eq!(image_mime(Path::new("a.gif")), "image/gif");
        assert_eq!(image_mime(Path::new("a.png")), "image/png");
        assert_eq!(image_mime(Path::new("photo.bin")), "image/png");
        assert_eq!(image_mime(Path::new("noext")), "image/png");
    }

    fn messages(payload: &JsonValue) -> &[JsonValue] {
        payload
            .as_object()
            .and_then(|object| object.get("messages"))
            .and_then(JsonValue::as_array)
            .expect("a messages array")
    }

    #[test]
    fn chat_payload_puts_the_images_on_the_last_message() {
        let image = ChatAttachment {
            kind: AttachmentKind::Image,
            data: b"hi".to_vec(),
            mime_type: "image/png".to_owned(),
            name: None,
        };
        let payload = chat_payload("what is this?", None, None, vec![image]);
        // The facade's vision guard only inspects the last message's `images`, so
        // the attachment must land there.
        let last = messages(&payload)
            .last()
            .and_then(JsonValue::as_object)
            .expect("a message object");
        let images = last
            .get("images")
            .and_then(JsonValue::as_array)
            .expect("an images array");
        assert_eq!(images.len(), 1);
    }

    #[test]
    fn a_plain_run_carries_no_images_key() {
        let payload = chat_payload("hello", None, None, Vec::new());
        let last = messages(&payload)
            .last()
            .and_then(JsonValue::as_object)
            .expect("a message object");
        assert!(last.get("images").is_none());
    }
}
