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
use crate::identity::OK_OUTCOME;

/// A summary of the audit log: overall totals plus a per-model breakdown.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GatewayStats {
    /// Every audited request, including those with no associated model.
    pub total_requests: u64,
    /// Requests whose outcome was anything other than `ok`.
    pub rejected_requests: u64,
    /// [`Self::rejected_requests`] over [`Self::total_requests`]; `0.0` when the
    /// log is empty.
    pub rejection_rate: f64,
    /// Per-model rows, busiest first, ties broken by model name. Requests with no
    /// model (auth failures, model listings, coalesced summaries) are counted in
    /// the totals above but appear in no row here.
    pub models: Vec<ModelStats>,
}

/// One model's slice of the audit log.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ModelStats {
    /// The model id as recorded on the request.
    pub model: String,
    /// Requests served for this model, successful or not.
    pub requests: u64,
    /// Those requests whose outcome was anything other than `ok`.
    pub errors: u64,
    /// [`Self::errors`] over [`Self::requests`].
    pub error_rate: f64,
    /// Serving-latency percentiles over the `ok` requests only, so failed or
    /// rejected requests do not skew the numbers. `None` when the model has no
    /// successful request to measure.
    pub latency: Option<LatencyPercentiles>,
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
}

/// Summarize `entries` into overall totals and a per-model breakdown. Pure over
/// its input: the same slice always yields the same summary, with rows ordered
/// deterministically (busiest first, then by model name).
pub fn summarize(entries: &[GatewayAuditEntry]) -> GatewayStats {
    let total_requests = entries.len() as u64;
    let mut rejected_requests = 0u64;
    let mut per_model: BTreeMap<&str, ModelTally> = BTreeMap::new();

    for entry in entries {
        let ok = entry.outcome == OK_OUTCOME;
        if !ok {
            rejected_requests += 1;
        }
        if let Some(model) = entry.model.as_deref() {
            let tally = per_model.entry(model).or_default();
            tally.requests += 1;
            if ok {
                tally.ok_durations.push(entry.duration_ms);
            } else {
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
    }
}

/// `numerator / denominator` as a ratio, or `0.0` when `denominator` is zero.
fn rate(numerator: u64, denominator: u64) -> f64 {
    if denominator == 0 {
        return 0.0;
    }
    numerator as f64 / denominator as f64
}

/// The p50/p90/p99 of `durations` by nearest-rank, or `None` when empty.
fn percentiles(mut durations: Vec<i64>) -> Option<LatencyPercentiles> {
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
        }
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
