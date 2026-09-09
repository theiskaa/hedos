//! Starting a bench: which models get a row, what the machine is called, and
//! clearing a model from memory so its first run is genuinely cold.
//!
//! Both `hedos bench` and the shelf's bench screen go through here, so the two
//! surfaces bench the same models under the same rules.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use kernel::bench::Row;
use kernel::profiles::FitVerdict;
use kernel::records::{Capability, ModelRecord, ModelState};
use runtime::bench::{EvictFuture, Eviction, Prepare};

use crate::support::residency::{self, Holder};
use crate::support::session::Session;
use crate::support::shelf_table::verdict;
use crate::support::text;

/// A row for every chat model on `shelf`, in shelf order, judged against
/// `budget` bytes of memory. `any_size` benches the ones too big for this
/// machine as well.
pub(crate) fn rows(shelf: &[ModelRecord], budget: u64, any_size: bool) -> Vec<Row> {
    shelf
        .iter()
        .filter(|record| record.capabilities.contains(&Capability::chat()))
        .map(|record| row_for(record, budget, any_size))
        .collect()
}

/// One model's row: queued, or skipped with the reason it cannot be measured.
/// A skipped model keeps its row rather than being dropped, so the table says
/// what it did not measure and why.
pub(crate) fn row_for(record: &ModelRecord, budget: u64, any_size: bool) -> Row {
    let runtime = record
        .runtime
        .id
        .as_ref()
        .map(|id| text::short_runtime(id.as_str()).to_owned());
    let quantization = record.quantization.clone();
    match skip_reason(record, budget, any_size) {
        Some(reason) => Row::skipped(
            record.id.clone(),
            record.display_name(),
            runtime,
            quantization,
            reason,
        ),
        None => Row::waiting(
            record.id.clone(),
            record.display_name(),
            runtime,
            quantization,
        ),
    }
}

/// Why a model will not be benched, when it will not be.
fn skip_reason(record: &ModelRecord, budget: u64, any_size: bool) -> Option<String> {
    if record.state == ModelState::Missing {
        return Some("weights are gone".to_owned());
    }
    if record.downloading {
        return Some("still downloading".to_owned());
    }
    if record.runtime.id.is_none() {
        return Some("no runtime serves it".to_owned());
    }
    if !any_size && verdict(record.footprint_bytes, budget) == Some(FitVerdict::TooLarge) {
        return Some(format!("too big for {} GiB", text::gib(budget as i64)));
    }
    None
}

/// `apple m3 max · 36 GiB`, or just the memory where the chip cannot be read.
pub(crate) fn machine_line(memory_bytes: i64) -> String {
    let memory = format!("{} GiB", text::gib(memory_bytes));
    match runtime::chip::name() {
        Some(chip) => format!("{} · {memory}", chip.to_lowercase()),
        None => memory,
    }
}

/// Clearing a model from wherever it is loaded, for the driver's cold run.
pub(crate) struct ShelfPrepare {
    session: Arc<Session>,
    records: BTreeMap<String, ModelRecord>,
}

impl ShelfPrepare {
    /// A preparer over the records of `shelf` the bench will touch.
    pub(crate) fn over(session: &Arc<Session>, shelf: &[ModelRecord], ids: &[String]) -> Self {
        let wanted: HashSet<&str> = ids.iter().map(String::as_str).collect();
        let records = shelf
            .iter()
            .filter(|record| wanted.contains(record.id.as_str()))
            .map(|record| (record.id.clone(), record.clone()))
            .collect();
        Self {
            session: Arc::clone(session),
            records,
        }
    }
}

impl Prepare for ShelfPrepare {
    fn evict(&self, model_id: &str) -> EvictFuture {
        let session = Arc::clone(&self.session);
        let record = self.records.get(model_id).cloned();
        Box::pin(async move {
            let Some(record) = record else {
                return Eviction::Cleared;
            };
            // A gateway holds its models in a process of its own, which nothing
            // here can evict; saying so is more use than a cold figure that is
            // not one.
            let holder = residency::loaded(&session, std::slice::from_ref(&record))
                .await
                .residents
                .into_iter()
                .find(|resident| resident.id == record.id)
                .map(|resident| resident.holder);
            if holder == Some(Holder::Gateway) {
                return Eviction::Held("a running gateway".to_owned());
            }
            match residency::unload_anywhere(&session, &record).await {
                Ok(false) => Eviction::Cleared,
                Ok(true) => Eviction::Held("still resident".to_owned()),
                Err(error) => Eviction::Held(error.message),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel::bench::Status;
    use kernel::records::{Modality, ModelSource, RuntimeId, SourceKind};

    const GIB: i64 = 1 << 30;

    fn record(name: &str, footprint: i64) -> ModelRecord {
        let mut record = ModelRecord::new(
            name,
            Modality::text(),
            vec![Capability::chat()],
            ModelSource::new(SourceKind::ollama(), name),
        );
        record.runtime.id = Some(RuntimeId::ollama());
        record.footprint_bytes = Some(footprint);
        record
    }

    fn reason(row: &Row) -> Option<String> {
        match &row.status {
            Status::Skipped(reason) => Some(reason.clone()),
            _ => None,
        }
    }

    #[test]
    fn a_model_that_fits_is_queued_and_one_that_does_not_says_so() {
        let budget = 16 * GIB as u64;
        assert_eq!(
            row_for(&record("small", GIB), budget, false).status,
            Status::Waiting
        );
        assert!(
            reason(&row_for(&record("huge", 32 * GIB), budget, false))
                .is_some_and(|reason| reason.starts_with("too big"))
        );
        assert_eq!(
            row_for(&record("huge", 32 * GIB), budget, true).status,
            Status::Waiting,
            "unless the caller asked for every size"
        );
    }

    #[test]
    fn a_model_that_cannot_run_names_what_stops_it() {
        let budget = 16 * GIB as u64;
        let mut gone = record("gone", GIB);
        gone.state = ModelState::Missing;
        assert_eq!(
            reason(&row_for(&gone, budget, true)).as_deref(),
            Some("weights are gone")
        );

        let mut downloading = record("coming", GIB);
        downloading.downloading = true;
        assert_eq!(
            reason(&row_for(&downloading, budget, true)).as_deref(),
            Some("still downloading")
        );

        let mut unresolved = record("orphan", GIB);
        unresolved.runtime.id = None;
        assert_eq!(
            reason(&row_for(&unresolved, budget, true)).as_deref(),
            Some("no runtime serves it")
        );
    }

    #[test]
    fn only_chat_models_get_a_row() {
        let mut speaker = record("kokoro", GIB);
        speaker.capabilities = vec![Capability::speak()];
        let shelf = vec![record("chatty", GIB), speaker];
        let rows = rows(&shelf, 16 * GIB as u64, true);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "chatty");
    }
}
