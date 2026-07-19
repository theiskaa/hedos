//! The disk-backed audit log: JSONL round-trips, unauthorized coalescing, and
//! size-triggered rotation.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use gateway::audit::{Auditing, GatewayAuditEntry, GatewayAuditLog};

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A fresh, empty temp directory unique to this test process.
fn temp_dir() -> PathBuf {
    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("hedos-audit-{}-{unique}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn entry(ts_millis: i64, outcome: &str) -> GatewayAuditEntry {
    GatewayAuditEntry {
        ts_millis,
        client: Some("abc123".to_owned()),
        client_name: Some("Laptop".to_owned()),
        method: "POST".to_owned(),
        route: "/v1/chat/completions".to_owned(),
        model: Some("llama3".to_owned()),
        capability: Some("chat".to_owned()),
        outcome: outcome.to_owned(),
        status: 200,
        duration_ms: 42,
        detail: None,
    }
}

fn unauthorized(ts_millis: i64) -> GatewayAuditEntry {
    GatewayAuditEntry {
        ts_millis,
        client: None,
        client_name: None,
        method: "POST".to_owned(),
        route: "/v1/chat/completions".to_owned(),
        model: None,
        capability: None,
        outcome: "unauthorized".to_owned(),
        status: 401,
        duration_ms: 0,
        detail: None,
    }
}

#[test]
fn entries_append_as_jsonl_and_tail_reads_them_back() {
    let dir = temp_dir();
    let log = GatewayAuditLog::new(&dir);
    log.append(entry(1_600_000_000_000, "ok"));
    log.append(entry(1_600_000_001_000, "ok"));

    let entries = log.tail(10);
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].ts_millis, 1_600_000_000_000);
    assert_eq!(entries[1].ts_millis, 1_600_000_001_000);
    assert_eq!(entries[0].client_name.as_deref(), Some("Laptop"));
}

#[test]
fn the_wire_shape_uses_camel_case_and_an_iso_timestamp() {
    let dir = temp_dir();
    let log = GatewayAuditLog::new(&dir);
    log.append(entry(1_600_000_000_000, "ok"));

    let text = fs::read_to_string(dir.join("audit.jsonl")).unwrap();
    assert!(text.contains("\"ts\":\"2020-09-13T12:26:40Z\""));
    assert!(text.contains("\"clientName\":\"Laptop\""));
    assert!(text.contains("\"durationMs\":42"));
    // A `None` field is omitted, not written as null.
    assert!(!text.contains("\"detail\""));
}

#[test]
fn a_burst_of_unauthorized_requests_coalesces_into_one_summary() {
    let dir = temp_dir();
    let log = GatewayAuditLog::new(&dir);
    // First is written; the next four fall inside the 60s window and coalesce.
    for offset in 0..5 {
        log.append_unauthorized(unauthorized(1_600_000_000_000 + offset * 1_000));
    }
    // A normal entry flushes the pending summary before it is written.
    log.append(entry(1_600_000_010_000, "ok"));

    let entries = log.tail(10);
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].outcome, "unauthorized");
    assert_eq!(
        entries[1].detail.as_deref(),
        Some("4 more unauthenticated requests rejected")
    );
    assert_eq!(entries[2].outcome, "ok");
}

#[test]
fn an_unauthorized_request_after_the_window_starts_a_new_line() {
    let dir = temp_dir();
    let log = GatewayAuditLog::new(&dir);
    log.append_unauthorized(unauthorized(1_600_000_000_000));
    // Past the 60s window: a fresh line, no summary yet (nothing was suppressed).
    log.append_unauthorized(unauthorized(1_600_000_070_000));

    let entries = log.tail(10);
    assert_eq!(entries.len(), 2);
    assert!(entries.iter().all(|entry| entry.outcome == "unauthorized"));
    assert!(entries.iter().all(|entry| entry.detail.is_none()));
}

#[test]
fn flush_writes_a_pending_summary_at_shutdown() {
    let dir = temp_dir();
    let log = GatewayAuditLog::new(&dir);
    for offset in 0..3 {
        log.append_unauthorized(unauthorized(1_600_000_000_000 + offset * 1_000));
    }
    // No further append to flush the window; shutdown must emit the summary.
    log.flush();

    let entries = log.tail(10);
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].outcome, "unauthorized");
    assert_eq!(
        entries[1].detail.as_deref(),
        Some("2 more unauthenticated requests rejected")
    );
}

#[test]
fn the_log_rotates_once_it_exceeds_its_size_bound() {
    let dir = temp_dir();
    let log = GatewayAuditLog::with_max_bytes(&dir, 200);
    for index in 0..40 {
        log.append(entry(1_600_000_000_000 + index * 1_000, "ok"));
    }
    assert!(dir.join("audit.jsonl").exists());
    assert!(dir.join("audit.1.jsonl").exists());
}
