use super::*;

use kernel::discovery::service::KindStat;
use kernel::install::pulls::PullState;
use kernel::records::SourceKind;

use crate::support::pulls::testing::{TempDir, job as make_job};

/// Every event a control sent, in order.
fn sent(control: impl FnOnce(&mpsc::UnboundedSender<Event>)) -> Vec<Event> {
    let (tx, mut rx) = mpsc::unbounded_channel();
    control(&tx);
    drop(tx);
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }
    events
}

/// Whether any of `events` is a refusal.
fn refused(events: &[Event]) -> bool {
    events
        .iter()
        .any(|event| matches!(event, Event::PullRefused(_)))
}

/// The rows the last refresh among `events` carried.
fn last_rows(events: &[Event]) -> &[jobs::JobRow] {
    events
        .iter()
        .rev()
        .find_map(|event| match event {
            Event::Pulls(rows) => Some(rows.as_slice()),
            _ => None,
        })
        .expect("a refresh of the rows")
}

#[test]
fn a_scan_summary_speaks_the_strip_register() {
    let mut summary = DiscoverySummary {
        total_count: 12,
        issues: vec!["a".to_owned(), "b".to_owned()],
        ..DiscoverySummary::default()
    };
    summary.per_kind.insert(
        SourceKind::huggingface_cache(),
        KindStat { count: 9, bytes: 0 },
    );
    summary
        .per_kind
        .insert(SourceKind::ollama(), KindStat { count: 3, bytes: 0 });
    summary
        .per_kind
        .insert(SourceKind::lm_studio(), KindStat { count: 0, bytes: 0 });
    let line = scan_summary(&summary);
    assert!(line.starts_with("found 12 models · "));
    assert!(line.contains("3 ollama") && line.contains("9 hf"));
    assert!(line.ends_with(" · 2 issues"));
    assert!(!line.contains("lm studio"));
    assert!(!line.contains('\u{2014}') && !line.contains(", "));
    assert!(!line.ends_with('.'));
    assert!(line.chars().all(|c| !c.is_uppercase()));
    assert_eq!(scan_summary(&DiscoverySummary::default()), "found nothing");
    let one = DiscoverySummary {
        total_count: 1,
        ..DiscoverySummary::default()
    };
    assert_eq!(scan_summary(&one), "found 1 model");
}

#[test]
fn controlling_a_missing_job_refuses_and_refreshes_the_rows() {
    let directory = TempDir::new("control-missing");
    let store = directory.store();
    let events = sent(|tx| control_pull(&store, PullAction::Pause, "nope", tx));
    assert!(matches!(
        events.as_slice(),
        [Event::PullRefused(_), Event::Pulls(_)]
    ));
}

#[test]
fn forgetting_an_ended_job_takes_its_row_away() {
    let directory = TempDir::new("control-forget");
    let store = directory.store();
    let ended = make_job(&store, "Qwen/Qwen3-8B", 1_000);
    ended
        .update_status(2_000, |status| status.state = PullState::Done)
        .expect("settle the job");
    let kept = make_job(&store, "Qwen/Qwen3-4B", 1_500);
    let events = sent(|tx| control_pull(&store, PullAction::Forget, ended.id(), tx));
    assert!(!refused(&events));
    let rows = last_rows(&events);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].job, kept.id());
}

#[test]
fn pausing_a_job_with_no_worker_settles_it_paused() {
    let directory = TempDir::new("control-pause");
    let store = directory.store();
    let job = make_job(&store, "Qwen/Qwen3-8B", now_millis());
    let events = sent(|tx| control_pull(&store, PullAction::Pause, job.id(), tx));
    assert!(!refused(&events));
    assert_eq!(job.stored_status().state, PullState::Paused);
    assert_eq!(last_rows(&events)[0].pull_state, PullState::Paused);
}

#[test]
fn forgetting_a_live_job_is_refused_by_its_reference() {
    let directory = TempDir::new("control-forget-live");
    let store = directory.store();
    let job = make_job(&store, "Qwen/Qwen3-8B", 1_000);
    let _worker = job.claim().expect("claim").expect("the lock is free");
    job.update_status(1_000, |status| status.state = PullState::Running)
        .expect("write the record");
    let events = sent(|tx| control_pull(&store, PullAction::Forget, job.id(), tx));
    let reason = events
        .iter()
        .find_map(|event| match event {
            Event::PullRefused(reason) => Some(reason.as_str()),
            _ => None,
        })
        .expect("a refusal");
    assert_eq!(reason, "Qwen/Qwen3-8B is running, not ended");
    assert!(job.path().exists());
}

/// Hold the job's lock the way a running worker does.
fn running(job: &kernel::install::pulls::PullJobDir) -> kernel::install::pulls::PullLock {
    let lock = kernel::install::pulls::take_lock(&job.lock_path())
        .expect("take the lock")
        .expect("the lock is free");
    job.update_status(now_millis(), |status| {
        status.state = PullState::Running;
        status.pid = Some(std::process::id());
    })
    .expect("write the record");
    lock
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancel_landing_as_the_worker_honours_a_pause_ends_the_pull_cancelled() {
    let directory = TempDir::new("control-cancel-behind-pause");
    let store = directory.store();
    let job = make_job(&store, "Qwen/Qwen3-8B", 1_000);
    let worker = running(&job);
    // The worker has already cleared the pause it is honouring, so the cancel
    // is written after its last read: it settles paused, then lets go.
    let settling = job.clone();
    let honouring = std::thread::spawn(move || {
        while settling.control() != Some(PullControl::Cancel) {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        settling
            .update_status(now_millis(), |status| status.state = PullState::Paused)
            .expect("write the record");
        std::thread::sleep(std::time::Duration::from_millis(50));
        drop(worker);
    });

    let id = job.id().to_owned();
    let events = tokio::task::spawn_blocking(move || {
        sent(|tx| control_pull(&store, PullAction::Cancel, &id, tx))
    })
    .await
    .expect("the control ran");
    honouring.join().expect("the worker's side");

    assert!(!refused(&events));
    assert_eq!(job.status().state, PullState::Cancelled);
    assert_eq!(last_rows(&events)[0].pull_state, PullState::Cancelled);
}

#[test]
fn a_refused_stop_names_the_pull_it_refused() {
    let directory = TempDir::new("control-refusal-subject");
    let store = directory.store();
    let job = make_job(&store, "Qwen/Qwen3-8B", 1_000);
    let _worker = running(&job);
    job.request(PullControl::Cancel).expect("ask for a cancel");

    let events = sent(|tx| control_pull(&store, PullAction::Pause, job.id(), tx));
    let reason = events
        .iter()
        .find_map(|event| match event {
            Event::PullRefused(reason) => Some(reason.as_str()),
            _ => None,
        })
        .expect("a refusal");
    assert_eq!(
        reason,
        "Qwen/Qwen3-8B: a cancel is already waiting for it; a pause would change nothing"
    );
}
