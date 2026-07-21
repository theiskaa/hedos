//! `hedos ls` — list the models on the shelf, with their runtime, store,
//! capabilities, and whether each is currently warm.

use clap::Args;
use kernel::profiles::FitVerdict;
use kernel::records::{Capability, ModelRecord};

use crate::error::CliError;
use crate::support::machine;
use crate::support::output::Out;
use crate::support::session::Session;
use crate::support::shelf_table;

/// Arguments for `ls`.
#[derive(Args)]
pub struct LsArgs {
    /// Rescan the machine's model stores before listing.
    #[arg(long)]
    scan: bool,
    /// Only show models serving this capability (chat, complete, embed, …).
    #[arg(long)]
    capability: Option<String>,
}

/// Run the `ls` command.
pub async fn run(args: LsArgs, out: &Out) -> Result<(), CliError> {
    let session = Session::open()?;
    // `retain` below needs an owned, mutable `Vec`; this is the one cold CLI
    // boundary where the shared shelf snapshot is unpacked into one.
    let mut shelf = if args.scan {
        session.discover().await?;
        session.shelf().await.to_vec()
    } else {
        session.shelf_or_discover().await?.to_vec()
    };
    if let Some(name) = &args.capability {
        let capability = Capability::from(name.as_str());
        shelf.retain(|record| record.capabilities.contains(&capability));
    }

    let budget = machine::memory_budget_bytes();
    if out.is_json() {
        out.json(&shelf_json(&shelf, budget));
        return Ok(());
    }

    if shelf.is_empty() {
        out.line("No models on the shelf — install one with `hedos pull <ref>`.");
        return Ok(());
    }

    let warm = session.warm_set();
    let records: Vec<&_> = shelf.iter().collect();
    out.line(&shelf_table::table(&records, &warm, budget));
    Ok(())
}

/// The shelf as a JSON array: every record's own fields plus a `fit` slug
/// (`runs_well`/`tight_fit`/`too_large`, or `null` when the footprint is unknown),
/// judged against `total_memory_bytes`.
fn shelf_json(shelf: &[ModelRecord], total_memory_bytes: u64) -> serde_json::Value {
    let models: Vec<serde_json::Value> = shelf
        .iter()
        .map(|record| {
            let mut value = serde_json::to_value(record).unwrap_or_default();
            let fit = FitVerdict::assess(record.footprint_mb, total_memory_bytes)
                .map(|assessment| assessment.verdict.as_str());
            if let Some(object) = value.as_object_mut() {
                // serde_json maps `None` to JSON `null`, `Some(slug)` to a string.
                object.insert("fit".to_owned(), fit.into());
            }
            value
        })
        .collect();
    serde_json::Value::Array(models)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel::records::{Modality, ModelSource, SourceKind};

    const GIB: u64 = 1 << 30;

    fn model(name: &str, footprint_mb: Option<i64>) -> ModelRecord {
        let mut record = ModelRecord::new(
            name,
            Modality::text(),
            vec![Capability::chat()],
            ModelSource::new(SourceKind::ollama(), name),
        );
        record.footprint_mb = footprint_mb;
        record
    }

    #[test]
    fn json_injects_a_fit_slug_and_keeps_every_record_field() {
        let record = model("gemma", Some(1024));
        let value = shelf_json(std::slice::from_ref(&record), 16 * GIB);
        let enriched = value[0].as_object().expect("record object");
        assert_eq!(
            enriched.get("fit").and_then(|fit| fit.as_str()),
            Some("runs_well")
        );

        // Every field of the plain serialization survives unchanged alongside the
        // new key — value equality also guards against a future `fit` field being
        // clobbered.
        let plain = serde_json::to_value(&record).unwrap();
        for (key, value) in plain.as_object().unwrap() {
            assert_eq!(
                enriched.get(key),
                Some(value),
                "field {key} changed or dropped"
            );
        }
    }

    #[test]
    fn json_fit_is_null_when_the_footprint_is_unknown() {
        let value = shelf_json(&[model("mystery", None)], 16 * GIB);
        assert!(value[0].get("fit").expect("fit key present").is_null());
    }
}
