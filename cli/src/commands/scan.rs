//! `hedos scan`: discover models on this machine and refresh the shelf, then
//! say how they split across the stores, which models are copies of weights
//! another model keeps, and how many are too big for this machine.

use std::collections::{BTreeSet, HashMap};

use clap::Args;
use kernel::discovery::{
    DiscoverySummary, DuplicateCopy, DuplicateGroup, DuplicateMember, KindStat, RemovableCopy,
};
use kernel::profiles::FitTally;
use kernel::records::{ModelRecord, SourceKind, format_bytes};
use serde_json::{Value, json};
use unicode_width::UnicodeWidthStr;

use crate::error::CliError;
use crate::support::machine;
use crate::support::output::Out;
use crate::support::session::Session;
use crate::support::table;
use crate::support::text;

/// Arguments for `scan`.
#[derive(Args)]
pub struct ScanArgs {}

/// Run the `scan` command.
pub async fn run(_args: ScanArgs, out: &Out) -> Result<(), CliError> {
    let session = Session::open()?;
    let summary = session.discover().await?;
    // Judged on the shelf's records, so a verdict matches the FIT column of
    // `hedos ls`, but only for the models this scan found.
    let memory_bytes = machine::memory_budget_bytes();
    let shelf = session.shelf().await;
    let fit = FitTally::over(
        shelf
            .iter()
            .filter(|record| summary.found_ids.contains(&record.id)),
        memory_bytes,
    );
    for issue in &summary.issues {
        for line in issue_lines(issue) {
            out.err(&line);
        }
    }
    out.line(&summary.headline());
    for line in details(&summary, &shelf, &fit, memory_bytes) {
        out.line(&line);
    }
    out.json(&scan_json(&summary, &fit, memory_bytes));
    Ok(())
}

/// `issue: ...` and then each further line of a multi-line issue (a TOML
/// parse error's caret diagram) as it is, every line escaped.
fn issue_lines(issue: &str) -> Vec<String> {
    issue
        .trim_end_matches(['\r', '\n'])
        .split('\n')
        .map(|line| text::printable(line.strip_suffix('\r').unwrap_or(line)))
        .enumerate()
        .map(|(index, line)| {
            if index == 0 {
                format!("issue: {line}")
            } else {
                line.into_owned()
            }
        })
        .collect()
}

/// The lines printed after the headline: the per-store split, the duplicate
/// groups, and the too-large count, each paragraph after a blank line and
/// only when it has something to say. Empty when the scan found nothing,
/// whatever the tally holds.
fn details(
    summary: &DiscoverySummary,
    shelf: &[ModelRecord],
    fit: &FitTally,
    memory_bytes: u64,
) -> Vec<String> {
    let mut lines = Vec::new();
    if summary.total_count == 0 {
        return lines;
    }
    let stores = summary.stores();
    if !stores.is_empty() {
        lines.push(String::new());
        lines.extend(store_lines(&stores));
    }
    if !summary.duplicates.is_empty() {
        lines.push(String::new());
        lines.extend(duplicate_lines(summary, shelf));
    }
    if fit.too_large > 0 {
        lines.push(String::new());
        let (verb, pronoun) = if fit.too_large == 1 {
            ("is", "it")
        } else {
            ("are", "them")
        };
        lines.push(format!(
            "{} {verb} too big for this machine's {} GiB. `hedos ls` marks {pronoun}.",
            text::count(fit.too_large, "model"),
            text::gib(memory_bytes as i64),
        ));
    }
    lines
}

/// `ollama  6  28.4 GB`, one aligned row per store, the size left off a
/// store whose models measure nothing on disk.
fn store_lines(stores: &[(SourceKind, KindStat)]) -> Vec<String> {
    let rows: Vec<Vec<String>> = stores
        .iter()
        .map(|(kind, stat)| {
            let size = if stat.bytes > 0 {
                format_bytes(stat.bytes)
            } else {
                String::new()
            };
            vec![kind.as_str().to_owned(), stat.count.to_string(), size]
        })
        .collect();
    let widths = table::widths(&rows, None);
    rows.iter()
        .map(|row| {
            format!(
                "{}  {}  {}",
                text::padded(&row[0], widths[0]),
                text::right_aligned(&row[1], widths[1]),
                text::right_aligned(&row[2], widths[2]),
            )
            .trim_end()
            .to_owned()
        })
        .collect()
}

