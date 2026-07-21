//! `hedos stats` — aggregate statistics from the gateway audit log: request
//! volume, rejection rate, and per-model serving-latency percentiles.

use clap::Args;
use gateway::audit::GatewayAuditLog;
use gateway::stats::{self, GatewayStats, ModelStats};
use runtime::boot::HedosDirs;

use crate::error::CliError;
use crate::support::output::Out;

/// Arguments for `stats`. The machine-readable form is the global `--json` flag.
#[derive(Args)]
pub struct StatsArgs {}

/// Run the `stats` command.
pub async fn run(_args: StatsArgs, out: &Out) -> Result<(), CliError> {
    let audit_dir = HedosDirs::detect().sub("gateway");
    let entries = GatewayAuditLog::new(audit_dir).read_all();
    let summary = stats::summarize(&entries);

    if out.is_json() {
        out.json(&serde_json::to_value(&summary).unwrap_or_default());
        return Ok(());
    }

    if summary.total_requests == 0 {
        out.line("no gateway activity yet — run `hedos serve` and make some requests");
        return Ok(());
    }

    out.line(&header(&summary));
    if !summary.models.is_empty() {
        out.line("");
        out.line(&table(&summary.models));
    }
    Ok(())
}

/// The one-line overall summary, e.g. `142 requests · 9 rejected (6.3%)`.
fn header(summary: &GatewayStats) -> String {
    format!(
        "{} requests · {} rejected ({})",
        summary.total_requests,
        summary.rejected_requests,
        percent(summary.rejection_rate),
    )
}

/// The per-model table, columns aligned to their widest cell.
fn table(models: &[ModelStats]) -> String {
    let headers = ["MODEL", "REQUESTS", "ERRORS", "P50", "P90", "P99"];
    let rows: Vec<[String; 6]> = models
        .iter()
        .map(|model| {
            let [p50, p90, p99] = match &model.latency {
                Some(latency) => [latency.p50, latency.p90, latency.p99].map(millis),
                None => ["—"; 3].map(str::to_owned),
            };
            [
                model.model.clone(),
                model.requests.to_string(),
                errors_cell(model),
                p50,
                p90,
                p99,
            ]
        })
        .collect();

    // Column widths in characters, not bytes, so a multibyte cell like `—` does
    // not over-pad its column — matching `shelf_table`'s rendering.
    let mut widths = headers.map(|header| header.chars().count());
    for row in &rows {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.chars().count());
        }
    }

    let mut lines = vec![render_row(&headers.map(str::to_owned), &widths)];
    for row in &rows {
        lines.push(render_row(row, &widths));
    }
    lines.join("\n")
}

/// One padded, space-separated row. Every column is left-aligned; the trailing
/// column is not padded so there is no dangling whitespace.
fn render_row(cells: &[String; 6], widths: &[usize; 6]) -> String {
    cells
        .iter()
        .enumerate()
        .map(|(index, cell)| {
            if index + 1 == cells.len() {
                cell.clone()
            } else {
                format!("{cell:<width$}", width = widths[index])
            }
        })
        .collect::<Vec<_>>()
        .join("  ")
}

/// The errors cell, e.g. `3 (12.0%)`.
fn errors_cell(model: &ModelStats) -> String {
    format!("{} ({})", model.errors, percent(model.error_rate))
}

/// A ratio in `[0, 1]` as a one-decimal percentage, e.g. `6.3%`.
fn percent(ratio: f64) -> String {
    format!("{:.1}%", ratio * 100.0)
}

/// A latency in milliseconds, e.g. `42ms`.
fn millis(value: i64) -> String {
    format!("{value}ms")
}

#[cfg(test)]
mod tests {
    use super::*;
    use gateway::stats::LatencyPercentiles;

    fn model(
        name: &str,
        requests: u64,
        errors: u64,
        latency: Option<LatencyPercentiles>,
    ) -> ModelStats {
        ModelStats {
            model: name.to_owned(),
            requests,
            errors,
            error_rate: errors as f64 / requests as f64,
            latency,
        }
    }

    #[test]
    fn header_reports_totals_and_rate() {
        let summary = GatewayStats {
            total_requests: 142,
            rejected_requests: 9,
            rejection_rate: 9.0 / 142.0,
            models: Vec::new(),
        };
        assert_eq!(header(&summary), "142 requests · 9 rejected (6.3%)");
    }

    #[test]
    fn table_shows_a_dash_when_latency_is_absent() {
        let rows = vec![model("errs-only", 2, 2, None)];
        let rendered = table(&rows);
        assert!(rendered.contains("MODEL"));
        assert!(rendered.contains("errs-only"));
        assert!(rendered.contains("—"));
        assert!(rendered.contains("2 (100.0%)"));
    }

    #[test]
    fn table_formats_latency_in_millis() {
        let rows = vec![model(
            "m",
            10,
            0,
            Some(LatencyPercentiles {
                p50: 12,
                p90: 34,
                p99: 56,
            }),
        )];
        let rendered = table(&rows);
        assert!(rendered.contains("12ms"));
        assert!(rendered.contains("34ms"));
        assert!(rendered.contains("56ms"));
    }
}
