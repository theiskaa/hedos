//! Deleting a model the same way from `hedos rm` and the UI.

use kernel::records::ModelRecord;
use kernel::removal::ModelDeletionReport;
use runtime::removal::{ModelRemover, OllamaModelRemover, permanent_delete_trasher};

use crate::error::CliError;
use crate::support::session::Session;

/// Delete `record`'s weights (or its Ollama tag), then forget the record, or
/// it lingers in the registry and `hedos ls` keeps showing the deleted model.
pub(crate) async fn remove_and_forget(
    session: &Session,
    record: &ModelRecord,
) -> Result<ModelDeletionReport, CliError> {
    let remover = ModelRemover::new(permanent_delete_trasher(), OllamaModelRemover::new());
    let report = remover.remove(record).await?;
    session.kernel.forget(&report.model_id).await?;
    Ok(report)
}
