//! Integration tests for the pull job directory: creation and layout, the
//! on-disk format, the record round-trip, the liveness rule behind
//! `interrupted`, control files, history, resolution by id/prefix/reference,
//! and the sweep. Public API only.

mod support;

use std::fs::{self, File};
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use kernel::install::event::InstallProgress;
use kernel::install::plan::{InstallPlan, InstallPlanFile};
use kernel::install::provider::InstallProviderId;
use kernel::install::pulls::{
    PULLED_AGAIN_LINE, PullControl, PullError, PullEventKind, PullJobDir, PullLock, PullState,
    PullStatus, PullStore, REGISTERING_LINE, START_GRACE_MS, StopAnswer,
};
use support::TempDir;

fn plan(reference: &str) -> InstallPlan {
    let mut plan = InstallPlan::new(
        InstallProviderId::huggingface(),
        reference,
        reference.rsplit('/').next().unwrap_or(reference),
        "/models/somewhere",
    );
    plan.files = vec![InstallPlanFile::new("model.gguf", Some(4_000))];
    plan.total_bytes = Some(4_000);
    plan.remaining_bytes = Some(4_000);
    plan.revision = Some("abc123".to_owned());
    plan
}

fn store(dir: &TempDir) -> PullStore {
    PullStore::new(dir.join("pulls"))
}

/// What a worker that died leaves behind: its pid in the record and its lock
/// file with nobody holding it.
fn abandoned_worker(job: &PullJobDir) {
    File::create(job.lock_path()).expect("lock file");
    job.update_status(1, |status| status.pid = Some(4_242))
        .expect("record the pid");
}

#[test]
fn create_lays_down_the_descriptor_and_a_queued_record() {
    let dir = TempDir::new();
    let store = store(&dir);
    let job = store.create(&plan("Qwen/Qwen3-8B"), 1_000).unwrap();

    assert_eq!(job.job().reference, "Qwen/Qwen3-8B");
    assert_eq!(job.job().provider.as_str(), "huggingface");
    assert_eq!(job.job().revision.as_deref(), Some("abc123"));
    assert_eq!(job.job().total_bytes, Some(4_000));
    assert_eq!(job.job().created_at_ms, 1_000);
    assert!(job.path().join("job.json").exists());
    assert!(job.path().join("status.json").exists());
    assert_eq!(job.stored_status().state, PullState::Queued);
}

#[test]
fn the_id_carries_the_timestamp_and_a_readable_slug() {
    let dir = TempDir::new();
    let store = store(&dir);
    let job = store
        .create(&plan("Qwen/Qwen3-8B"), 1_700_000_000_000)
        .unwrap();

    assert_eq!(job.id(), "1700000000000-qwen-qwen3-8b");
}

#[test]
fn a_long_reference_is_cut_down_to_a_typeable_id() {
    let dir = TempDir::new();
    let store = store(&dir);
    let job = store
        .create(&plan("unsloth/Qwen3-Coder-30B-A3B-Instruct-GGUF"), 1_000)
        .unwrap();

    let (_, slug) = job.id().split_once('-').expect("stamped id");
    assert!(slug.len() <= 40, "slug {slug} is longer than the limit");
    assert!(!slug.ends_with('-'));
    assert!(job.id().starts_with("1000-unsloth-qwen3-coder-30b"));
}

#[test]
fn a_reference_with_nothing_to_slug_still_gets_a_name() {
    let dir = TempDir::new();
    let store = store(&dir);
    let job = store.create(&plan("///..."), 1_000).unwrap();

    assert_eq!(job.id(), "1000-model");
    assert_eq!(job.path().parent(), Some(store.root()));
}

#[test]
fn two_pulls_in_one_millisecond_get_their_own_directories() {
    let dir = TempDir::new();
    let store = store(&dir);
    let first = store.create(&plan("gemma3:4b"), 500).unwrap();
    let second = store.create(&plan("gemma3:4b"), 500).unwrap();

    assert_eq!(first.id(), "500-gemma3-4b");
    assert_eq!(second.id(), "500-gemma3-4b-2");
    assert_eq!(store.list().len(), 2);
}

#[test]
fn create_steps_over_a_directory_that_already_holds_the_id() {
    let dir = TempDir::new();
    let store = store(&dir);
    fs::create_dir_all(store.root().join("700-gemma3-4b")).unwrap();

    let job = store.create(&plan("gemma3:4b"), 700).unwrap();
    assert_eq!(job.id(), "700-gemma3-4b-2");
}

#[test]
fn the_descriptor_is_written_in_the_shape_other_builds_read() {
    let dir = TempDir::new();
    let store = store(&dir);
    let job = store.create(&plan("Qwen/Qwen3-8B"), 1_000).unwrap();

    let written: serde_json::Value =
        serde_json::from_slice(&fs::read(job.path().join("job.json")).unwrap()).unwrap();
    assert_eq!(written["id"], serde_json::json!(job.id()));
    assert_eq!(written["provider"], serde_json::json!("huggingface"));
    assert_eq!(written["reference"], serde_json::json!("Qwen/Qwen3-8B"));
    assert_eq!(written["display_name"], serde_json::json!("Qwen3-8B"));
    assert_eq!(
        written["destination"],
        serde_json::json!("/models/somewhere")
    );
    assert_eq!(written["revision"], serde_json::json!("abc123"));
    assert_eq!(written["total_bytes"], serde_json::json!(4_000));
    assert_eq!(written["created_at_ms"], serde_json::json!(1_000));
}

#[test]
fn the_record_is_written_in_the_shape_other_builds_read() {
    let dir = TempDir::new();
    let store = store(&dir);
    let job = store.create(&plan("gemma3:4b"), 1_000).unwrap();
    job.update_status(2_000, |status| {
        status.state = PullState::Running;
        status.attempt = 1;
        status.pid = Some(77);
        status.progress = InstallProgress {
            bytes_downloaded: 10,
            total_bytes: Some(20),
            total_is_partial: false,
            current_file: Some("model.gguf".to_owned()),
        };
    })
    .unwrap();

    let written: serde_json::Value =
        serde_json::from_slice(&fs::read(job.path().join("status.json")).unwrap()).unwrap();
    assert_eq!(written["state"], serde_json::json!("running"));
    assert_eq!(written["attempt"], serde_json::json!(1));
    assert_eq!(written["pid"], serde_json::json!(77));
    assert_eq!(written["updated_at_ms"], serde_json::json!(2_000));
    assert_eq!(
        written["progress"]["bytes_downloaded"],
        serde_json::json!(10)
    );
    assert_eq!(written["progress"]["total_bytes"], serde_json::json!(20));
    assert_eq!(
        written["progress"]["current_file"],
        serde_json::json!("model.gguf")
    );
    assert!(written.get("message").is_none());
}

#[test]
fn a_history_line_is_written_in_the_shape_other_builds_read() {
    let dir = TempDir::new();
    let store = store(&dir);
    let job = store.create(&plan("gemma3:4b"), 1_000).unwrap();
    job.append(
        PullEventKind::State {
            state: PullState::Running,
        },
        10,
    )
    .unwrap();

    let line = fs::read_to_string(job.path().join("events.jsonl")).unwrap();
    assert_eq!(
        line.trim(),
        r#"{"at_ms":10,"event":"state","state":"running"}"#
    );
}

