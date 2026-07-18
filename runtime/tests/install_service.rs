//! Tests for the install orchestrator, driven by scriptable mock providers.

use std::sync::Arc;

use kernel::install::{
    InstallAvailability, InstallError, InstallEvent, InstallPlan, InstallProgress,
    InstallProviderId, InstallSearchHit, InstallStreamEvent,
};
use kernel::records::SourceKind;
use runtime::install::InstallService;
use runtime::install::provider::{InstallEventStream, InstallFuture, InstallProvider};
use tokio::sync::mpsc;

#[derive(Clone)]
enum Behavior {
    /// Emit the events, then finish cleanly (→ Done).
    Events(Vec<InstallStreamEvent>),
    /// Emit the events, then fail.
    Fail(Vec<InstallStreamEvent>, String),
    /// Emit the events, then hold the stream open until cancelled.
    Hang(Vec<InstallStreamEvent>),
}

struct MockProvider {
    id: InstallProviderId,
    source_kind: SourceKind,
    availability: InstallAvailability,
    supports_search: bool,
    hits: Vec<InstallSearchHit>,
    behavior: Behavior,
}

impl MockProvider {
    fn hf(behavior: Behavior) -> Arc<Self> {
        Arc::new(Self {
            id: InstallProviderId::huggingface(),
            source_kind: SourceKind::huggingface_cache(),
            availability: InstallAvailability::Ready,
            supports_search: true,
            hits: vec![hit("org/Model")],
            behavior,
        })
    }
}

fn hit(reference: &str) -> InstallSearchHit {
    InstallSearchHit {
        provider: InstallProviderId::huggingface(),
        reference: reference.to_owned(),
        name: reference.rsplit('/').next().unwrap_or(reference).to_owned(),
        downloads: None,
        likes: None,
        updated_at: None,
    }
}

fn plan() -> InstallPlan {
    let mut plan = InstallPlan::new(
        InstallProviderId::huggingface(),
        "org/Model",
        "Model",
        "~/.cache/hf",
    );
    plan.total_bytes = Some(1000);
    plan.remaining_bytes = Some(1000);
    plan
}

impl InstallProvider for MockProvider {
    fn id(&self) -> InstallProviderId {
        self.id.clone()
    }
    fn display_name(&self) -> &str {
        "Mock"
    }
    fn source_kind(&self) -> SourceKind {
        self.source_kind.clone()
    }
    fn supports_search(&self) -> bool {
        self.supports_search
    }
    fn availability(&self) -> InstallFuture<'_, InstallAvailability> {
        let availability = self.availability.clone();
        Box::pin(async move { availability })
    }
    fn search(
        &self,
        _query: &str,
        _limit: usize,
    ) -> InstallFuture<'_, Result<Vec<InstallSearchHit>, InstallError>> {
        let hits = self.hits.clone();
        Box::pin(async move { Ok(hits) })
    }
    fn plan(&self, _reference: &str) -> InstallFuture<'_, Result<InstallPlan, InstallError>> {
        Box::pin(async move { Ok(plan()) })
    }
    fn install(&self, _plan: InstallPlan) -> InstallEventStream {
        let (tx, rx) = mpsc::channel(16);
        let behavior = self.behavior.clone();
        tokio::spawn(async move {
            let events = match &behavior {
                Behavior::Events(events) | Behavior::Fail(events, _) | Behavior::Hang(events) => {
                    events.clone()
                }
            };
            for event in events {
                if tx.send(Ok(event)).await.is_err() {
                    return;
                }
            }
            match behavior {
                Behavior::Events(_) => {} // drop tx → clean finish
                Behavior::Fail(_, message) => {
                    let _ = tx.send(Err(InstallError::TransferFailed(message))).await;
                }
                Behavior::Hang(_) => {
                    // Stay open until the consumer (the service) drops the receiver.
                    tx.closed().await;
                }
            }
        });
        rx
    }
}

fn service(provider: Arc<dyn InstallProvider>) -> InstallService {
    InstallService::builder(vec![provider])
        .clock(|| 100)
        .build()
}

