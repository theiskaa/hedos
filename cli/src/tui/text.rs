//! The labels and numbers the screen shows, in their short human forms.

use std::path::Path;

use kernel::capabilities::GenerationStats;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use kernel::records::byte_format::{BYTES_PER_GIB, format_bytes, one_decimal};
const MINUTE: i64 = 60;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;

/// Bytes as `4.7 GB` / `512 MB`.
pub fn bytes(bytes: i64) -> String {
    format_bytes(bytes)
}

/// Bytes in gibibytes with one decimal, for memory figures set against a
/// machine total: `14.2`. Negative counts read as zero.
pub fn gib(bytes: i64) -> String {
    one_decimal(bytes.max(0) as f64 / BYTES_PER_GIB as f64)
}

/// A duration in seconds as its largest whole unit: `45s`, `26m`, `3h`, `2d`.
pub fn duration(seconds: i64) -> String {
    let seconds = seconds.max(0);
    if seconds >= DAY {
        format!("{}d", seconds / DAY)
    } else if seconds >= HOUR {
        format!("{}h", seconds / HOUR)
    } else if seconds >= MINUTE {
        format!("{}m", seconds / MINUTE)
    } else {
        format!("{seconds}s")
    }
}

/// `buckets` as one bar per bucket, scaled to the largest; a flat line when
/// every bucket is empty.
pub fn sparkline(buckets: &[u32]) -> String {
    const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let top = buckets.iter().copied().max().unwrap_or(0).max(1) as f64;
    buckets
        .iter()
        .map(|&count| {
            let level = ((count as f64 / top) * (BARS.len() - 1) as f64).round() as usize;
            BARS[level.min(BARS.len() - 1)]
        })
        .collect()
}

/// A runtime id as the shelf shows it: the sidecar prefix and long vendor
/// names carry nothing at a glance.
pub fn short_runtime(id: &str) -> &str {
    match id {
        "apple-foundation" => "apple",
        other => other.strip_prefix("python:").unwrap_or(other),
    }
}

/// A store kind as the shelf shows it.
pub fn short_store(kind: &str) -> &str {
    match kind {
        "huggingface-cache" => "hf",
        "lm-studio" => "lm studio",
        other => other,
    }
}

/// `path` with the home directory written as `~`, matched on whole path
/// components so `/Users/ab` never turns into `~b`.
pub fn home_relative(path: &str, home: Option<&Path>) -> String {
    match home.and_then(|home| Path::new(path).strip_prefix(home).ok()) {
        Some(rest) if home.is_some_and(|home| home.as_os_str().len() > 1) => {
            format!("~/{}", rest.display())
        }
        _ => path.to_owned(),
    }
}

/// `text` cut to `width` cells by dropping its middle, so a path keeps both
/// its root and its file name.
pub fn elide_middle(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_owned();
    }
    let graphemes: Vec<&str> = text.graphemes(true).collect();
    if width < 5 {
        return take_cells(graphemes.iter().copied(), width);
    }
    let head = take_cells(graphemes.iter().copied(), (width - 1) / 2);
    let tail = take_cells(graphemes.iter().rev().copied(), width - 1 - head.width());
    let tail: String = tail.graphemes(true).rev().collect();
    format!("{head}…{tail}")
}

/// `text` cut to `width` cells from the tail, with `…` where it was cut, for
/// a value whose start carries the meaning.
pub fn clip(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_owned();
    }
    if width < 2 {
        return take_cells(text.graphemes(true), width);
    }
    format!("{}…", take_cells(text.graphemes(true), width - 1))
}

/// A count in its shortest readable form: `987`, `1.5k`, `45k`, `1.2M`.
pub fn compact(count: i64) -> String {
    const THOUSAND: f64 = 1000.0;
    let count = count.max(0);
    let scaled = |value: f64, unit: &str| {
        if value >= 10.0 {
            format!("{}{unit}", value.round() as i64)
        } else {
            format!("{}{unit}", one_decimal(value))
        }
    };
    if count >= 999_500 {
        scaled(count as f64 / (THOUSAND * THOUSAND), "M")
    } else if count >= 1000 {
        scaled(count as f64 / THOUSAND, "k")
    } else {
        count.to_string()
    }
}

/// The leading graphemes of `graphemes` that fit in `width` cells.
fn take_cells<'a>(graphemes: impl Iterator<Item = &'a str>, width: usize) -> String {
    let mut used = 0;
    graphemes
        .take_while(|grapheme| {
            let fits = used + grapheme.width() <= width;
            if fits {
                used += grapheme.width();
            }
            fits
        })
        .collect()
}