#[test]
fn the_record_round_trips_through_disk() {
    let dir = TempDir::new();
    let store = store(&dir);
    let job = store.create(&plan("Qwen/Qwen3-8B"), 1_000).unwrap();

    let mut status = PullStatus::queued(1_000);
    status.state = PullState::Running;
    status.attempt = 2;
    status.next_attempt_at_ms = Some(5_000);
    status.message = Some("retrying".to_owned());
    status.pid = Some(4_242);
    status.status_line = Some("fetching model.gguf".to_owned());
    status.progress = InstallProgress {
        bytes_downloaded: 1_500,
        total_bytes: Some(4_000),
        total_is_partial: false,
        current_file: Some("model.gguf".to_owned()),
    };
    job.write_status(&status).unwrap();

    let reopened = store.open(job.id()).unwrap();
    assert_eq!(reopened.stored_status(), status);
}

#[test]
fn update_status_stamps_the_write() {
    let dir = TempDir::new();
    let store = store(&dir);
    let job = store.create(&plan("gemma3:4b"), 1_000).unwrap();

    let updated = job
        .update_status(9_000, |status| status.state = PullState::Paused)
        .unwrap();

    assert_eq!(updated.updated_at_ms, 9_000);
    assert_eq!(job.stored_status().state, PullState::Paused);
}

#[test]
fn a_provider_saying_registering_does_not_put_a_pull_past_stopping() {
    let mut status = PullStatus::queued(1_000);
    status.state = PullState::Running;
    status.status_line = Some(REGISTERING_LINE.to_owned());

    assert!(!status.past_stopping());
}

#[test]
fn the_worker_registering_puts_a_pull_past_stopping_whatever_its_line_says() {
    let mut status = PullStatus::queued(1_000);
    status.state = PullState::Running;
    status.registering_until_ms = Some(5_000);

    for line in [None, Some("fetching model.gguf"), Some(REGISTERING_LINE)] {
        status.status_line = line.map(str::to_owned);
        assert!(status.past_stopping(), "{line:?}");
    }
    // Only a running worker is registering: a record whose worker is gone
    // reads interrupted, and that can be stopped like any other.
    status.state = PullState::Interrupted;
    assert!(!status.past_stopping());
}

#[test]
fn a_record_without_the_registering_field_still_decodes() {
    let dir = TempDir::new();
    let store = store(&dir);
    let job = store.create(&plan("Qwen/Qwen3-8B"), 1_000).unwrap();
    fs::write(
        job.path().join("status.json"),
        br#"{"state":"running","status_line":"registering","attempt":1,"pid":7,"updated_at_ms":2000}"#,
    )
    .unwrap();

    let status = job.stored_status();
    assert_eq!(status.state, PullState::Running);
    assert_eq!(status.pid, Some(7));
    assert_eq!(status.registering_until_ms, None);
    assert!(!status.past_stopping());

    job.update_status(3_000, |status| status.registering_until_ms = Some(9_000))
        .unwrap();
    let written: serde_json::Value =
        serde_json::from_slice(&fs::read(job.path().join("status.json")).unwrap()).unwrap();
    assert_eq!(written["registering_until_ms"], serde_json::json!(9_000));
    job.update_status(4_000, |status| status.registering_until_ms = None)
        .unwrap();
    let written: serde_json::Value =
        serde_json::from_slice(&fs::read(job.path().join("status.json")).unwrap()).unwrap();
    assert!(written.get("registering_until_ms").is_none());
}

#[test]
fn a_stop_reads_as_answered_only_once_the_record_says_how() {
    use PullControl::{Cancel, Pause};
    use PullState::*;

    let cases = [
        (Pause, Queued, false, None),
        (Pause, Running, false, None),
        (Pause, Running, true, Some(StopAnswer::Landed)),
        (Pause, Paused, false, Some(StopAnswer::Honoured(Paused))),
        (Pause, Done, false, Some(StopAnswer::Landed)),
        (Pause, Failed, false, Some(StopAnswer::Ended(Failed))),
        (Pause, Cancelled, false, Some(StopAnswer::Ended(Cancelled))),
        (
            Pause,
            Interrupted,
            false,
            Some(StopAnswer::Ended(Interrupted)),
        ),
        (Cancel, Queued, false, None),
        (Cancel, Running, false, None),
        (Cancel, Running, true, Some(StopAnswer::Landed)),
        (
            Cancel,
            Cancelled,
            false,
            Some(StopAnswer::Honoured(Cancelled)),
        ),
        (Cancel, Paused, false, Some(StopAnswer::Ended(Paused))),
        (Cancel, Done, false, Some(StopAnswer::Landed)),
        (Cancel, Failed, false, Some(StopAnswer::Ended(Failed))),
        (
            Cancel,
            Interrupted,
            false,
            Some(StopAnswer::Ended(Interrupted)),
        ),
        (
            Pause,
            Unreadable,
            false,
            Some(StopAnswer::Ended(Unreadable)),
        ),
        (
            Cancel,
            Unreadable,
            false,
            Some(StopAnswer::Ended(Unreadable)),
        ),
    ];
    for (control, state, registering, expected) in cases {
        let mut status = PullStatus::queued(1_000);
        status.state = state;
        status.registering_until_ms = registering.then_some(5_000);
        assert_eq!(
            control.answer(&status),
            expected,
            "{control} finding {state}, registering {registering}"
        );
    }
}

#[test]
fn a_running_job_with_no_worker_reads_as_interrupted() {
    let dir = TempDir::new();
    let store = store(&dir);
    let job = store.create(&plan("gemma3:4b"), 1_000).unwrap();
    job.update_status(1, |status| status.state = PullState::Running)
        .unwrap();
    abandoned_worker(&job);

    assert!(!job.worker_alive());
    assert_eq!(job.stored_status().state, PullState::Running);
    assert_eq!(job.status().state, PullState::Interrupted);
}

#[test]
fn a_queued_job_waits_to_be_picked_up_before_it_can_be_interrupted() {
    let dir = TempDir::new();
    let store = store(&dir);
    let job = store.create(&plan("gemma3:4b"), 1_000).unwrap();

    // No pid yet: the worker is still starting, or stood down before it ran.
    assert_eq!(job.status().state, PullState::Queued);
    File::create(job.lock_path()).unwrap();
    assert_eq!(job.status().state, PullState::Queued);

    abandoned_worker(&job);
    assert_eq!(job.status().state, PullState::Interrupted);
}

#[test]
fn a_claimed_job_stays_running_and_frees_up_when_the_claim_drops() {
    let dir = TempDir::new();
    let store = store(&dir);
    let job = store.create(&plan("gemma3:4b"), 1_000).unwrap();
    job.update_status(1, |status| status.state = PullState::Running)
        .unwrap();

    let claim = job.claim().unwrap().expect("an unclaimed job");
    assert!(job.worker_alive());
    assert_eq!(job.status().state, PullState::Running);
    assert!(job.claim().unwrap().is_none(), "claimed twice");

    drop(claim);
    assert!(!job.worker_alive());
    assert_eq!(job.status().state, PullState::Interrupted);
}