async fn drain(mut feed: runtime::install::InstallEventFeed) -> Vec<InstallEvent> {
    let mut events = Vec::new();
    while let Some(event) = feed.recv().await {
        events.push(event);
    }
    events
}

#[tokio::test]
async fn a_full_install_runs_the_lifecycle_to_done() {
    let provider = MockProvider::hf(Behavior::Events(vec![
        InstallStreamEvent::Status("Resolving".to_owned()),
        InstallStreamEvent::Progress(InstallProgress {
            bytes_downloaded: 500,
            total_bytes: Some(1000),
            total_is_partial: false,
            current_file: Some("w.gguf".to_owned()),
        }),
    ]));
    let service = service(provider);
    let id = service.begin(plan()).expect("begin");
    let events = drain(service.events(&id)).await;

    assert!(events.contains(&InstallEvent::Preparing));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, InstallEvent::Status(m) if m == "Resolving"))
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, InstallEvent::Progress(_)))
    );
    assert_eq!(events.last(), Some(&InstallEvent::Done));
}

#[tokio::test]
async fn the_same_reference_in_flight_is_deduplicated() {
    let service = service(MockProvider::hf(Behavior::Hang(vec![])));
    let first = service.begin(plan()).expect("begin");
    let second = service.begin(plan()).expect("begin again");
    assert_eq!(first, second);
    assert_eq!(service.active().len(), 1);
    service.cancel(&first);
}

#[tokio::test]
async fn a_failing_install_ends_failed() {
    let provider = MockProvider::hf(Behavior::Fail(vec![], "disk exploded".to_owned()));
    let service = service(provider);
    let id = service.begin(plan()).expect("begin");
    let events = drain(service.events(&id)).await;
    match events.last() {
        Some(InstallEvent::Failed { message }) => assert!(message.contains("disk exploded")),
        other => panic!("expected Failed, got {other:?}"),
    }
}

#[tokio::test]
async fn cancelling_ends_the_install_cancelled() {
    let service = service(MockProvider::hf(Behavior::Hang(vec![
        InstallStreamEvent::Status("working".to_owned()),
    ])));
    let id = service.begin(plan()).expect("begin");
    let feed = service.events(&id);
    service.cancel(&id);
    let events = drain(feed).await;
    assert_eq!(events.last(), Some(&InstallEvent::Cancelled));
}

#[tokio::test]
async fn a_completion_is_announced_with_the_source_kind() {
    let service = service(MockProvider::hf(Behavior::Events(vec![])));
    let mut completions = service.completions();
    let id = service.begin(plan()).expect("begin");
    // Drain the install to completion first.
    drain(service.events(&id)).await;
    let kinds = completions.recv().await.expect("a completion");
    assert!(kinds.contains(&SourceKind::huggingface_cache()));
}

#[tokio::test]
async fn a_concluded_install_replays_its_terminal_event() {
    let service = service(MockProvider::hf(Behavior::Events(vec![])));
    let id = service.begin(plan()).expect("begin");
    drain(service.events(&id)).await; // run to Done
    // A late subscriber still sees the terminal event, then a clean close.
    let replay = drain(service.events(&id)).await;
    assert_eq!(replay, vec![InstallEvent::Done]);
}

#[tokio::test]
async fn an_over_budget_disk_check_rejects_begin() {
    let service = InstallService::builder(vec![MockProvider::hf(Behavior::Events(vec![]))])
        .clock(|| 100)
        .disk_probe("/", |_| Some(10)) // only 10 bytes free
        .build();
    match service.begin(plan()) {
        Err(InstallError::InsufficientDisk {
            available_bytes, ..
        }) => assert_eq!(available_bytes, 10),
        other => panic!("expected InsufficientDisk, got {other:?}"),
    }
}

#[tokio::test]
async fn dedup_wins_over_a_failing_disk_check() {
    // A hanging install already in flight; a failing probe would reject a *new*
    // install, but the duplicate must dedup to the existing id without re-probing.
    let service = InstallService::builder(vec![MockProvider::hf(Behavior::Hang(vec![]))])
        .clock(|| 100)
        .disk_probe("/", |_| Some(1)) // 1 byte free: any sized plan fails the check
        .build();
    // The first install has no known size, so it skips the disk check and starts.
    let mut first_plan = plan();
    first_plan.total_bytes = None;
    first_plan.remaining_bytes = None;
    let first = service.begin(first_plan).expect("first begin");
    // The second begin (same reference) carries a size that WOULD fail the probe,
    // but it dedups to the running install before the disk check runs.
    let second = service.begin(plan()).expect("dedup begin");
    assert_eq!(first, second);
    service.cancel(&first);
}