/// `~120 tokens · 40 tok/s · first in 0.4s`, from whatever a reply reported.
pub fn stats(stats: &GenerationStats) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(tokens) = stats.completion_tokens {
        let estimated = if stats.token_counts_estimated {
            "~"
        } else {
            ""
        };
        parts.push(format!("{estimated}{tokens} tokens"));
        if let Some(ms) = stats.duration_ms.filter(|ms| *ms > 0) {
            parts.push(format!("{:.0} tok/s", tokens as f64 * 1000.0 / ms as f64));
        }
    }
    if let Some(ms) = stats.ttft_ms {
        parts.push(format!("first in {:.1}s", ms as f64 / 1000.0));
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// A context length as `4k`, `32k`, `128k`, or the plain count under 1000.
pub fn tokens(count: i64) -> String {
    if count >= 1000 {
        format!("{}k", count / 1000)
    } else {
        count.to_string()
    }
}

/// A count with a noun that takes a plain `s` plural: `1 model`, `12 models`.
pub fn count(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elide_middle_budgets_cells_not_characters() {
        assert_eq!(elide_middle("abcdefghij", 7), "abc…hij");
        assert_eq!(
            elide_middle("日本語のモデル/ファイル名.gguf", 10),
            "日本….gguf"
        );
        assert!(elide_middle("日本語のモデル/ファイル名.gguf", 10).width() <= 10);
        assert_eq!(elide_middle("short", 10), "short");
        assert_eq!(elide_middle("abcdef", 3), "abc");
    }

    #[test]
    fn stats_read_as_one_dim_line() {
        let reported = GenerationStats {
            completion_tokens: Some(120),
            duration_ms: Some(3000),
            ttft_ms: Some(420),
            ..GenerationStats::default()
        };
        assert_eq!(
            stats(&reported).as_deref(),
            Some("120 tokens · 40 tok/s · first in 0.4s")
        );
        let estimated = GenerationStats {
            completion_tokens: Some(7),
            duration_ms: Some(0),
            token_counts_estimated: true,
            ..GenerationStats::default()
        };
        assert_eq!(stats(&estimated).as_deref(), Some("~7 tokens"));
        let first_only = GenerationStats {
            ttft_ms: Some(1500),
            ..GenerationStats::default()
        };
        assert_eq!(stats(&first_only).as_deref(), Some("first in 1.5s"));
        assert_eq!(stats(&GenerationStats::default()), None);
    }

    #[test]
    fn gib_keeps_one_decimal_and_trims_zero() {
        assert_eq!(gib(64 * (1 << 30)), "64");
        assert_eq!(gib(14_200_000_000), "13.2");
        assert_eq!(gib(0), "0");
        assert_eq!(gib(-1), "0");
    }

    #[test]
    fn durations_pick_the_largest_whole_unit() {
        assert_eq!(duration(45), "45s");
        assert_eq!(duration(26 * 60 + 30), "26m");
        assert_eq!(duration(3 * 3600), "3h");
        assert_eq!(duration(2 * 86_400 + 5), "2d");
        assert_eq!(duration(-5), "0s");
    }

    #[test]
    fn sparklines_scale_to_the_busiest_bucket() {
        assert_eq!(sparkline(&[0, 0, 0]), "▁▁▁");
        assert_eq!(sparkline(&[1, 4, 8]), "▂▅█");
        assert_eq!(sparkline(&[]), "");
    }

    #[test]
    fn labels_shorten_what_carries_nothing() {
        assert_eq!(short_runtime("python:mlx-lm"), "mlx-lm");
        assert_eq!(short_runtime("apple-foundation"), "apple");
        assert_eq!(short_runtime("llama-cpp"), "llama-cpp");
        assert_eq!(short_store("huggingface-cache"), "hf");
        assert_eq!(short_store("ollama"), "ollama");
    }

    #[test]
    fn home_is_contracted_by_component() {
        let home = Path::new("/Users/theis");
        assert_eq!(
            home_relative("/Users/theis/.ollama/x", Some(home)),
            "~/.ollama/x"
        );
        assert_eq!(
            home_relative("/Users/theiskaa/.ollama/x", Some(home)),
            "/Users/theiskaa/.ollama/x"
        );
        assert_eq!(home_relative("/etc/x", None), "/etc/x");
        assert_eq!(home_relative("/x", Some(Path::new("/"))), "/x");
    }

    #[test]
    fn eliding_keeps_both_ends() {
        assert_eq!(elide_middle("short", 10), "short");
        assert_eq!(
            elide_middle("/a/very/long/path/file.gguf", 15),
            "/a/very…le.gguf"
        );
        assert_eq!(elide_middle("abcdef", 3), "abc");
    }

    #[test]
    fn compact_reads_in_thousands() {
        assert_eq!(compact(987), "987");
        assert_eq!(compact(1_500), "1.5k");
        assert_eq!(compact(45_312), "45k");
        assert_eq!(compact(1_234_567), "1.2M");
        assert_eq!(compact(999_950), "1M");
        assert_eq!(compact(-3), "0");
    }

    #[test]
    fn clipping_keeps_the_head() {
        assert_eq!(clip("short", 10), "short");
        assert_eq!(clip("chat, complete, embed", 12), "chat, compl…");
        assert_eq!(clip("日本語のモデル", 5), "日本…");
        assert_eq!(clip("abc", 1), "a");
    }

    #[test]
    fn tokens_read_in_thousands() {
        assert_eq!(tokens(4096), "4k");
        assert_eq!(tokens(131_072), "131k");
        assert_eq!(tokens(512), "512");
    }

    #[test]
    fn counts_pluralize() {
        assert_eq!(count(1, "model"), "1 model");
        assert_eq!(count(0, "model"), "0 models");
    }
}
