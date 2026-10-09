//! The labels and numbers the screen shows, in their short human forms.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use kernel::install::event::InstallProgress;
use kernel::machine::{Fit, Machine};
use kernel::profiles::FitVerdict;
use kernel::records::ModelRecord;
use kernel::records::byte_format::{format_bytes, one_decimal};

use crate::support::shelf_table::verdict_label;
pub use crate::support::text::{clip, count, elide_middle, gib, gib_short, short_runtime};

/// Bytes as `4.7 GB` / `512 MB`.
pub fn bytes(bytes: i64) -> String {
    format_bytes(bytes)
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

/// `path` with this user's home written as `~`, read from `HOME` once.
pub fn at_home(path: &str) -> String {
    static HOME: OnceLock<Option<PathBuf>> = OnceLock::new();
    let home = HOME.get_or_init(|| std::env::var_os("HOME").map(PathBuf::from));
    home_relative(path, home.as_deref())
}

/// `fits · needs 4.7 of 51.8 GiB`, `too big for this machine`, or that the
/// footprint is unknown, and when the model fits at all, how. Judged on
/// `machine` under the engine `record` resolved to, against what that engine
/// may give it.
pub fn fit_parts(record: &ModelRecord, machine: &Machine) -> (String, Option<Fit>) {
    match machine.fit_record(record) {
        None => ("unknown footprint".to_owned(), None),
        Some(fit) if fit.assessment.verdict == FitVerdict::TooLarge => {
            ("too big for this machine".to_owned(), None)
        }
        Some(fit) => (
            format!(
                "{} · needs {} of {} GiB",
                verdict_label(Some(fit.assessment.verdict)),
                gib(fit.assessment.required_bytes),
                gib(fit.budget_bytes as i64)
            ),
            Some(fit),
        ),
    }
}

/// What a pull has on disk: `125 MB of 468 MB` against a firm total, `125 MB
/// so far` when the total is only an estimate, nothing when nothing has
/// landed.
pub fn landed(progress: &InstallProgress) -> Option<String> {
    if progress.bytes_downloaded <= 0 {
        return None;
    }
    let done = bytes(progress.bytes_downloaded);
    Some(match (progress.fraction(), progress.total_bytes) {
        (Some(_), Some(total)) => format!("{done} of {}", bytes(total)),
        _ => format!("{done} so far"),
    })
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

/// A context length as `4k`, `32k`, `128k`, or the plain count under 1000.
pub fn tokens(count: i64) -> String {
    if count >= 1000 {
        format!("{}k", count / 1000)
    } else {
        count.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use unicode_width::UnicodeWidthStr;

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
    fn gib_keeps_one_decimal_and_trims_zero() {
        assert_eq!(gib(64 * (1 << 30)), "64");
        assert_eq!(gib(14_200_000_000), "13.2");
        assert_eq!(gib(0), "0");
        assert_eq!(gib(-1), "0");
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
