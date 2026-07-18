//! The install-provider trait: a backend that can search for, plan, and run the
//! installation of a model. Async methods return boxed futures (no `async-trait`
//! dep); a running install streams events through a channel.

use std::future::Future;
use std::pin::Pin;

use kernel::install::{
    InstallAvailability, InstallError, InstallPlan, InstallProviderId, InstallSearchHit,
    InstallStreamEvent,
};
use kernel::records::SourceKind;
use tokio::sync::mpsc;

/// A boxed future returned by a provider's async methods, borrowing the provider
/// for the duration of the call.
pub type InstallFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The stream a running install emits: status/progress events until it ends —
/// a terminal `Err`, or the sender finishing (dropping) once the install succeeds.
pub type InstallEventStream = mpsc::Receiver<Result<InstallStreamEvent, InstallError>>;

/// A provider's advertised identity and current readiness.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct InstallProviderStatus {
    /// The provider's stable id.
    pub id: InstallProviderId,
    /// The name to show the user.
    pub display_name: String,
    /// The source kind installs from this provider are tagged with.
    pub source_kind: SourceKind,
    /// Whether the provider supports free-text search.
    pub supports_search: bool,
    /// Whether the provider can run right now.
    pub availability: InstallAvailability,
}

/// A backend that installs models — a Hugging Face repo or an Ollama tag.
pub trait InstallProvider: Send + Sync {
    /// The provider's stable id.
    fn id(&self) -> InstallProviderId;

    /// The name to show the user.
    fn display_name(&self) -> &str;

    /// The source kind installs from this provider are tagged with.
    fn source_kind(&self) -> SourceKind;

    /// Whether the provider supports free-text search.
    fn supports_search(&self) -> bool;

    /// Whether the provider can install right now (daemon present, reachable, …).
    fn availability(&self) -> InstallFuture<'_, InstallAvailability>;

    /// Search for models matching `query`, up to `limit` hits.
    fn search(
        &self,
        query: &str,
        limit: usize,
    ) -> InstallFuture<'_, Result<Vec<InstallSearchHit>, InstallError>>;

    /// Resolve `reference` (a repo or tag) to an install plan.
    fn plan(&self, reference: &str) -> InstallFuture<'_, Result<InstallPlan, InstallError>>;

    /// Start installing `plan`, returning a stream of progress events. Dropping
    /// the returned receiver cancels the install.
    fn install(&self, plan: InstallPlan) -> InstallEventStream;

    /// This provider's identity plus its current availability.
    fn status(&self) -> InstallFuture<'_, InstallProviderStatus> {
        Box::pin(async move {
            InstallProviderStatus {
                id: self.id(),
                display_name: self.display_name().to_owned(),
                source_kind: self.source_kind(),
                supports_search: self.supports_search(),
                availability: self.availability().await,
            }
        })
    }
}