/// The duplicate paragraph: a sentence with what removing every copy that
/// can go frees, each file counted once, then each group from the copy kept:
/// a `keep` row naming it, and a row per copy that can go with the bytes
/// removing that copy alone frees. The two differ when copies share a file
/// through a hard link, which goes only with the last of them, so the
/// sentence says which is which once there are several rows. The total says
/// "up to": a clone on a copy-on-write disk shares its blocks, so deleting it
/// frees less than its size. When a copy has more than one name, or is only
/// a link, a closing line says what removing a name frees.
fn duplicate_lines(summary: &DiscoverySummary, shelf: &[ModelRecord]) -> Vec<String> {
    let groups = &summary.duplicates;
    let rows = groups
        .iter()
        .map(|group| group.removable.len())
        .sum::<usize>();
    let models = groups
        .iter()
        .flat_map(|group| &group.removable)
        .map(|copy| 1 + copy.copy.aliases.len())
        .sum::<usize>();
    let subject = if models == 1 {
        "1 model is a copy".to_owned()
    } else {
        format!("{models} models are copies")
    };
    let keeper = if groups.len() == 1 {
        "another model keeps"
    } else {
        "other models keep"
    };
    let total = format_bytes(summary.reclaimable_bytes);
    let frees = match (models, rows) {
        (1, _) => format!("Removing it frees up to {total}:"),
        (_, 1) => format!("Removing them frees up to {total}:"),
        _ => format!(
            "Removing all of them frees up to {total}, each file counted once; a row says what removing that copy alone frees:"
        ),
    };
    let mut lines = vec![format!(
        "{subject} of weights {keeper} (identical in size and sampled content). {frees}"
    )];
    let width = groups
        .iter()
        .flat_map(|group| &group.removable)
        .map(|copy| format_bytes(copy.reclaimable_bytes).width())
        .max()
        .unwrap_or(0);
    let names = Names::over(shelf, groups);
    for group in groups {
        lines.push(format!("  keep {}", copy_label(&group.kept, &names)));
        for copy in &group.removable {
            lines.push(format!(
                "    {}  {}",
                text::right_aligned(&format_bytes(copy.reclaimable_bytes), width),
                copy_label(&copy.copy, &names)
            ));
        }
    }
    let copies = groups.iter().flat_map(copies_of);
    if copies.clone().any(|copy| !copy.aliases.is_empty()) {
        lines.push(
            "  A name in brackets reaches the same files as the copy before it. Removing all of them frees the space; removing one alone can free nothing while another keeps the files (a hard link, a second Ollama tag), and removing a symlink frees nothing."
                .to_owned(),
        );
    } else if copies.clone().any(|copy| copy.link_target.is_some()) {
        lines.push(
            "  Removing a symlink frees nothing: the file it points to holds the bytes.".to_owned(),
        );
    }
    lines
}

/// The copy `group` keeps, then each copy it offers.
fn copies_of(group: &DuplicateGroup) -> impl Iterator<Item = &DuplicateCopy> + Clone {
    std::iter::once(&group.kept).chain(group.removable.iter().map(|copy| &copy.copy))
}

/// The models each name could mean, matched the way `hedos rm` matches one
/// (a model's shown name or its own, ASCII case ignored), over the shelf and
/// the paragraph, so a name that means more than one (two loose files both
/// named `model`) also gets its path.
struct Names<'a>(HashMap<String, BTreeSet<&'a str>>);

