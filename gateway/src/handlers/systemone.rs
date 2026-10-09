//! The System One handler: TypeSafe's `POST /v1/systemone`, so a TypeSafe SDK
//! pointed at this gateway reaches a local model with no change on its side.
//!
//! The handler hands the question to whatever serves `judge` for the model
//! named: a llama.cpp decision server, or a manifest runtime such as laya's.
//! Each takes it as JSON in one user message and replies with the System One
//! envelope as text, which goes back as the body. There is no streamed form:
//! System One is one request and one envelope.
//!
//! **Images** go as data URLs in `images`, or as `image_url` parts of a state
//! made of chat messages, and only to a model that sees; any other is refused
//! with the judges that would read them.
//!
//! **Model routing.** The name in the body has to resolve, and to a model that
//! declares the `judge` capability. Anything else is refused with the names of
//! the models that would do, rather than answered by whatever else is on the
//! shelf: a chat model handed this question replies in prose, and a client that
//! asked for `jev-latest` and got a local model's opinion under that name would
//! have no way to tell. The SDK sends `jev-latest` unless told otherwise, so the
//! refusal is also what tells a caller to set `TYPESAFE_DEFAULT_MODEL`.
//!
//! **Errors** keep the OpenAI shape, `{"error": {"message": …}}`. The SDK reads
//! `error.message` out of a failed response, so it shows the text written here.

use kernel::capabilities::question_carries_images;
use kernel::records::Capability;

use super::named::{Wanted, listed, named};
use super::{GatewayHandling, HandlerFuture, bad_request, collect_completion, completion_id};
use crate::admission::GatewayWorkKind;
use crate::identity::{GatewayIdentity, GatewayOutcome};
use crate::port::{GatewayPort, require_admission};
use crate::request::GatewayRequest;
use crate::responder::GatewayResponder;
use crate::wire::typesafe;

/// The response header the TypeSafe SDK reads a request id from, to quote in
/// its errors and logs.
const REQUEST_ID_HEADER: &str = "x-typesafe-request-id";

/// What `/v1/systemone` asks of a model, and of one handed images.
fn judges(seeing: bool) -> Wanted {
    if seeing {
        Wanted {
            required: vec![Capability::judge(), Capability::see()],
            plural: "answer typed questions about images",
            singular: "answers typed questions about images",
        }
    } else {
        Wanted {
            required: vec![Capability::judge()],
            plural: "answer typed questions",
            singular: "answers typed questions",
        }
    }
}

/// The System One handler.
pub struct SystemOneHandler;

impl GatewayHandling for SystemOneHandler {
    fn handle<'a>(
        &'a self,
        request: &'a GatewayRequest,
        identity: &'a GatewayIdentity,
        port: &'a dyn GatewayPort,
        responder: &'a GatewayResponder,
    ) -> HandlerFuture<'a> {
        Box::pin(async move {
            let question = typesafe::decode_request(&request.body)?;
            let shelf = port.shelf().await;
            let record = named(&shelf, identity, &question.model, &judges(false))?;
            identity.require(&record.id, &Capability::judge())?;
            let text = question.question_text();
            if question_carries_images(&text) && !record.capabilities.contains(&Capability::see()) {
                return Err(bad_request(format!(
                    "{} does not read images; {}",
                    record.display_name(),
                    listed(&shelf, identity, &judges(true))
                )));
            }
            require_admission(port, &record, GatewayWorkKind::Stream).await?;

            let mut stream = port
                .invoke(
                    &record.id,
                    Capability::judge(),
                    typesafe::chat_payload(&text),
                )
                .await?;
            let (reply, _, _) = collect_completion(&mut stream).await?;
            responder.respond(
                200,
                "application/json",
                typesafe::envelope(&reply)?,
                vec![(REQUEST_ID_HEADER.to_owned(), completion_id("req-"))],
            );
            Ok(GatewayOutcome::ok_for(
                Some(&record.id),
                Some(&Capability::judge()),
            ))
        })
    }
}
