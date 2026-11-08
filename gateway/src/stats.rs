//! Aggregate statistics over the gateway audit log: request volume, rejection
//! rates, and per-model serving-latency percentiles.
//!
//! The audit log records no token counts, so usage here is request-shaped, not
//! token-shaped. A coalesced-unauthorized summary line counts as a single
//! rejection even though its detail reports several suppressed requests; the
//! detail text is never parsed.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::audit::GatewayAuditEntry;

/// A summary of the audit log: overall totals plus a per-model breakdown.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GatewayStats {
    /// Every audited request, including those with no associated model.
    pub total_requests: u64,
    /// Requests whose outcome was anything other than `ok`, apart from the
    /// ones their client abandoned, which nothing rejected.
    pub rejected_requests: u64,
    /// [`Self::rejected_requests`] over [`Self::total_requests`]; `0.0` when the
    /// log is empty.
    pub rejection_rate: f64,
    /// Per-model rows, busiest first, ties broken by model name. Requests with no
    /// model (auth failures, model listings, coalesced summaries) are counted in
    /// the totals above but appear in no row here.
    pub models: Vec<ModelStats>,
    /// What auto routing did, when it served anything.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub routing: Option<RoutingStats>,
}

/// The requests auto routing chose a model for: how many went to each model,
/// and why.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RoutingStats {
    /// Requests that named the virtual model.
    pub requests: u64,
    /// Those that were refused or failed, apart from the ones their client
    /// abandoned.
    pub failed: u64,
    /// How many each model served, busiest first.
    pub models: Vec<Count>,
    /// How many each reason sent where they went, commonest first.
    pub reasons: Vec<Count>,
}

/// A label and how many requests it covers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Count {
    /// The model id or the reason.
    pub label: String,
    /// The requests.
    pub requests: u64,
}

/// One model's slice of the audit log.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ModelStats {
    /// The model id as recorded on the request.
    pub model: String,
    /// Requests served for this model, successful or not.
    pub requests: u64,
    /// Those requests whose outcome was anything other than `ok`, apart from
    /// the ones their client abandoned.
    pub errors: u64,
    /// [`Self::errors`] over [`Self::requests`].
    pub error_rate: f64,
    /// Serving-latency percentiles over the `ok` requests only, so failed or
    /// rejected requests do not skew the numbers. `None` when the model has no
    /// successful request to measure.
    pub latency: Option<LatencyPercentiles>,
    /// When the model was last requested, in Unix milliseconds.
    pub last_seen_millis: i64,
}

/// Serving-latency percentiles in milliseconds, by nearest-rank.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LatencyPercentiles {
    /// The median latency.
    pub p50: i64,
    /// The 90th-percentile latency.
    pub p90: i64,
    /// The 99th-percentile latency.
    pub p99: i64,
}

/// The running tally for a single model before it is finalized.
#[derive(Default)]
struct ModelTally {
    requests: u64,
    errors: u64,
    ok_durations: Vec<i64>,
    last_seen_millis: i64,
}

/// Summarize `entries` into overall totals and a per-model breakdown. Pure over
/// its input: the same slice always yields the same summary, with rows ordered
/// deterministically (busiest first, then by model name).
pub fn summarize(entries: &[GatewayAuditEntry]) -> GatewayStats {
    let total_requests = entries.len() as u64;
    let mut rejected_requests = 0u64;
    let mut per_model: BTreeMap<&str, ModelTally> = BTreeMap::new();
    let mut routed = 0u64;
    let mut routed_failed = 0u64;
    let mut routed_models: BTreeMap<&str, u64> = BTreeMap::new();
    let mut reasons: BTreeMap<&str, u64> = BTreeMap::new();

    for entry in entries {
        let ok = entry.is_ok();
        let failed = !ok && !entry.is_cancelled();
        if failed {
            rejected_requests += 1;
        }
        if entry.routed_from.is_some() {
            routed += 1;
            if failed {
                routed_failed += 1;
            }
            if let Some(model) = entry.model.as_deref() {
                *routed_models.entry(model).or_default() += 1;
            }
            if let Some(reason) = entry.route_reason.as_deref() {
                *reasons.entry(reason).or_default() += 1;
            }
        }
        if let Some(model) = entry.model.as_deref() {
            let tally = per_model.entry(model).or_default();
            tally.requests += 1;
            tally.last_seen_millis = tally.last_seen_millis.max(entry.ts_millis);
            if ok {
                tally.ok_durations.push(entry.duration_ms);
            } else if failed {
                tally.errors += 1;
            }
        }
    }

    let mut models: Vec<ModelStats> = per_model
        .into_iter()
        .map(|(model, tally)| ModelStats {
            model: model.to_owned(),
            requests: tally.requests,
            errors: tally.errors,
            error_rate: rate(tally.errors, tally.requests),
            latency: percentiles(tally.ok_durations),
            last_seen_millis: tally.last_seen_millis,
        })
        .collect();
    // Busiest first; the name is the tie-break, and since the source is a
    // `BTreeMap` the names already ascend, so a stable sort on requests suffices.
    models.sort_by_key(|model| std::cmp::Reverse(model.requests));

    GatewayStats {
        total_requests,
        rejected_requests,
        rejection_rate: rate(rejected_requests, total_requests),
        models,
        routing: (routed > 0).then(|| RoutingStats {
            requests: routed,
            failed: routed_failed,
            models: busiest(routed_models),
            reasons: busiest(reasons),
        }),
    }
}

