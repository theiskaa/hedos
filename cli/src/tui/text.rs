//! The labels and numbers the screen shows, in their short human forms.

use std::path::Path;

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
    let count = text.chars().count();
    if count <= width {
        return text.to_owned();
    }
    if width < 5 {
        return text.chars().take(width).collect();
    }
    let head = (width - 1) / 2;
    let tail = width - 1 - head;
    let start: String = text.chars().take(head).collect();
    let end: String = text.chars().skip(count - tail).collect();
    format!("{start}…{end}")
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
