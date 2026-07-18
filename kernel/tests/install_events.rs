//! Tests for install progress/events and the Ollama pull-stream aggregator.

use kernel::install::InstallError;
use kernel::install::event::{InstallEvent, InstallProgress, InstallStreamEvent};
use kernel::install::ollama_pull::{Aggregator, Outcome};

#[test]
fn fraction_is_none_unless_a_firm_total_is_known() {
    let firm = InstallProgress {
        bytes_downloaded: 25,
        total_bytes: Some(100),
        total_is_partial: false,
        current_file: None,
    };
    assert_eq!(firm.fraction(), Some(0.25));

    // A partial (growing) total yields no fraction.
    let partial = InstallProgress {
        total_is_partial: true,
        ..firm.clone()
    };
    assert_eq!(partial.fraction(), None);

    // No total, or a zero total, yields no fraction.
    assert_eq!(InstallProgress::default().fraction(), None);
    let zero = InstallProgress {
        bytes_downloaded: 10,
        total_bytes: Some(0),
        ..InstallProgress::default()
    };
    assert_eq!(zero.fraction(), None);

    // Over-count clamps to 1.0.
    let over = InstallProgress {
        bytes_downloaded: 200,
        total_bytes: Some(100),
        ..InstallProgress::default()
    };
    assert_eq!(over.fraction(), Some(1.0));
}

#[test]
fn only_the_ending_events_are_terminal() {
    assert!(InstallEvent::Done.is_terminal());
    assert!(InstallEvent::Cancelled.is_terminal());
    assert!(
        InstallEvent::Failed {
            message: "x".to_owned()
        }
        .is_terminal()
    );
    assert!(!InstallEvent::Queued.is_terminal());
    assert!(!InstallEvent::Preparing.is_terminal());
    assert!(!InstallEvent::Status("s".to_owned()).is_terminal());
    assert!(!InstallEvent::Progress(InstallProgress::default()).is_terminal());
    // The stream-event type is distinct.
    let _ = InstallStreamEvent::Status("s".to_owned());
}