#[tokio::test]
async fn a_mid_flight_subscriber_replays_the_latest_progress() {
    let service = service(MockProvider::hf(Behavior::Hang(vec![
        InstallStreamEvent::Progress(InstallProgress {
            bytes_downloaded: 256,
            total_bytes: Some(1000),
            total_is_partial: false,
            current_file: None,
        }),
    ])));
    let id = service.begin(plan()).expect("begin");
    // Let the install emit its progress before we subscribe.
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    let mut feed = service.events(&id);
    // The replay includes the current phase then the latest progress.
    let mut saw_progress = false;
    while let Ok(Some(event)) =
        tokio::time::timeout(std::time::Duration::from_millis(50), feed.recv()).await
    {
        if let InstallEvent::Progress(progress) = event {
            assert_eq!(progress.bytes_downloaded, 256);
            saw_progress = true;
            break;
        }
    }
    assert!(
        saw_progress,
        "a mid-flight subscriber should replay progress"
    );
    service.cancel(&id);
}

#[tokio::test]
async fn a_direct_ollama_tag_query_short_circuits_browse() {
    let service = service(MockProvider::hf(Behavior::Events(vec![])));
    // A bare ollama tag isn't an HF browse — it returns empty.
    let result = service.browse("gemma3:4b", 10).await;
    assert!(result.hits.is_empty());
    assert!(result.failure_hint.is_none());
}

#[tokio::test]
async fn an_unavailable_provider_fails_search() {
    let provider = Arc::new(MockProvider {
        id: InstallProviderId::huggingface(),
        source_kind: SourceKind::huggingface_cache(),
        availability: InstallAvailability::Unavailable {
            hint: "offline".to_owned(),
        },
        supports_search: true,
        hits: vec![],
        behavior: Behavior::Events(vec![]),
    });
    let service = InstallService::new(vec![provider]);
    match service
        .search(&InstallProviderId::huggingface(), "q", 10)
        .await
    {
        Err(InstallError::ProviderUnavailable(hint)) => assert_eq!(hint, "offline"),
        other => panic!("expected ProviderUnavailable, got {other:?}"),
    }
}

#[tokio::test]
async fn providers_lists_statuses_in_order() {
    let service = service(MockProvider::hf(Behavior::Events(vec![])));
    let statuses = service.providers().await;
    assert_eq!(statuses.len(), 1);
    assert_eq!(statuses[0].id, InstallProviderId::huggingface());
    assert_eq!(statuses[0].availability, InstallAvailability::Ready);
}

#[tokio::test]
async fn browse_surfaces_a_repo_shaped_query_as_an_exact_hit() {
    // The provider returns an unrelated hit; the exact repo is prepended.
    let provider = Arc::new(MockProvider {
        id: InstallProviderId::huggingface(),
        source_kind: SourceKind::huggingface_cache(),
        availability: InstallAvailability::Ready,
        supports_search: true,
        hits: vec![hit("someone/else")],
        behavior: Behavior::Events(vec![]),
    });
    let service = InstallService::new(vec![provider]);
    let result = service.browse("huggingface.co/org/Model", 10).await;
    assert_eq!(
        result.hits.first().map(|h| h.reference.as_str()),
        Some("org/Model")
    );
    assert!(result.failure_hint.is_none());
}

#[tokio::test]
async fn begin_rejects_an_unknown_provider() {
    let service = service(MockProvider::hf(Behavior::Events(vec![])));
    let mut plan = plan();
    plan.provider = InstallProviderId::from("nope");
    match service.begin(plan) {
        Err(InstallError::ProviderUnknown(_)) => {}
        other => panic!("expected ProviderUnknown, got {other:?}"),
    }
}