#[test]
fn readers_probing_at_once_do_not_report_each_other_as_the_worker() {
    let dir = TempDir::new();
    let store = store(&dir);
    let job = store.create(&plan("gemma3:4b"), 1_000).unwrap();
    job.update_status(1, |status| status.state = PullState::Running)
        .unwrap();
    abandoned_worker(&job);

    let wrong = Arc::new(AtomicU64::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let readers: Vec<_> = (0..4)
        .map(|_| {
            let job = job.clone();
            let wrong = Arc::clone(&wrong);
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                for _ in 0..4_000 {
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    if job.worker_alive() {
                        wrong.fetch_add(1, Ordering::Relaxed);
                        stop.store(true, Ordering::Relaxed);
                    }
                }
            })
        })
        .collect();
    for reader in readers {
        reader.join().expect("reader thread");
    }

    assert_eq!(
        wrong.load(Ordering::Relaxed),
        0,
        "a probe reported a dead worker as alive"
    );
}

#[test]
fn an_ended_job_is_never_reread_as_interrupted() {
    let dir = TempDir::new();
    let store = store(&dir);
    for (offset, state) in [PullState::Done, PullState::Failed, PullState::Cancelled]
        .into_iter()
        .enumerate()
    {
        let job = store.create(&plan("gemma3:4b"), offset as i64).unwrap();
        job.update_status(1, |status| status.state = state).unwrap();
        abandoned_worker(&job);
        assert_eq!(job.status().state, state);
    }
    let paused = store.create(&plan("gemma3:4b"), 10).unwrap();
    paused
        .update_status(1, |status| status.state = PullState::Paused)
        .unwrap();
    abandoned_worker(&paused);
    assert_eq!(paused.status().state, PullState::Paused);
}

#[test]
fn a_missing_record_reads_as_a_job_that_never_started() {
    let dir = TempDir::new();
    let store = store(&dir);
    let job = store.create(&plan("gemma3:4b"), 1_000).unwrap();
    fs::remove_file(job.path().join("status.json")).unwrap();

    assert_eq!(job.stored_status().state, PullState::Queued);
    assert_eq!(job.status().state, PullState::Queued);
}

/// A job created at 1 whose record has been replaced by `bytes`.
fn job_with_record(store: &PullStore, reference: &str, bytes: &[u8]) -> PullJobDir {
    let job = store.create(&plan(reference), 1).unwrap();
    fs::write(job.path().join("status.json"), bytes).unwrap();
    job
}

#[test]
fn an_undecodable_record_is_left_in_place_and_read_as_unreadable() {
    let dir = TempDir::new();
    let store = store(&dir);
    let job = job_with_record(&store, "gemma3:4b", b"{ not json");

    let stored = job.stored_status();
    assert_eq!(stored.state, PullState::Unreadable);
    assert!(
        stored
            .message
            .as_deref()
            .is_some_and(|message| message.starts_with("its record could not be read: ")),
        "{stored:?}"
    );
    assert!(
        stored.updated_at_ms > 1,
        "stamped with the file's own change"
    );
    assert_eq!(job.status().state, PullState::Unreadable);
    let reading = job.reading(i64::MAX);
    assert_eq!(reading.status.state, PullState::Unreadable);
    assert!(!reading.abandoned);
    assert!(!job.abandoned(i64::MAX, START_GRACE_MS));
    assert_eq!(
        fs::read(job.path().join("status.json")).unwrap(),
        b"{ not json"
    );
}

#[test]
fn a_state_word_from_a_newer_build_reads_as_unreadable() {
    let dir = TempDir::new();
    let store = store(&dir);
    let record = br#"{"state":"verifying","updated_at_ms":1}"#;
    let job = job_with_record(&store, "gemma3:4b", record);

    assert_eq!(job.stored_status().state, PullState::Unreadable);
    assert_eq!(job.reading(i64::MAX).status.state, PullState::Unreadable);
    assert_eq!(fs::read(job.path().join("status.json")).unwrap(), record);
}

#[test]
fn an_unreadable_record_is_never_written_over() {
    let dir = TempDir::new();
    let store = store(&dir);
    let job = job_with_record(&store, "gemma3:4b", b"{ not json");

    let error = job
        .update_status(2_000, |status| status.state = PullState::Queued)
        .expect_err("nothing writes over a record it could not read");

    assert!(matches!(error, PullError::RecordUnreadable(_)), "{error:?}");
    assert_eq!(
        fs::read(job.path().join("status.json")).unwrap(),
        b"{ not json"
    );
}

#[test]
fn an_unreadable_record_is_not_under_way_orphaned_or_swept() {
    let dir = TempDir::new();
    let store = store(&dir);
    let hub = InstallProviderId::huggingface();
    let job = job_with_record(&store, "Qwen/Qwen3-8B", b"{ not json");

    assert!(store.under_way(&hub, "Qwen/Qwen3-8B", i64::MAX).is_none());
    assert!(store.orphaned(i64::MAX).is_empty());
    assert!(store.stopped(i64::MAX).expect("read the store").is_empty());
    assert_eq!(store.sweep(0, i64::MAX), 0);
    assert!(matches!(
        job.forget(),
        Err(PullError::NotEnded {
            state: PullState::Unreadable,
            ..
        })
    ));
    assert!(job.path().exists());
    assert_eq!(
        fs::read(job.path().join("status.json")).unwrap(),
        b"{ not json"
    );
}

#[test]
fn a_name_an_unreadable_job_shares_means_the_one_still_going() {
    let dir = TempDir::new();
    let store = store(&dir);
    job_with_record(&store, "Qwen/Qwen3-8B", b"{ not json");
    let going = store.create(&plan("Qwen/Qwen3-8B"), 2_000).unwrap();
    going
        .update_status(2_000, |status| status.state = PullState::Paused)
        .expect("stop the second job");

    let found = store.resolve("Qwen/Qwen3-8B").expect("the job still going");

    assert_eq!(found.id(), going.id());
}