/// `counts` as rows, busiest first; the labels already ascend, so a stable
/// sort leaves ties in name order.
fn busiest(counts: BTreeMap<&str, u64>) -> Vec<Count> {
    let mut rows: Vec<Count> = counts
        .into_iter()
        .map(|(label, requests)| Count {
            label: label.to_owned(),
            requests,
        })
        .collect();
    rows.sort_by_key(|row| std::cmp::Reverse(row.requests));
    rows
}

/// `numerator / denominator` as a ratio, or `0.0` when `denominator` is zero.
fn rate(numerator: u64, denominator: u64) -> f64 {
    if denominator == 0 {
        return 0.0;
    }
    numerator as f64 / denominator as f64
}

/// The p50/p90/p99 of `durations` by nearest-rank, or `None` when empty.
pub fn percentiles(mut durations: Vec<i64>) -> Option<LatencyPercentiles> {
    if durations.is_empty() {
        return None;
    }
    durations.sort_unstable();
    Some(LatencyPercentiles {
        p50: percentile(&durations, 50),
        p90: percentile(&durations, 90),
        p99: percentile(&durations, 99),
    })
}

/// The `pct`th percentile of a non-empty, ascending `sorted` slice by
/// nearest-rank: rank `ceil(pct/100 · n)`, clamped into range.
fn percentile(sorted: &[i64], pct: u32) -> i64 {
    debug_assert!(!sorted.is_empty(), "percentile requires a non-empty slice");
    let n = sorted.len();
    let rank = (pct as usize * n).div_ceil(100);
    let index = rank.saturating_sub(1).min(n - 1);
    sorted[index]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(model: Option<&str>, outcome: &str, duration_ms: i64) -> GatewayAuditEntry {
        GatewayAuditEntry {
            ts_millis: 0,
            client: None,
            client_name: None,
            method: "POST".to_owned(),
            route: "/v1/chat/completions".to_owned(),
            model: model.map(str::to_owned),
            capability: Some("chat".to_owned()),
            outcome: outcome.to_owned(),
            status: if outcome == "ok" { 200 } else { 500 },
            duration_ms,
            detail: None,
            routed_from: None,
            route_reason: None,
        }
    }

    fn routed(model: &str, reason: &str) -> GatewayAuditEntry {
        GatewayAuditEntry {
            routed_from: Some("auto".to_owned()),
            route_reason: Some(reason.to_owned()),
            ..entry(Some(model), "ok", 10)
        }
    }

    #[test]
    fn routed_requests_are_counted_by_model_and_reason() {
        let entries = [
            routed("tiny", "short turn"),
            routed("tiny", "short turn"),
            routed("big", "hard word \"refactor\""),
            entry(Some("big"), "ok", 10),
            entry(None, "unauthorized", 0),
            GatewayAuditEntry {
                routed_from: Some("auto".to_owned()),
                ..entry(None, "bad_request", 0)
            },
        ];
        let summary = summarize(&entries);
        let routing = summary.routing.expect("something was routed");
        assert_eq!(routing.requests, 4);
        assert_eq!(routing.failed, 1);
        let models: Vec<(&str, u64)> = routing
            .models
            .iter()
            .map(|row| (row.label.as_str(), row.requests))
            .collect();
        assert_eq!(models, [("tiny", 2), ("big", 1)]);
        assert_eq!(routing.reasons[0].label, "short turn");
        assert_eq!(routing.reasons[0].requests, 2);
        assert_eq!(
            summary.models[0].requests, 2,
            "per-model rows count every request"
        );
    }

    #[test]
    fn nothing_routed_leaves_routing_out() {
        let summary = summarize(&[entry(Some("big"), "ok", 10)]);
        assert!(summary.routing.is_none());
        let value = serde_json::to_value(&summary).unwrap();
        assert!(value.get("routing").is_none());
    }

    #[test]
    fn an_old_log_line_without_routing_fields_still_reads() {
        let line = r#"{"ts":"2026-11-01T00:00:00Z","method":"POST","route":"/v1/chat/completions","model":"m","outcome":"ok","status":200,"durationMs":5}"#;
        let entry: GatewayAuditEntry = serde_json::from_str(line).unwrap();
        assert_eq!(entry.routed_from, None);
        assert_eq!(entry.route_reason, None);
    }

    #[test]
    fn percentile_nearest_rank_holds_at_small_n() {
        assert_eq!(percentile(&[7], 50), 7);
        assert_eq!(percentile(&[7], 99), 7);
        // Two samples: p50 falls on the lower, p99 on the upper.
        assert_eq!(percentile(&[1, 2], 50), 1);
        assert_eq!(percentile(&[1, 2], 99), 2);
        // p99 of three reaches the top element by nearest-rank.
        assert_eq!(percentile(&[1, 2, 3], 99), 3);
    }

    #[test]
    fn empty_input_is_all_zeroes() {
        let stats = summarize(&[]);
        assert_eq!(stats.total_requests, 0);
        assert_eq!(stats.rejected_requests, 0);
        assert_eq!(stats.rejection_rate, 0.0);
        assert!(stats.models.is_empty());
    }

    #[test]
    fn an_abandoned_request_is_neither_rejected_nor_an_error() {
        let entries = vec![
            entry(Some("nomic"), "ok", 10),
            entry(Some("nomic"), "cancelled", 2000),
            entry(None, "cancelled", 2000),
        ];
        let stats = summarize(&entries);
        assert_eq!(stats.total_requests, 3);
        assert_eq!(stats.rejected_requests, 0);
        assert_eq!(stats.models[0].requests, 2);
        assert_eq!(stats.models[0].errors, 0);
        assert_eq!(stats.models[0].latency.as_ref().map(|l| l.p99), Some(10));
    }

    #[test]
    fn totals_count_every_entry_including_modelless_ones() {
        let entries = vec![
            entry(Some("llama3"), "ok", 10),
            entry(Some("llama3"), "ok", 20),
            entry(Some("qwen3"), "error", 5),
            entry(None, "unauthorized", 0),
        ];
        let stats = summarize(&entries);
        assert_eq!(stats.total_requests, 4);
        // The qwen3 error and the modelless unauthorized entry are both rejections.
        assert_eq!(stats.rejected_requests, 2);
        assert!((stats.rejection_rate - 0.5).abs() < f64::EPSILON);
        // The modelless entry produced no row.
        assert_eq!(stats.models.len(), 2);
    }

    #[test]
    fn per_model_counts_errors_and_rates() {
        let entries = vec![
            entry(Some("qwen3"), "ok", 30),
            entry(Some("qwen3"), "timeout", 40),
            entry(Some("qwen3"), "ok", 50),
        ];
        let stats = summarize(&entries);
        let row = &stats.models[0];
        assert_eq!(row.model, "qwen3");
        assert_eq!(row.requests, 3);
        assert_eq!(row.errors, 1);
        assert!((row.error_rate - (1.0 / 3.0)).abs() < 1e-9);
    }

    #[test]
    fn latency_covers_ok_requests_only() {
        // The one non-ok entry has a wild duration that must not enter the sample.
        let mut entries = vec![entry(Some("m"), "timeout", 9_999)];
        entries.extend((1..=100).map(|d| entry(Some("m"), "ok", d)));
        let latency = summarize(&entries).models[0]
            .latency
            .clone()
            .expect("ok samples yield percentiles");
        assert_eq!(latency.p50, 50);
        assert_eq!(latency.p90, 90);
        assert_eq!(latency.p99, 99);
    }

    #[test]
    fn a_single_sample_is_all_three_percentiles() {
        let stats = summarize(&[entry(Some("m"), "ok", 7)]);
        let latency = stats.models[0].latency.clone().expect("one ok sample");
        assert_eq!(
            latency,
            LatencyPercentiles {
                p50: 7,
                p90: 7,
                p99: 7
            }
        );
    }

    #[test]
    fn a_model_with_no_ok_requests_has_no_latency() {
        let stats = summarize(&[entry(Some("m"), "error", 3)]);
        assert!(stats.models[0].latency.is_none());
    }

    #[test]
    fn last_seen_is_the_newest_request_whatever_its_outcome() {
        let mut entries = [
            entry(Some("a"), "ok", 10),
            entry(Some("a"), "error", 10),
            entry(Some("a"), "ok", 10),
        ];
        entries[0].ts_millis = 1_000;
        entries[1].ts_millis = 5_000;
        entries[2].ts_millis = 3_000;
        assert_eq!(summarize(&entries).models[0].last_seen_millis, 5_000);
    }

    #[test]
    fn rows_are_ordered_by_requests_then_name() {
        let entries = vec![
            entry(Some("busy"), "ok", 1),
            entry(Some("busy"), "ok", 1),
            entry(Some("abc"), "ok", 1),
            entry(Some("xyz"), "ok", 1),
        ];
        let models = summarize(&entries).models;
        let order: Vec<&str> = models.iter().map(|m| m.model.as_str()).collect();
        // `busy` leads on volume; the two single-request rows break the tie by name.
        assert_eq!(order, ["busy", "abc", "xyz"]);
    }
}