#[test]
fn the_aggregator_emits_deduped_statuses() {
    let mut aggregator = Aggregator::new();
    assert_eq!(
        aggregator.fold(r#"{"status":"pulling manifest"}"#).unwrap(),
        Outcome::Status("pulling manifest".to_owned())
    );
    // The same status again is ignored.
    assert_eq!(
        aggregator.fold(r#"{"status":"pulling manifest"}"#).unwrap(),
        Outcome::Ignored
    );
    // A new status is emitted.
    assert_eq!(
        aggregator.fold(r#"{"status":"verifying"}"#).unwrap(),
        Outcome::Status("verifying".to_owned())
    );
}

#[test]
fn the_aggregator_sums_layer_progress_across_digests() {
    let mut aggregator = Aggregator::new();
    let first = aggregator
        .fold(r#"{"status":"downloading","digest":"sha:1","total":100,"completed":30}"#)
        .unwrap();
    assert_eq!(
        first,
        Outcome::Progress(InstallProgress {
            bytes_downloaded: 30,
            total_bytes: Some(100),
            total_is_partial: true,
            current_file: None,
        })
    );
    // A second layer adds to both totals.
    let second = aggregator
        .fold(r#"{"status":"downloading","digest":"sha:2","total":50,"completed":10}"#)
        .unwrap();
    assert_eq!(
        second,
        Outcome::Progress(InstallProgress {
            bytes_downloaded: 40,
            total_bytes: Some(150),
            total_is_partial: true,
            current_file: None,
        })
    );
    // A line for a known layer that omits `completed` keeps its last count.
    let third = aggregator
        .fold(r#"{"status":"downloading","digest":"sha:1","total":100}"#)
        .unwrap();
    assert_eq!(
        third,
        Outcome::Progress(InstallProgress {
            bytes_downloaded: 40,
            total_bytes: Some(150),
            total_is_partial: true,
            current_file: None,
        })
    );
}

#[test]
fn the_aggregator_reports_success_and_errors_and_ignores_noise() {
    let mut aggregator = Aggregator::new();
    assert_eq!(
        aggregator.fold(r#"{"status":"success"}"#).unwrap(),
        Outcome::Success
    );

    // An error line becomes a transfer failure.
    let error = aggregator
        .fold(r#"{"error":"model not found"}"#)
        .unwrap_err();
    assert_eq!(
        error,
        InstallError::TransferFailed("ollama: model not found".to_owned())
    );

    // Unparseable JSON, a blank status, and a status-less object are all ignored.
    assert_eq!(aggregator.fold("not json").unwrap(), Outcome::Ignored);
    assert_eq!(
        aggregator.fold(r#"{"status":""}"#).unwrap(),
        Outcome::Ignored
    );
    assert_eq!(aggregator.fold("{}").unwrap(), Outcome::Ignored);
    // A type-mismatched field fails to decode → ignored (not an error).
    assert_eq!(
        aggregator.fold(r#"{"status":"x","total":"nope"}"#).unwrap(),
        Outcome::Ignored
    );
    // Unknown extra fields are tolerated.
    assert_eq!(
        aggregator
            .fold(r#"{"status":"pulling","extra":42}"#)
            .unwrap(),
        Outcome::Status("pulling".to_owned())
    );
}

#[test]
fn fold_check_order_puts_errors_and_success_first() {
    let mut aggregator = Aggregator::new();
    // An error alongside a status → the error wins.
    assert_eq!(
        aggregator
            .fold(r#"{"error":"boom","status":"downloading"}"#)
            .unwrap_err(),
        InstallError::TransferFailed("ollama: boom".to_owned())
    );
    // A success line carrying digest/total is still Success, not Progress.
    assert_eq!(
        aggregator
            .fold(r#"{"status":"success","digest":"sha:1","total":100,"completed":100}"#)
            .unwrap(),
        Outcome::Success
    );
}

#[test]
fn a_digest_without_a_total_is_a_status_not_progress() {
    let mut aggregator = Aggregator::new();
    // Missing `total` → falls through to the status path.
    assert_eq!(
        aggregator
            .fold(r#"{"status":"downloading","digest":"sha:1"}"#)
            .unwrap(),
        Outcome::Status("downloading".to_owned())
    );
    // A progress line does NOT reset last_status, so the same status after it dedups.
    aggregator
        .fold(r#"{"status":"downloading","digest":"sha:1","total":10,"completed":5}"#)
        .unwrap();
    assert_eq!(
        aggregator
            .fold(r#"{"status":"downloading","digest":"sha:1"}"#)
            .unwrap(),
        Outcome::Ignored
    );
}

#[test]
fn negatives_are_clamped_in_progress_and_fraction() {
    let mut aggregator = Aggregator::new();
    // Negative total/completed clamp to zero in the aggregate.
    assert_eq!(
        aggregator
            .fold(r#"{"status":"downloading","digest":"sha:1","total":-100,"completed":-30}"#)
            .unwrap(),
        Outcome::Progress(InstallProgress {
            bytes_downloaded: 0,
            total_bytes: Some(0),
            total_is_partial: true,
            current_file: None,
        })
    );
    // An explicit completed: 0 keeps 0 (not the default/stored path).
    let mut aggregator = Aggregator::new();
    aggregator
        .fold(r#"{"status":"downloading","digest":"sha:1","total":100,"completed":50}"#)
        .unwrap();
    let zeroed = aggregator
        .fold(r#"{"status":"downloading","digest":"sha:1","total":100,"completed":0}"#)
        .unwrap();
    assert_eq!(
        zeroed,
        Outcome::Progress(InstallProgress {
            bytes_downloaded: 0,
            total_bytes: Some(100),
            total_is_partial: true,
            current_file: None,
        })
    );

    // A negative byte count floors the fraction at 0; a negative total → None.
    let below = InstallProgress {
        bytes_downloaded: -10,
        total_bytes: Some(100),
        ..InstallProgress::default()
    };
    assert_eq!(below.fraction(), Some(0.0));
    let negative_total = InstallProgress {
        bytes_downloaded: 10,
        total_bytes: Some(-5),
        ..InstallProgress::default()
    };
    assert_eq!(negative_total.fraction(), None);
}
