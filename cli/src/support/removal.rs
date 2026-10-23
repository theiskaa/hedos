//! Deleting a model the same way from `hedos rm` and the UI.

use kernel::records::ModelRecord;
use kernel::removal::{ModelDeletionPreview, ModelDeletionReport};
use runtime::removal::{ModelRemover, OllamaModelRemover, permanent_delete_trasher};

use crate::error::CliError;
use crate::support::session::Session;

/// What deleting `record` would do, as [`remove_and_forget`] would do it.
pub(crate) fn deletion_preview(record: &ModelRecord) -> ModelDeletionPreview {
    remover().preview(record)
}

fn remover() -> ModelRemover {
    ModelRemover::new(permanent_delete_trasher(), OllamaModelRemover::new())
}

/// Delete `record`'s weights (or its Ollama tag), then forget the record, or
/// it lingers in the registry and `hedos ls` keeps showing the deleted model,
/// and resolve the rest again so a model that needed it as a companion says so.
pub(crate) async fn remove_and_forget(
    session: &Session,
    record: &ModelRecord,
) -> Result<ModelDeletionReport, CliError> {
    let report = remover().remove(record).await?;
    session.kernel.forget(&report.model_id).await?;
    session.kernel.resolve().await?;
    Ok(report)
}