impl<'a> Names<'a> {
    fn over(shelf: &'a [ModelRecord], groups: &'a [DuplicateGroup]) -> Self {
        let mut meant: HashMap<String, BTreeSet<&str>> = HashMap::new();
        let mut mean = |name: &str, id: &'a str| {
            meant
                .entry(name.to_ascii_lowercase())
                .or_default()
                .insert(id);
        };
        for record in shelf {
            mean(record.display_name(), &record.id);
            mean(&record.name, &record.id);
        }
        for copy in groups.iter().flat_map(copies_of) {
            for member in std::iter::once(&copy.member).chain(&copy.aliases) {
                mean(&member.name, &member.id);
            }
        }
        Self(meant)
    }

    /// `qwen2.5:7b (ollama)`, or with the path when the name means more than
    /// one model.
    fn label(&self, member: &DuplicateMember, link_target: Option<&str>) -> String {
        let name = text::printable(&member.name);
        let store = member.kind.as_str();
        let ambiguous = self
            .0
            .get(&member.name.to_ascii_lowercase())
            .is_some_and(|ids| ids.len() > 1);
        let mut detail = vec![store.to_owned()];
        if ambiguous {
            detail.push(text::printable(&member.path).into_owned());
        }
        if let Some(target) = link_target {
            detail.push(format!("a link to {}", text::printable(target)));
        }
        format!("{name} ({})", detail.join(", "))
    }
}

/// `qwen2.5:7b (ollama) [same files: qwen2.5:latest (ollama)]`.
fn copy_label(copy: &DuplicateCopy, names: &Names) -> String {
    let label = names.label(&copy.member, copy.link_target.as_deref());
    if copy.aliases.is_empty() {
        return label;
    }
    let aliases: Vec<String> = copy
        .aliases
        .iter()
        .map(|alias| names.label(alias, None))
        .collect();
    format!("{label} [same files: {}]", aliases.join(", "))
}

fn member_json(member: &DuplicateMember) -> Value {
    json!({
        "id": member.id,
        "name": member.name,
        "store": member.kind.as_str(),
        "path": member.path,
    })
}

fn copy_json(copy: &DuplicateCopy) -> Value {
    let mut value = member_json(&copy.member);
    value["aliases"] = copy.aliases.iter().map(member_json).collect();
    value["linkTarget"] = json!(copy.link_target);
    value
}

fn removable_json(copy: &RemovableCopy) -> Value {
    let mut value = copy_json(&copy.copy);
    value["reclaimableBytes"] = json!(copy.reclaimable_bytes);
    value
}

