//! `hedos warm <model>` — load a model into residency with a tiny request.

use clap::Args;

use crate::error::CliError;
use crate::support::interactive;
use crate::support::output::Out;
use crate::support::residency::{self, is_resident, residency_outcome, warm_request};
use crate::support::session::Session;

/// Arguments for `warm`.
#[derive(Args)]
pub struct WarmArgs {
    /// The model to warm (name, alias, or id). Omit to pick one interactively.
    model: Option<String>,
    /// The port of the gateway to warm, when it is not the configured one.
    #[arg(short, long)]
    port: Option<u16>,
}

/// The gateway to warm on: the one named, else whichever is answering on the
/// configured port. A named port is taken at its word rather than probed, so a
/// gateway that is slow to answer is still warmed rather than quietly skipped.
async fn live_gateway(session: &Session, named: Option<u16>) -> Option<u16> {
    match named {
        Some(port) => Some(port),
        None => session.live_gateway().await.map(|live| live.port),
    }
}

/// Run the `warm` command.
pub async fn run(args: WarmArgs, out: &Out) -> Result<(), CliError> {
    let session = Session::open()?;
    let shelf = session.shelf_or_discover().await?;
    let warm = session.warm_set_anywhere(&shelf).await;
    let record =
        interactive::choose_model(out, args.model.as_deref(), &shelf, None, "warm", &warm)?;

    // A model is warm where it is served. While a gateway is running that is the
    // gateway's own copy, and warming this process's would load the model here,
    // report success, and leave the gateway exactly as cold as it was. Only a
    // model the gateway has a warm request for goes that way: a speech or
    // prompt-shaped probe has no route on the conversation endpoint, and
    // failing it there would be refusing a model that warms perfectly well
    // here just because a gateway happens to be up.
    if let Some(live) = live_gateway(&session, args.port).await
        && residency::gateway_warm_body(record).is_some()
    {
        let outcome = residency::warm_via_gateway(record, live)
            .await
            .map_err(CliError::new)?;
        let resident = residency::held_by_gateway(live, &record.id).await;
        out.line(&format!("{} is {outcome}", record.display_name()));
        if !resident {
            out.err("the gateway served it but does not report it resident");
        }
        out.json(&serde_json::json!({
            "model": record.id,
            "resident": resident,
            "gatewayPort": live,
        }));
        return Ok(());
    }

    let (capability, payload) = warm_request(record)
        .ok_or_else(|| CliError::new(format!("{} can't be warmed", record.display_name())))?;
    let mut stream = session
        .kernel
        .invoke(&record.id, capability, payload)
        .await?;
    while let Some(result) = stream.recv().await {
        result?;
    }

    let resident = match is_resident(&session, record).await {
        Ok(resident) => resident,
        Err(reason) => {
            out.err(&reason);
            false
        }
    };
    out.line(&format!(
        "{} is {}",
        record.display_name(),
        residency_outcome(resident)
    ));
    out.json(&serde_json::json!({ "model": record.id, "resident": resident }));
    Ok(())
}