#[test]
fn an_unreadable_state_is_never_decoded_from_disk() {
    let dir = TempDir::new();
    let store = store(&dir);
    let job = job_with_record(
        &store,
        "gemma3:4b",
        br#"{"state":"unreadable","updated_at_ms":5}"#,
    );

    // Shown as the word it is, never taken back from a file: a record that
    // says it is unreadable is no more readable for saying so.
    let stored = job.stored_status();
    assert_eq!(stored.state, PullState::Unreadable);
    assert_ne!(stored.updated_at_ms, 5);
    assert!(serde_json::from_str::<PullState>(r#""unreadable""#).is_err());
    assert_eq!(
        serde_json::to_value(PullState::Unreadable).unwrap(),
        serde_json::json!("unreadable")
    );
    assert!(!PullState::Unreadable.is_terminal());
    assert!(!PullState::Unreadable.is_resumable());
    assert!(!PullState::Unreadable.is_live());
}

#[test]
fn a_record_written_before_these_fields_still_reads_as_written() {
    let dir = TempDir::new();
    let store = store(&dir);
    // As 69b3b11 writes them: every optional key left out, and a full one.
    let bare = job_with_record(
        &store,
        "org/bare",
        br#"{"state":"paused","progress":{"bytes_downloaded":0,"total_is_partial":false},"attempt":0,"updated_at_ms":5}"#,
    );
    let full = job_with_record(
        &store,
        "org/full",
        br#"{"state":"failed","progress":{"bytes_downloaded":1024,"total_bytes":2048,"total_is_partial":false,"current_file":"model.gguf"},"status_line":"fetching model.gguf","attempt":2,"next_attempt_at_ms":9000,"message":"the hub went away","pid":4242,"updated_at_ms":7}"#,
    );
    let registering = job_with_record(
        &store,
        "org/registering",
        br#"{"state":"running","progress":{"bytes_downloaded":2048,"total_bytes":2048,"total_is_partial":false},"status_line":"registering","attempt":1,"registering_until_ms":9000,"pid":4242,"updated_at_ms":8}"#,
    );

    let bare = bare.stored_status();
    assert_eq!(bare.state, PullState::Paused);
    assert_eq!(bare.updated_at_ms, 5);
    assert_eq!(bare.registering_until_ms, None);
    let full = full.stored_status();
    assert_eq!(full.state, PullState::Failed);
    assert_eq!(full.message.as_deref(), Some("the hub went away"));
    assert_eq!(full.progress.total_bytes, Some(2048));
    let registering = registering.stored_status();
    assert_eq!(registering.state, PullState::Running);
    assert_eq!(registering.registering_until_ms, Some(9_000));
}

#[test]
fn an_undecodable_descriptor_is_reported_and_never_moved_aside() {
    let dir = TempDir::new();
    let store = store(&dir);
    let job = store.create(&plan("gemma3:4b"), 1_000).unwrap();
    let id = job.id().to_owned();
    fs::write(job.path().join("job.json"), b"{\"id\":").unwrap();

    assert!(matches!(store.open(&id), Err(PullError::Unreadable { .. })));
    assert!(store.list().is_empty());
    let left: Vec<String> = fs::read_dir(store.root().join(&id))
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        left.contains(&"job.json".to_owned()),
        "left behind: {left:?}"
    );
}

#[test]
fn writing_into_a_swept_job_reports_it_gone_instead_of_recreating_it() {
    let dir = TempDir::new();
    let store = store(&dir);
    let job = store.create(&plan("gemma3:4b"), 1_000).unwrap();
    let path = job.path().to_path_buf();
    job.remove().unwrap();

    assert!(matches!(
        job.update_status(2, |status| status.state = PullState::Done),
        Err(PullError::NotFound(_))
    ));
    assert!(job.request(PullControl::Cancel).is_err());
    assert!(!path.exists(), "the directory came back");
}

#[test]
fn control_is_written_read_and_cleared() {
    let dir = TempDir::new();
    let store = store(&dir);
    let job = store.create(&plan("gemma3:4b"), 1_000).unwrap();

    assert_eq!(job.control(), None);
    job.request(PullControl::Pause).unwrap();
    assert_eq!(job.control(), Some(PullControl::Pause));
    assert_eq!(job.clear_control(PullControl::Pause).unwrap(), None);
    assert_eq!(job.control(), None);
    assert_eq!(job.clear_control(PullControl::Pause).unwrap(), None);
}

#[test]
fn clearing_a_honoured_pause_leaves_a_cancel_that_landed_after_it() {
    let dir = TempDir::new();
    let store = store(&dir);
    let job = store.create(&plan("gemma3:4b"), 1_000).unwrap();

    job.request(PullControl::Pause).unwrap();
    job.request(PullControl::Cancel).unwrap();
    let waiting = job.clear_control(PullControl::Pause).unwrap();

    assert_eq!(waiting, Some(PullControl::Cancel));
    assert_eq!(job.control(), Some(PullControl::Cancel));
}

/// Hold the lock the control file's writers and its clearer share, as one of
/// them in the middle of its turn would.
fn hold_control(job: &PullJobDir) -> PullLock {
    kernel::install::pulls::take_lock(&job.path().join("control.lock"))
        .expect("take the control lock")
        .expect("the control lock is free")
}

/// Whether `done` stays unset for a moment, as it does for a thread blocked on
/// a lock.
fn stays_blocked(done: &AtomicBool) -> bool {
    std::thread::sleep(std::time::Duration::from_millis(50));
    !done.load(Ordering::SeqCst)
}

#[test]
fn clearing_a_control_waits_for_a_request_in_flight_and_keeps_what_it_wrote() {
    let dir = TempDir::new();
    let store = store(&dir);
    let job = store.create(&plan("gemma3:4b"), 1_000).unwrap();
    job.request(PullControl::Pause).unwrap();
    let held = hold_control(&job);

    let done = Arc::new(AtomicBool::new(false));
    let clearing = {
        let job = job.clone();
        let done = Arc::clone(&done);
        std::thread::spawn(move || {
            let waiting = job.clear_control(PullControl::Pause);
            done.store(true, Ordering::SeqCst);
            waiting
        })
    };
    assert!(stays_blocked(&done), "the clear waits for the hold");
    // The request in flight lands its cancel over the pause, then lets go.
    fs::write(job.path().join("control"), b"cancel").unwrap();
    drop(held);

    let waiting = clearing.join().expect("the clearing thread").unwrap();
    assert_eq!(waiting, Some(PullControl::Cancel));
    assert_eq!(job.control(), Some(PullControl::Cancel));
}

#[test]
fn a_request_waits_for_a_clear_in_flight() {
    let dir = TempDir::new();
    let store = store(&dir);
    let job = store.create(&plan("gemma3:4b"), 1_000).unwrap();
    job.request(PullControl::Pause).unwrap();
    let held = hold_control(&job);

    let done = Arc::new(AtomicBool::new(false));
    let requesting = {
        let job = job.clone();
        let done = Arc::clone(&done);
        std::thread::spawn(move || {
            let written = job.request(PullControl::Cancel);
            done.store(true, Ordering::SeqCst);
            written
        })
    };
    assert!(stays_blocked(&done), "the request waits for the hold");
    assert_eq!(job.control(), Some(PullControl::Pause));
    // The clear in flight takes the pause it read, then lets go.
    fs::remove_file(job.path().join("control")).unwrap();
    drop(held);

    requesting.join().expect("the requesting thread").unwrap();
    assert_eq!(job.control(), Some(PullControl::Cancel));
}

#[test]
fn a_control_word_nobody_understands_is_ignored() {
    let dir = TempDir::new();
    let store = store(&dir);
    let job = store.create(&plan("gemma3:4b"), 1_000).unwrap();
    fs::write(job.path().join("control"), b"detonate").unwrap();

    assert_eq!(job.control(), None);
}

#[test]
fn history_appends_and_survives_a_damaged_line() {
    let dir = TempDir::new();
    let store = store(&dir);
    let job = store.create(&plan("gemma3:4b"), 1_000).unwrap();

    job.append(
        PullEventKind::State {
            state: PullState::Running,
        },
        10,
    )
    .unwrap();
    job.append(
        PullEventKind::Status {
            text: "resolving".to_owned(),
        },
        20,
    )
    .unwrap();
    fs::OpenOptions::new()
        .append(true)
        .open(job.path().join("events.jsonl"))
        .and_then(|mut file| file.write_all(b"{ torn\n"))
        .unwrap();
    job.append(
        PullEventKind::Retry {
            attempt: 1,
            reason: "connection reset".to_owned(),
            delay_ms: 5_000,
        },
        30,
    )
    .unwrap();

    let events = job.events();
    assert_eq!(events.len(), 3);
    assert_eq!(events[0].at_ms, 10);
    assert_eq!(
        events[2].kind,
        PullEventKind::Retry {
            attempt: 1,
            reason: "connection reset".to_owned(),
            delay_ms: 5_000,
        }
    );
}

#[test]
fn a_line_that_is_not_even_text_costs_only_that_line() {
    let dir = TempDir::new();
    let store = store(&dir);
    let job = store.create(&plan("gemma3:4b"), 1_000).unwrap();

    job.append(
        PullEventKind::Status {
            text: "first".to_owned(),
        },
        10,
    )
    .unwrap();
    fs::OpenOptions::new()
        .append(true)
        .open(job.path().join("events.jsonl"))
        .and_then(|mut file| file.write_all(&[0xff, 0xfe]))
        .unwrap();
    job.append(
        PullEventKind::Status {
            text: "second".to_owned(),
        },
        20,
    )
    .unwrap();

    let events = job.events();
    assert_eq!(events.len(), 2);
    assert_eq!(events[1].at_ms, 20);
}

#[test]
fn a_job_with_no_history_has_no_events() {
    let dir = TempDir::new();
    let store = store(&dir);
    let job = store.create(&plan("gemma3:4b"), 1_000).unwrap();
    assert!(job.events().is_empty());
}

#[test]
fn a_worker_that_stood_down_leaves_a_job_nobody_reads_as_interrupted() {
    let dir = TempDir::new();
    let store = store(&dir);
    let job = store.create(&plan("gemma3:4b"), 1_000).unwrap();
    // A worker took the job's lock, found the reference already being pulled,
    // and left without ever writing a pid.
    drop(job.claim().unwrap().expect("an unclaimed job"));

    assert!(job.lock_path().exists());
    assert!(!job.worker_alive());
    assert_eq!(job.status().state, PullState::Queued);
}

#[test]
fn a_job_directory_opens_by_path_the_way_a_worker_is_handed_one() {
    let dir = TempDir::new();
    let store = store(&dir);
    let job = store.create(&plan("gemma3:4b"), 1_000).unwrap();

    let opened = PullJobDir::open(job.path()).unwrap();
    assert_eq!(opened.id(), job.id());
    assert_eq!(opened.job().reference, "gemma3:4b");
}

#[test]
fn a_directory_with_no_descriptor_is_not_a_job() {
    let dir = TempDir::new();
    let store = store(&dir);
    fs::create_dir_all(store.root().join("1000-nothing")).unwrap();

    assert!(matches!(
        PullJobDir::open(store.root().join("1000-nothing")),
        Err(PullError::NotFound(_))
    ));
}

#[test]
fn list_is_oldest_first_and_skips_what_is_not_a_job() {
    let dir = TempDir::new();
    let store = store(&dir);
    let first = store.create(&plan("first/model"), 100).unwrap();
    let second = store.create(&plan("second/model"), 200).unwrap();
    fs::create_dir_all(store.root().join("not-a-job")).unwrap();
    fs::write(store.root().join("stray-file"), b"x").unwrap();

    let ids: Vec<String> = store.list().iter().map(|job| job.id().to_owned()).collect();
    assert_eq!(ids, vec![first.id().to_owned(), second.id().to_owned()]);
}

#[test]
fn a_store_nothing_has_been_pulled_into_is_empty_not_an_error() {
    let dir = TempDir::new();
    let store = store(&dir);
    assert!(store.list().is_empty());
    assert!(store.jobs().unwrap().is_empty());
}

#[test]
fn resolve_takes_an_id_a_prefix_or_a_reference() {
    let dir = TempDir::new();
    let store = store(&dir);
    let job = store
        .create(&plan("Qwen/Qwen3-8B"), 1_700_000_000_000)
        .unwrap();

    assert_eq!(store.resolve(job.id()).unwrap().id(), job.id());
    assert_eq!(store.resolve("17000000").unwrap().id(), job.id());
    assert_eq!(store.resolve("qwen/qwen3-8b").unwrap().id(), job.id());
    assert_eq!(store.resolve("  qwen/qwen3-8b  ").unwrap().id(), job.id());
}

#[test]
fn resolve_prefers_an_id_prefix_to_another_jobs_reference() {
    let dir = TempDir::new();
    let store = store(&dir);
    let prefixed = store.create(&plan("first/model"), 900).unwrap();
    let named = store.create(&plan("900"), 1_000).unwrap();

    assert_eq!(prefixed.id(), "900-first-model");
    assert_eq!(named.job().reference, "900");
    assert_eq!(store.resolve("900").unwrap().id(), prefixed.id());
}

#[test]
fn resolve_refuses_a_prefix_that_matches_two_jobs() {
    let dir = TempDir::new();
    let store = store(&dir);
    store.create(&plan("gemma3:4b"), 500).unwrap();
    store.create(&plan("gemma3:4b"), 500).unwrap();

    match store.resolve("500-gemma") {
        Err(PullError::Ambiguous { count, .. }) => assert_eq!(count, 2),
        other => panic!("expected an ambiguous match, got {other:?}"),
    }
}

#[test]
fn resolve_reports_what_it_could_not_find() {
    let dir = TempDir::new();
    let store = store(&dir);
    store.create(&plan("gemma3:4b"), 1_000).unwrap();

    assert!(matches!(
        store.resolve("nothing-like-this"),
        Err(PullError::NotFound(_))
    ));
    assert!(matches!(store.resolve("   "), Err(PullError::NotFound(_))));
}

#[test]
fn open_reports_a_job_that_is_not_there() {
    let dir = TempDir::new();
    let store = store(&dir);
    assert!(matches!(
        store.open("1700000000000-ghost"),
        Err(PullError::NotFound(_))
    ));
}

#[test]
fn sweep_collects_old_ended_jobs_and_keeps_the_newest() {
    let dir = TempDir::new();
    let store = store(&dir);
    let old_done = store.create(&plan("old/done"), 10).unwrap();
    let newer_done = store.create(&plan("newer/done"), 20).unwrap();
    let running = store.create(&plan("still/running"), 30).unwrap();
    let recent_done = store.create(&plan("recent/done"), 40).unwrap();

    old_done
        .update_status(100, |status| status.state = PullState::Done)
        .unwrap();
    newer_done
        .update_status(200, |status| status.state = PullState::Failed)
        .unwrap();
    running
        .update_status(100, |status| status.state = PullState::Running)
        .unwrap();
    recent_done
        .update_status(9_000, |status| status.state = PullState::Done)
        .unwrap();

    assert_eq!(store.sweep(2, 1_000), 1);

    let left: Vec<String> = store.list().iter().map(|job| job.id().to_owned()).collect();
    assert!(!left.contains(&old_done.id().to_owned()));
    assert!(left.contains(&newer_done.id().to_owned()));
    assert!(left.contains(&running.id().to_owned()));
    assert!(left.contains(&recent_done.id().to_owned()));
}

#[test]
fn forget_takes_only_an_ended_job_nobody_holds() {
    let dir = TempDir::new();
    let store = store(&dir);
    let job = store.create(&plan("gemma3:4b"), 10).unwrap();
    job.update_status(20, |status| status.state = PullState::Running)
        .unwrap();
    // Nobody holds it, so it is the interrupted pull every surface shows.
    assert!(matches!(
        job.forget(),
        Err(PullError::NotEnded {
            state: PullState::Interrupted,
            ..
        })
    ));
    assert!(job.path().exists());

    let claim = job.claim().unwrap().expect("an unclaimed job");
    assert!(matches!(
        job.forget(),
        Err(PullError::NotEnded {
            state: PullState::Running,
            ..
        })
    ));
    assert!(job.path().exists());

    job.update_status(30, |status| status.state = PullState::Done)
        .unwrap();
    assert!(matches!(job.forget(), Err(PullError::Held(_))));
    assert!(job.path().exists());

    drop(claim);
    job.forget().unwrap();
    assert!(!job.path().exists());
    assert!(store.list().is_empty());
}

#[test]
fn forgetting_a_pull_no_worker_took_up_names_it_interrupted() {
    let dir = TempDir::new();
    let store = store(&dir);
    let job = store.create(&plan("gemma3:4b"), 1).unwrap();

    assert!(matches!(
        job.forget(),
        Err(PullError::NotEnded {
            state: PullState::Interrupted,
            ..
        })
    ));
    assert!(job.path().exists());
}

#[test]
fn sweep_leaves_a_job_whose_worker_still_holds_it() {
    let dir = TempDir::new();
    let store = store(&dir);
    let job = store.create(&plan("gemma3:4b"), 10).unwrap();
    let claim = job.claim().unwrap().expect("an unclaimed job");
    // A worker writes `done` and then registers what it fetched, still holding
    // the lock: sweeping the directory out from under it is what this forbids.
    job.update_status(20, |status| status.state = PullState::Done)
        .unwrap();

    assert_eq!(store.sweep(0, i64::MAX), 0);
    assert_eq!(store.list().len(), 1);
    assert!(job.path().exists());

    drop(claim);
    assert_eq!(store.sweep(0, i64::MAX), 1);
    assert!(store.list().is_empty());
}

#[test]
fn sweep_leaves_a_job_that_has_not_ended() {
    let dir = TempDir::new();
    let store = store(&dir);
    let job = store.create(&plan("gemma3:4b"), 10).unwrap();
    job.update_status(1, |status| status.state = PullState::Paused)
        .unwrap();

    assert_eq!(store.sweep(0, i64::MAX), 0);
    assert_eq!(store.list().len(), 1);
}

#[test]
fn remove_takes_the_directory_and_nothing_else() {
    let dir = TempDir::new();
    let store = store(&dir);
    let job = store.create(&plan("gemma3:4b"), 1_000).unwrap();
    let path = job.path().to_path_buf();

    job.remove().unwrap();
    assert!(!path.exists());
    assert!(store.root().exists());
    assert!(store.list().is_empty());
}

#[test]
fn the_states_agree_on_what_has_ended() {
    assert!(PullState::Done.is_terminal());
    assert!(PullState::Failed.is_terminal());
    assert!(PullState::Cancelled.is_terminal());
    assert!(!PullState::Interrupted.is_terminal());
    assert!(PullState::Paused.is_resumable());
    assert!(PullState::Interrupted.is_resumable());
    assert!(!PullState::Running.is_resumable());
    assert!(PullState::Queued.is_live());
    assert!(PullState::Running.is_live());
    assert!(!PullState::Paused.is_live());
    assert_eq!(PullState::Running.to_string(), "running");
    assert_eq!(PullControl::Pause.resulting_state(), PullState::Paused);
    assert_eq!(PullControl::Cancel.resulting_state(), PullState::Cancelled);
    assert_eq!(PullControl::Cancel.to_string(), "cancel");
}

#[test]
fn a_name_several_jobs_answer_to_means_the_one_still_going() {
    let dir = TempDir::new();
    let store = store(&dir);
    let ended = store.create(&plan("Qwen/Qwen3-8B"), 1_000).unwrap();
    ended
        .update_status(1_000, |status| status.state = PullState::Done)
        .expect("end the first job");
    let going = store.create(&plan("Qwen/Qwen3-8B"), 2_000).unwrap();
    going
        .update_status(2_000, |status| status.state = PullState::Paused)
        .expect("stop the second job");

    let found = store.resolve("Qwen/Qwen3-8B").expect("the job still going");

    assert_eq!(found.id(), going.id());
}

#[test]
fn a_name_two_live_jobs_answer_to_is_still_ambiguous() {
    let dir = TempDir::new();
    let store = store(&dir);
    for at in [1_000, 2_000] {
        let job = store.create(&plan("Qwen/Qwen3-8B"), at).unwrap();
        job.update_status(at, |status| status.state = PullState::Paused)
            .expect("stop the job");
    }

    let error = store
        .resolve("Qwen/Qwen3-8B")
        .expect_err("two live jobs cannot be told apart");

    assert!(matches!(error, PullError::Ambiguous { count: 2, .. }));
}

#[test]
fn a_queued_job_nothing_ever_took_up_reads_as_abandoned() {
    let dir = TempDir::new();
    let store = store(&dir);
    let job = store.create(&plan("Qwen/Qwen3-8B"), 1_000).unwrap();

    assert!(!job.abandoned(1_000, 3_000), "not yet past the grace");
    assert!(job.abandoned(9_000, 3_000), "nothing came for it");

    // A worker writes its pid as soon as it holds the job, and that is what
    // tells a job being picked up from one nobody is coming for.
    job.update_status(9_000, |status| status.pid = Some(4_242))
        .expect("record the pid");
    assert!(!job.abandoned(99_000, 3_000));
}

#[test]
fn a_job_a_worker_holds_is_never_abandoned() {
    let dir = TempDir::new();
    let store = store(&dir);
    let job = store.create(&plan("Qwen/Qwen3-8B"), 1_000).unwrap();
    let _worker = job
        .claim()
        .expect("claim the job")
        .expect("the lock is free");

    assert!(!job.abandoned(99_000, 3_000));
}

#[test]
fn a_pull_of_the_same_reference_is_joined_until_it_ends() {
    let dir = TempDir::new();
    let store = store(&dir);
    let hub = InstallProviderId::huggingface();
    let job = store.create(&plan("Qwen/Qwen3-8B"), 1_000).unwrap();
    job.update_status(1_000, |status| {
        status.state = PullState::Paused;
    })
    .expect("stop the job");

    let found = store
        .under_way(&hub, "qwen/qwen3-8b", 2_000)
        .expect("a reference is matched whatever its case");
    assert_eq!(found.id(), job.id());

    assert!(
        store
            .under_way(&InstallProviderId::ollama(), "Qwen/Qwen3-8B", 2_000)
            .is_none()
    );
    assert!(store.under_way(&hub, "Qwen/Qwen3-4B", 2_000).is_none());

    job.update_status(1_000, |status| status.state = PullState::Done)
        .expect("end the job");
    assert!(store.under_way(&hub, "Qwen/Qwen3-8B", 2_000).is_none());
}

#[test]
fn a_pull_whose_worker_never_arrived_is_not_joined_over_the_one_that_is_running() {
    let dir = TempDir::new();
    let store = store(&dir);
    let hub = InstallProviderId::huggingface();
    let running = store.create(&plan("Qwen/Qwen3-8B"), 1_000).unwrap();
    let _worker = running.claim().expect("claim").expect("the lock is free");
    running
        .update_status(1_000, |status| {
            status.state = PullState::Running;
            status.pid = Some(std::process::id());
        })
        .expect("write the record");
    // Newer, but nothing ever took it up: preferring it would hide the pull
    // that is actually moving bytes.
    store.create(&plan("Qwen/Qwen3-8B"), 2_000).unwrap();

    let found = store
        .under_way(&hub, "Qwen/Qwen3-8B", 9_000)
        .expect("the live job");

    assert_eq!(found.id(), running.id());
}

#[test]
fn a_job_nobody_took_up_reads_interrupted_once_past_the_grace() {
    let dir = TempDir::new();
    let store = store(&dir);
    let job = store.create(&plan("Qwen/Qwen3-8B"), 1_000).unwrap();

    let early = job.reading(1_000);
    assert_eq!(early.status.state, PullState::Queued);
    assert!(!early.abandoned);

    let late = job.reading(1_000 + START_GRACE_MS);
    assert_eq!(late.status.state, PullState::Interrupted);
    assert!(late.abandoned);
    assert_eq!(job.stored_status().state, PullState::Queued);
}

#[test]
fn a_job_whose_worker_died_reads_interrupted_without_being_abandoned() {
    let dir = TempDir::new();
    let store = store(&dir);
    let job = store.create(&plan("Qwen/Qwen3-8B"), 1_000).unwrap();
    job.update_status(1, |status| status.state = PullState::Running)
        .unwrap();
    abandoned_worker(&job);

    let reading = job.reading(99_000);

    assert_eq!(reading.status.state, PullState::Interrupted);
    assert!(!reading.abandoned);
}

#[test]
fn a_job_with_a_live_worker_is_never_read_as_abandoned() {
    let dir = TempDir::new();
    let store = store(&dir);
    let job = store.create(&plan("Qwen/Qwen3-8B"), 1_000).unwrap();
    let _worker = job.claim().expect("claim").expect("the lock is free");

    let reading = job.reading(1_000 + 100 * START_GRACE_MS);

    assert_eq!(reading.status.state, PullState::Queued);
    assert!(!reading.abandoned);
}

/// When the stores below are read.
const NOW: i64 = 100_000;

/// Fill `store` with one job of every kind `orphaned` and `stopped` choose
/// between, as they stand at `NOW`. Returns the ids of the abandoned, paused,
/// and dead-worker jobs, and the lock of the one a worker holds.
fn stopped_and_otherwise(store: &PullStore) -> ((String, String, String), PullLock) {
    let abandoned = store.create(&plan("org/abandoned"), 1_000).unwrap();
    let paused = store.create(&plan("org/paused"), 2_000).unwrap();
    paused
        .update_status(2_000, |status| status.state = PullState::Paused)
        .unwrap();
    let dead = store.create(&plan("org/dead"), 3_000).unwrap();
    dead.update_status(3_000, |status| status.state = PullState::Running)
        .unwrap();
    abandoned_worker(&dead);
    let done = store.create(&plan("org/done"), 4_000).unwrap();
    done.update_status(4_000, |status| status.state = PullState::Done)
        .unwrap();
    // Abandoned too, but the user pulled the same model again since.
    store.create(&plan("org/pulled-past"), 5_000).unwrap();
    let past = store.create(&plan("org/pulled-past"), 6_000).unwrap();
    let held = past.claim().expect("claim").expect("the lock is free");
    past.update_status(6_000, |status| {
        status.state = PullState::Running;
        status.pid = Some(std::process::id());
    })
    .unwrap();
    store.create(&plan("org/fresh"), NOW - 1).unwrap();
    (
        (
            abandoned.id().to_owned(),
            paused.id().to_owned(),
            dead.id().to_owned(),
        ),
        held,
    )
}

#[test]
fn the_orphaned_jobs_are_the_interrupted_and_the_abandoned_nobody_pulled_past() {
    let dir = TempDir::new();
    let store = store(&dir);
    let ((abandoned, _, dead), _held) = stopped_and_otherwise(&store);

    let orphaned: Vec<String> = store
        .orphaned(NOW)
        .iter()
        .map(|job| job.id().to_owned())
        .collect();

    assert_eq!(orphaned, vec![abandoned, dead]);
}

#[test]
fn the_stopped_jobs_add_the_paused_to_the_orphaned() {
    let dir = TempDir::new();
    let store = store(&dir);
    let ((abandoned, paused, dead), _held) = stopped_and_otherwise(&store);

    let stopped: Vec<String> = store
        .stopped(NOW)
        .expect("read the store")
        .iter()
        .map(|job| job.id().to_owned())
        .collect();

    assert_eq!(stopped, vec![abandoned, paused, dead]);
}

#[test]
fn the_stopped_jobs_of_a_store_that_cannot_be_read_are_an_error_not_none() {
    let dir = TempDir::new();
    // A file where the store's directory should be cannot be listed.
    fs::write(dir.join("pulls"), b"not a directory").expect("block the store");
    let store = store(&dir);

    assert!(store.stopped(NOW).is_err());
    assert!(store.orphaned(NOW).is_empty());
}

#[test]
fn a_write_into_a_job_swept_from_under_it_leaves_nothing_behind() {
    let dir = TempDir::new();
    let store = store(&dir);
    let job = store.create(&plan("Qwen/Qwen3-8B"), 1_000).unwrap();
    let path = job.path().to_path_buf();
    fs::remove_dir_all(&path).expect("sweep the job");

    let error = job
        .write_status(&PullStatus::queued(2_000))
        .expect_err("a swept job is gone, not recreated");

    assert!(matches!(error, PullError::NotFound(_)));
    // An atomic write makes its parents, so the guard has to take back what the
    // write put there: a directory with a record and no descriptor is invisible
    // to every listing and therefore uncollectable.
    assert!(!path.exists());
    assert!(store.jobs().expect("read the store").is_empty());
}

#[test]
fn a_pause_never_replaces_a_cancel_waiting_to_be_read() {
    let dir = TempDir::new();
    let store = store(&dir);
    let job = store.create(&plan("gemma3:4b"), 1_000).unwrap();

    assert_eq!(
        job.request(PullControl::Cancel).unwrap(),
        PullControl::Cancel
    );
    assert_eq!(
        job.request(PullControl::Pause).unwrap(),
        PullControl::Cancel
    );
    assert_eq!(job.control(), Some(PullControl::Cancel));
}

#[test]
fn a_cancel_still_replaces_a_pause_waiting_to_be_read() {
    let dir = TempDir::new();
    let store = store(&dir);
    let job = store.create(&plan("gemma3:4b"), 1_000).unwrap();
    job.request(PullControl::Pause).unwrap();

    assert_eq!(
        job.request(PullControl::Cancel).unwrap(),
        PullControl::Cancel
    );
    assert_eq!(job.control(), Some(PullControl::Cancel));
}

#[test]
fn a_request_gives_up_on_a_control_lock_held_past_its_wait() {
    let dir = TempDir::new();
    let store = store(&dir);
    let job = store.create(&plan("gemma3:4b"), 1_000).unwrap();
    let _held = hold_control(&job);

    let refused = job
        .request_within(PullControl::Cancel, std::time::Duration::from_millis(30))
        .expect_err("the lock never came free");

    assert!(matches!(refused, PullError::ControlBusy(ref id) if id == job.id()));
    assert!(refused.to_string().contains("nothing was asked"));
    assert_eq!(job.control(), None);
}

/// An abandoned job of `org/model` beside a newer one of the same model.
fn pulled_again(store: &PullStore) -> (PullJobDir, PullJobDir) {
    let abandoned = store.create(&plan("org/model"), 1_000).unwrap();
    let again = store.create(&plan("ORG/Model"), 2_000).unwrap();
    (abandoned, again)
}

#[test]
fn a_job_nobody_took_up_reads_failed_while_its_model_is_pulled_again() {
    let dir = TempDir::new();
    let store = store(&dir);
    let (abandoned, again) = pulled_again(&store);
    let _held = again.claim().expect("claim").expect("the lock is free");
    again
        .update_status(2_000, |status| {
            status.state = PullState::Running;
            status.pid = Some(std::process::id());
        })
        .unwrap();

    let reading = abandoned.reading(NOW);

    assert_eq!(reading.status.state, PullState::Failed);
    assert_eq!(reading.status.message.as_deref(), Some(PULLED_AGAIN_LINE));
    assert!(reading.superseded);
    assert!(!reading.abandoned);
    assert_eq!(abandoned.stored_status().state, PullState::Queued);
    assert!(store.orphaned(NOW).is_empty());
    assert!(store.stopped(NOW).unwrap().is_empty());
}

#[test]
fn a_job_nobody_took_up_reads_failed_once_a_newer_pull_of_its_model_landed() {
    let dir = TempDir::new();
    let store = store(&dir);
    let (abandoned, again) = pulled_again(&store);
    again
        .update_status(2_000, |status| status.state = PullState::Done)
        .unwrap();

    assert_eq!(abandoned.reading(NOW).status.state, PullState::Failed);
    abandoned
        .forget()
        .expect("it reads ended and nobody holds it");
    assert!(!abandoned.path().exists());
}

#[test]
fn a_job_nobody_took_up_stays_resumable_beside_an_older_landed_or_a_failed_pull() {
    let dir = TempDir::new();
    let store = store(&dir);
    let older = store.create(&plan("org/model"), 500).unwrap();
    older
        .update_status(500, |status| status.state = PullState::Done)
        .unwrap();
    let (abandoned, again) = pulled_again(&store);
    again
        .update_status(2_000, |status| status.state = PullState::Failed)
        .unwrap();

    let reading = abandoned.reading(NOW);

    assert_eq!(reading.status.state, PullState::Interrupted);
    assert!(reading.abandoned && !reading.superseded);
    let orphaned: Vec<String> = store
        .orphaned(NOW)
        .iter()
        .map(|job| job.id().to_owned())
        .collect();
    assert_eq!(orphaned, vec![abandoned.id().to_owned()]);
}

#[test]
fn sweeping_takes_a_job_nobody_took_up_whose_model_was_pulled_again() {
    let dir = TempDir::new();
    let store = store(&dir);
    let (abandoned, again) = pulled_again(&store);
    again
        .update_status(2_000, |status| status.state = PullState::Done)
        .unwrap();

    assert_eq!(store.sweep(1, i64::MAX), 1);
    assert!(!abandoned.path().exists());
    assert!(again.path().exists(), "the newest ended job is kept");
}

#[test]
fn a_job_nobody_took_up_reads_failed_once_an_older_pull_of_its_model_landed_after_it() {
    let dir = TempDir::new();
    let store = store(&dir);
    let older = store.create(&plan("org/model"), 500).unwrap();
    let abandoned = store.create(&plan("org/model"), 1_000).unwrap();
    older
        .update_status(2_000, |status| status.state = PullState::Done)
        .unwrap();

    let reading = abandoned.reading(NOW);

    assert_eq!(reading.status.state, PullState::Failed);
    assert!(reading.superseded, "it landed after this job was created");
}

#[test]
fn a_copied_job_directory_is_a_sibling_not_the_job_itself() {
    let dir = TempDir::new();
    let store = store(&dir);
    let abandoned = store.create(&plan("org/model"), 1_000).unwrap();
    let copy = store.root().join("copy");
    fs::create_dir_all(&copy).unwrap();
    for file in ["job.json", "status.json"] {
        fs::copy(abandoned.path().join(file), copy.join(file)).unwrap();
    }
    let copy = PullJobDir::open(&copy).unwrap();
    assert_eq!(copy.id(), abandoned.id(), "the descriptor came along");
    copy.update_status(2_000, |status| status.state = PullState::Done)
        .unwrap();

    assert!(abandoned.reading(NOW).superseded);
}

#[test]
fn settling_a_job_nobody_took_up_keeps_it_failed_once_the_newer_pull_is_gone() {
    let dir = TempDir::new();
    let store = store(&dir);
    let (abandoned, again) = pulled_again(&store);
    again
        .update_status(2_000, |status| status.state = PullState::Done)
        .unwrap();

    assert!(abandoned.settle_superseded(NOW).unwrap());
    assert!(
        !abandoned.settle_superseded(NOW).unwrap(),
        "written down once"
    );
    again.remove().unwrap();

    let reading = abandoned.reading(NOW);
    assert_eq!(reading.status.state, PullState::Failed);
    assert_eq!(reading.status.message.as_deref(), Some(PULLED_AGAIN_LINE));
    assert!(!reading.superseded, "the record says it now");
    assert!(store.orphaned(NOW).is_empty());
    assert!(store.stopped(NOW).unwrap().is_empty());
    assert!(matches!(
        abandoned.events().last().map(|event| &event.kind),
        Some(PullEventKind::State {
            state: PullState::Failed
        })
    ));
}

#[test]
fn settling_leaves_every_other_job_alone() {
    let dir = TempDir::new();
    let store = store(&dir);
    let abandoned = store.create(&plan("org/model"), 1_000).unwrap();
    let held = store.create(&plan("org/other"), 1_000).unwrap();
    let _worker = held.claim().unwrap().expect("the lock is free");

    assert_eq!(store.settle_superseded(NOW), 0);
    assert_eq!(abandoned.stored_status().state, PullState::Queued);
    assert_eq!(held.stored_status().state, PullState::Queued);
}

#[test]
fn a_sweep_that_keeps_a_job_nobody_took_up_writes_it_down_first() {
    let dir = TempDir::new();
    let store = store(&dir);
    let (abandoned, again) = pulled_again(&store);
    again
        .update_status(2_000, |status| status.state = PullState::Done)
        .unwrap();

    assert_eq!(store.sweep(5, i64::MAX), 0);
    again.remove().unwrap();

    assert_eq!(abandoned.stored_status().state, PullState::Failed);
    assert!(store.orphaned(NOW).is_empty());
}

#[test]
fn clearing_the_control_file_gives_up_on_a_lock_held_past_its_wait() {
    let dir = TempDir::new();
    let store = store(&dir);
    let job = store.create(&plan("gemma3:4b"), 1_000).unwrap();
    job.request(PullControl::Pause).unwrap();
    let _held = hold_control(&job);

    let refused = job
        .clear_control_within(PullControl::Pause, std::time::Duration::from_millis(30))
        .expect_err("the lock never came free");

    assert!(matches!(refused, PullError::ControlBusy(_)));
    assert_eq!(job.control(), Some(PullControl::Pause));
}

#[test]
fn a_job_whose_worker_died_drops_what_that_worker_was_saying() {
    let dir = TempDir::new();
    let store = store(&dir);
    let job = store.create(&plan("org/model"), 1_000).unwrap();
    abandoned_worker(&job);
    job.update_status(2_000, |status| {
        status.state = PullState::Running;
        status.status_line = Some(REGISTERING_LINE.to_owned());
        status.registering_until_ms = Some(9_000);
    })
    .unwrap();

    let status = job.reading(NOW).status;

    assert_eq!(status.state, PullState::Interrupted);
    assert_eq!(status.status_line, None);
    assert_eq!(status.registering_until_ms, None);
    assert_eq!(
        job.stored_status().status_line.as_deref(),
        Some(REGISTERING_LINE),
        "the record is left as written"
    );
}