/// The `--json` object: the keys the command always emitted (`totalCount`,
/// `headline`, `issues`) beside the split, the duplicates, the fit tally and
/// the stores whose scan failed.
fn scan_json(summary: &DiscoverySummary, fit: &FitTally, memory_bytes: u64) -> Value {
    let stores: Vec<Value> = summary
        .stores()
        .into_iter()
        .map(|(kind, stat)| {
            json!({
                "store": kind.as_str(),
                "count": stat.count,
                "bytes": stat.bytes,
            })
        })
        .collect();
    let duplicates: Vec<Value> = summary
        .duplicates
        .iter()
        .map(|group| {
            json!({
                "reclaimableBytes": group.reclaimable_bytes,
                "kept": copy_json(&group.kept),
                "removable": group.removable.iter().map(removable_json).collect::<Vec<_>>(),
            })
        })
        .collect();
    let failed: Vec<&str> = summary
        .failed_kinds
        .iter()
        .map(|kind| kind.as_str())
        .collect();
    json!({
        "totalCount": summary.total_count,
        "totalBytes": summary.total_bytes,
        "headline": summary.headline(),
        "stores": stores,
        "duplicates": duplicates,
        "reclaimableBytes": summary.reclaimable_bytes,
        "fit": {
            "runsWell": fit.runs_well,
            "tightFit": fit.tight_fit,
            "tooLarge": fit.too_large,
            "unknown": fit.unknown,
        },
        "memoryBytes": memory_bytes,
        "failedStores": failed,
        "issues": summary.issues,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1 << 30;

    fn lines_for(summary: &DiscoverySummary, fit: &FitTally, memory_bytes: u64) -> Vec<String> {
        details(summary, &[], fit, memory_bytes)
    }

    fn summary(stores: &[(SourceKind, usize, i64)]) -> DiscoverySummary {
        let mut summary = DiscoverySummary::default();
        for (kind, count, bytes) in stores {
            summary.per_kind.insert(
                kind.clone(),
                KindStat {
                    count: *count,
                    bytes: *bytes,
                },
            );
            summary.total_count += count;
            summary.total_bytes += bytes;
        }
        summary
    }

    fn member(name: &str, kind: SourceKind, path: &str) -> DuplicateMember {
        DuplicateMember {
            id: format!("id-{name}@{path}"),
            name: name.to_owned(),
            kind,
            path: path.to_owned(),
        }
    }

    fn copy(member: DuplicateMember) -> DuplicateCopy {
        DuplicateCopy {
            member,
            aliases: Vec::new(),
            link_target: None,
        }
    }

    fn group(size_bytes: i64, members: Vec<DuplicateMember>) -> DuplicateGroup {
        group_of(size_bytes, members.into_iter().map(copy).collect())
    }

    /// The first of `copies` kept, and each other offered for `size_bytes`.
    fn group_of(size_bytes: i64, copies: Vec<DuplicateCopy>) -> DuplicateGroup {
        let mut copies = copies.into_iter();
        let kept = copies.next().unwrap();
        let removable: Vec<RemovableCopy> = copies
            .map(|copy| RemovableCopy {
                copy,
                reclaimable_bytes: size_bytes,
            })
            .collect();
        DuplicateGroup {
            reclaimable_bytes: removable.len() as i64 * size_bytes,
            kept,
            removable,
        }
    }

    fn total(groups: &[DuplicateGroup]) -> i64 {
        groups.iter().map(|group| group.reclaimable_bytes).sum()
    }

    fn shelf() -> DiscoverySummary {
        let mut summary = summary(&[
            (SourceKind::lm_studio(), 2, 10_900_000_000),
            (SourceKind::ollama(), 6, 28_400_000_000),
            (SourceKind::builtin(), 1, 0),
        ]);
        summary.duplicates = vec![
            group(
                4_700_000_000,
                vec![
                    member("qwen2.5:7b", SourceKind::ollama(), "/o/blob"),
                    member("Qwen2.5-7B-Q4_K_M", SourceKind::lm_studio(), "/l/q.gguf"),
                ],
            ),
            group(
                2_000_000_000,
                vec![
                    member("llama3.2:3b", SourceKind::ollama(), "/o/llama"),
                    member("Llama-3.2-3B", SourceKind::lm_studio(), "/l/l.gguf"),
                ],
            ),
        ];
        summary.reclaimable_bytes = total(&summary.duplicates);
        summary
    }

    #[test]
    fn details_lists_each_store_with_its_decimal_size() {
        let lines = lines_for(
            &summary(&[(SourceKind::ollama(), 3, 4_900_000_000)]),
            &FitTally::default(),
            16 * GIB,
        );
        assert_eq!(lines, vec!["".to_owned(), "ollama  3  4.9 GB".to_owned()]);
    }

    #[test]
    fn details_aligns_the_stores_in_the_headline_order() {
        let lines = lines_for(&shelf(), &FitTally::default(), 16 * GIB);
        assert_eq!(
            &lines[..4],
            &[
                "".to_owned(),
                "ollama     6  28.4 GB".to_owned(),
                "lm-studio  2  10.9 GB".to_owned(),
                "builtin    1".to_owned(),
            ]
        );
    }

    #[test]
    fn details_gives_a_store_with_no_bytes_no_size() {
        let lines = lines_for(
            &summary(&[(SourceKind::builtin(), 1, 0)]),
            &FitTally::default(),
            16 * GIB,
        );
        assert_eq!(lines, vec!["".to_owned(), "builtin  1".to_owned()]);
    }

    #[test]
    fn details_names_every_copy_with_its_store_and_the_reclaimable_bytes() {
        let lines = lines_for(&shelf(), &FitTally::default(), 16 * GIB);
        assert_eq!(
            &lines[4..],
            &[
                "".to_owned(),
                "2 models are copies of weights other models keep (identical in size and sampled content). Removing all of them frees up to 6.7 GB, each file counted once; a row says what removing that copy alone frees:".to_owned(),
                "  keep qwen2.5:7b (ollama)".to_owned(),
                "    4.7 GB  Qwen2.5-7B-Q4_K_M (lm-studio)".to_owned(),
                "  keep llama3.2:3b (ollama)".to_owned(),
                "      2 GB  Llama-3.2-3B (lm-studio)".to_owned(),
            ]
        );
    }

    #[test]
    fn details_counts_the_copies_that_can_go_under_the_one_kept() {
        let mut found = summary(&[(SourceKind::file(), 3, 3_000_000_000)]);
        found.duplicates = vec![group(
            1_000_000_000,
            vec![
                member("a", SourceKind::file(), "/a.gguf"),
                member("b", SourceKind::file(), "/b.gguf"),
                member("c", SourceKind::file(), "/c.gguf"),
            ],
        )];
        found.reclaimable_bytes = total(&found.duplicates);
        let lines = lines_for(&found, &FitTally::default(), 16 * GIB);
        assert_eq!(
            &lines[3..],
            &[
                "2 models are copies of weights another model keeps (identical in size and sampled content). Removing all of them frees up to 2 GB, each file counted once; a row says what removing that copy alone frees:".to_owned(),
                "  keep a (file)".to_owned(),
                "    1 GB  b (file)".to_owned(),
                "    1 GB  c (file)".to_owned(),
            ]
        );
        found.duplicates = vec![group(
            1_000_000_000,
            vec![
                member("a", SourceKind::file(), "/a.gguf"),
                member("b", SourceKind::file(), "/b.gguf"),
            ],
        )];
        found.reclaimable_bytes = total(&found.duplicates);
        let lines = lines_for(&found, &FitTally::default(), 16 * GIB);
        assert_eq!(
            lines[3],
            "1 model is a copy of weights another model keeps (identical in size and sampled content). Removing it frees up to 1 GB:"
        );
    }

    #[test]
    fn details_gives_the_total_for_every_copy_together_beside_each_copy_alone() {
        let mut found = summary(&[(SourceKind::file(), 3, 3_000_000_000)]);
        let mut shares = group(
            300_000_000,
            vec![
                member("k", SourceKind::file(), "/k.gguf"),
                member("x1", SourceKind::file(), "/x1.gguf"),
                member("x2", SourceKind::file(), "/x2.gguf"),
            ],
        );
        shares.reclaimable_bytes = 900_000_000;
        found.duplicates = vec![shares];
        found.reclaimable_bytes = 900_000_000;
        let lines = lines_for(&found, &FitTally::default(), 16 * GIB);
        assert_eq!(
            &lines[3..],
            &[
                "2 models are copies of weights another model keeps (identical in size and sampled content). Removing all of them frees up to 900 MB, each file counted once; a row says what removing that copy alone frees:".to_owned(),
                "  keep k (file)".to_owned(),
                "    300 MB  x1 (file)".to_owned(),
                "    300 MB  x2 (file)".to_owned(),
            ]
        );
        let value = scan_json(&found, &FitTally::default(), 16 * GIB);
        assert_eq!(value["reclaimableBytes"], 900_000_000);
        assert_eq!(value["duplicates"][0]["reclaimableBytes"], 900_000_000);
        assert_eq!(
            value["duplicates"][0]["removable"][0]["reclaimableBytes"],
            300_000_000
        );
    }

    #[test]
    fn details_counts_every_name_of_a_copy_as_a_model() {
        let mut found = summary(&[(SourceKind::ollama(), 3, 3_000_000_000)]);
        let mut tagged = copy(member("t:3b", SourceKind::ollama(), "/o/blob"));
        tagged.aliases = vec![member("t:latest", SourceKind::ollama(), "/o/blob")];
        found.duplicates = vec![group_of(
            1_000_000_000,
            vec![
                copy(member("t", SourceKind::lm_studio(), "/l/t.gguf")),
                tagged,
            ],
        )];
        found.reclaimable_bytes = total(&found.duplicates);
        let lines = lines_for(&found, &FitTally::default(), 16 * GIB);
        assert_eq!(
            lines[3],
            "2 models are copies of weights another model keeps (identical in size and sampled content). Removing them frees up to 1 GB:"
        );
    }

    #[test]
    fn details_names_the_path_of_copies_that_would_read_the_same() {
        let mut found = summary(&[(SourceKind::file(), 2, 2_000_000_000)]);
        found.duplicates = vec![group(
            1_000_000_000,
            vec![
                member("model", SourceKind::file(), "/a/model.gguf"),
                member("model", SourceKind::file(), "/b/model.gguf"),
            ],
        )];
        found.reclaimable_bytes = total(&found.duplicates);
        let lines = lines_for(&found, &FitTally::default(), 16 * GIB);
        assert_eq!(
            &lines[4..],
            &[
                "  keep model (file, /a/model.gguf)".to_owned(),
                "    1 GB  model (file, /b/model.gguf)".to_owned(),
            ]
        );
    }

    #[test]
    fn details_says_nothing_about_duplicates_or_fit_when_there_is_nothing_to_say() {
        let fit = FitTally {
            runs_well: 3,
            tight_fit: 1,
            too_large: 0,
            unknown: 1,
        };
        let lines = lines_for(&summary(&[(SourceKind::ollama(), 5, 1)]), &fit, 16 * GIB);
        assert_eq!(lines.len(), 2, "{lines:?}");
    }

    #[test]
    fn details_counts_too_large_models_against_memory_in_gib() {
        let one = FitTally {
            too_large: 1,
            ..FitTally::default()
        };
        let lines = lines_for(&summary(&[(SourceKind::ollama(), 1, 1)]), &one, 16 * GIB);
        assert_eq!(
            &lines[2..],
            &[
                "".to_owned(),
                "1 model is too big for this machine's 16 GiB. `hedos ls` marks it.".to_owned(),
            ]
        );
        let three = FitTally {
            too_large: 3,
            ..FitTally::default()
        };
        let lines = lines_for(&summary(&[(SourceKind::ollama(), 3, 1)]), &three, 36 * GIB);
        assert_eq!(
            lines[3],
            "3 models are too big for this machine's 36 GiB. `hedos ls` marks them."
        );
    }

    #[test]
    fn details_names_the_other_names_of_a_copy_and_says_what_they_mean() {
        let mut found = summary(&[(SourceKind::ollama(), 2, 4_700_000_000)]);
        let mut shared = copy(member("qwen2.5:7b", SourceKind::ollama(), "/o/blob"));
        shared.aliases = vec![
            member("qwen2.5:latest", SourceKind::ollama(), "/o/blob"),
            member("q-linked", SourceKind::lm_studio(), "/l/hard.gguf"),
        ];
        found.duplicates = vec![group_of(
            4_700_000_000,
            vec![
                shared,
                copy(member("Qwen2.5-7B", SourceKind::lm_studio(), "/l/q.gguf")),
            ],
        )];
        found.reclaimable_bytes = total(&found.duplicates);
        let lines = lines_for(&found, &FitTally::default(), 16 * GIB);
        assert_eq!(
            lines[4],
            "  keep qwen2.5:7b (ollama) [same files: qwen2.5:latest (ollama), q-linked (lm-studio)]"
        );
        assert_eq!(lines[5], "    4.7 GB  Qwen2.5-7B (lm-studio)");
        assert_eq!(
            lines[6],
            "  A name in brackets reaches the same files as the copy before it. Removing all of them frees the space; removing one alone can free nothing while another keeps the files (a hard link, a second Ollama tag), and removing a symlink frees nothing."
        );
    }

    #[test]
    fn details_names_the_file_behind_a_copy_reached_only_by_a_link() {
        let mut found = summary(&[(SourceKind::file(), 2, 2_000_000_000)]);
        let mut linked = copy(member("link", SourceKind::file(), "/e/link.gguf"));
        linked.link_target = Some("/x/real.gguf".to_owned());
        found.duplicates = vec![group_of(
            1_000_000_000,
            vec![
                linked,
                copy(member("other", SourceKind::file(), "/b/other.gguf")),
            ],
        )];
        found.reclaimable_bytes = total(&found.duplicates);
        let lines = lines_for(&found, &FitTally::default(), 16 * GIB);
        assert_eq!(lines[4], "  keep link (file, a link to /x/real.gguf)");
        assert_eq!(lines[5], "    1 GB  other (file)");
        assert_eq!(
            &lines[6..],
            &[
                "  Removing a symlink frees nothing: the file it points to holds the bytes."
                    .to_owned()
            ],
            "no bracket note without aliases"
        );
    }

    #[test]
    fn details_names_the_path_of_a_name_the_shelf_holds_more_than_once() {
        let mut found = summary(&[(SourceKind::file(), 3, 2_000_000_000)]);
        found.duplicates = vec![group(
            1_000_000_000,
            vec![
                member("model", SourceKind::file(), "/a/model.gguf"),
                member("other", SourceKind::file(), "/b/other.gguf"),
            ],
        )];
        found.reclaimable_bytes = total(&found.duplicates);
        let elsewhere = ModelRecord::new(
            "MODEL",
            kernel::records::Modality::text(),
            Vec::new(),
            kernel::records::ModelSource::new(SourceKind::file(), "/c/MODEL.gguf"),
        );
        let lines = details(&found, &[elsewhere], &FitTally::default(), 16 * GIB);
        assert_eq!(lines[4], "  keep model (file, /a/model.gguf)");
        assert_eq!(lines[5], "    1 GB  other (file)");
    }

    #[test]
    fn an_issue_of_several_lines_prints_each_on_its_own() {
        assert_eq!(issue_lines("plain"), vec!["issue: plain".to_owned()]);
        assert_eq!(
            issue_lines("bad.toml: not valid TOML\r\n  |\n2 | a = = \u{1b}\n  |      ^\n"),
            vec![
                "issue: bad.toml: not valid TOML".to_owned(),
                "  |".to_owned(),
                "2 | a = = \\u{1b}".to_owned(),
                "  |      ^".to_owned(),
            ]
        );
    }

    #[test]
    fn details_gives_a_path_to_a_name_repeated_anywhere_in_the_paragraph() {
        let mut found = summary(&[(SourceKind::file(), 3, 3_000_000_000)]);
        let mut first = copy(member("model", SourceKind::file(), "/a/model.gguf"));
        first.aliases = vec![member("model", SourceKind::file(), "/c/model.gguf")];
        found.duplicates = vec![group_of(
            1_000_000_000,
            vec![
                first,
                copy(member("other", SourceKind::file(), "/b/other.gguf")),
            ],
        )];
        found.reclaimable_bytes = total(&found.duplicates);
        let lines = lines_for(&found, &FitTally::default(), 16 * GIB);
        assert_eq!(
            lines[4],
            "  keep model (file, /a/model.gguf) [same files: model (file, /c/model.gguf)]"
        );
        assert_eq!(lines[5], "    1 GB  other (file)");
    }

    #[test]
    fn details_writes_control_characters_in_names_and_paths_visibly() {
        let mut found = summary(&[(SourceKind::file(), 2, 2_000_000_000)]);
        found.duplicates = vec![group(
            1_000_000_000,
            vec![
                member("evil\u{1b}[31mred", SourceKind::file(), "/a/x.gguf"),
                member("line\nbreak", SourceKind::file(), "/b/y.gguf"),
                member("line\nbreak", SourceKind::file(), "/c/\u{7}z.gguf"),
            ],
        )];
        found.reclaimable_bytes = total(&found.duplicates);
        let lines = lines_for(&found, &FitTally::default(), 16 * GIB);
        let row = lines[4..].join(" ");
        assert!(row.chars().all(|c| !c.is_control()), "{row:?}");
        assert!(row.contains("evil\\u{1b}[31mred (file)"), "{row:?}");
        assert!(
            row.contains("line\\nbreak (file, /c/\\u{7}z.gguf)"),
            "{row:?}"
        );
    }

    #[test]
    fn details_is_empty_for_an_empty_scan() {
        assert!(lines_for(&DiscoverySummary::default(), &FitTally::default(), 16 * GIB).is_empty());
    }

    #[test]
    fn details_says_nothing_about_fit_after_a_scan_that_found_nothing() {
        let fit = FitTally {
            runs_well: 2,
            too_large: 1,
            ..FitTally::default()
        };
        assert!(lines_for(&DiscoverySummary::default(), &fit, 16 * GIB).is_empty());
    }

    #[test]
    fn details_never_prints_an_em_dash() {
        let fit = FitTally {
            too_large: 2,
            unknown: 4,
            ..FitTally::default()
        };
        let found = shelf();
        let lines = lines_for(&found, &fit, 16 * GIB);
        for line in lines.iter().chain([&found.headline()]) {
            assert!(!line.contains('\u{2014}'), "{line:?}");
        }
    }

    #[test]
    fn json_keeps_the_keys_it_always_had() {
        let mut found = shelf();
        found.issues = vec!["one".to_owned()];
        let value = scan_json(&found, &FitTally::default(), 16 * GIB);
        assert_eq!(value["totalCount"], 9);
        assert_eq!(value["headline"], found.headline());
        assert_eq!(value["issues"], json!(["one"]));
    }

    #[test]
    fn json_carries_bytes_stores_duplicates_fit_and_failed_stores() {
        let mut found = shelf();
        found.failed_kinds = vec![SourceKind::huggingface_cache()];
        let fit = FitTally {
            runs_well: 6,
            tight_fit: 1,
            too_large: 1,
            unknown: 1,
        };
        let value = scan_json(&found, &fit, 16 * GIB);
        assert_eq!(value["totalBytes"], 39_300_000_000_i64);
        assert_eq!(
            value["stores"],
            json!([
                { "store": "ollama", "count": 6, "bytes": 28_400_000_000_i64 },
                { "store": "lm-studio", "count": 2, "bytes": 10_900_000_000_i64 },
                { "store": "builtin", "count": 1, "bytes": 0 },
            ])
        );
        assert_eq!(
            value["duplicates"][0]["reclaimableBytes"],
            4_700_000_000_i64
        );
        assert_eq!(
            value["duplicates"][0]["kept"],
            json!({
                "id": "id-qwen2.5:7b@/o/blob",
                "name": "qwen2.5:7b",
                "store": "ollama",
                "path": "/o/blob",
                "aliases": [],
                "linkTarget": null,
            })
        );
        assert_eq!(
            value["duplicates"][0]["removable"],
            json!([{
                "id": "id-Qwen2.5-7B-Q4_K_M@/l/q.gguf",
                "name": "Qwen2.5-7B-Q4_K_M",
                "store": "lm-studio",
                "path": "/l/q.gguf",
                "aliases": [],
                "linkTarget": null,
                "reclaimableBytes": 4_700_000_000_i64,
            }])
        );
        assert_eq!(value["reclaimableBytes"], 6_700_000_000_i64);
        assert_eq!(
            value["fit"],
            json!({ "runsWell": 6, "tightFit": 1, "tooLarge": 1, "unknown": 1 })
        );
        assert_eq!(value["memoryBytes"], 16 * GIB);
        assert_eq!(value["failedStores"], json!(["huggingface-cache"]));
    }
}
