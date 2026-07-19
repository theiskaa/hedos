//! Turning rendered markdown into speakable plain text: strip the structure a
//! reader shouldn't hear (fences, tables, headings, list markers, links, and
//! inline emphasis) and leave the words.

use std::sync::LazyLock;

use regex::Regex;

/// The inline markdown transforms, applied in order to each surviving line.
///
/// Each entry is a compiled pattern and the replacement it substitutes (`$1`
/// keeps the first capture). Compiled once, mirroring the Swift `stripInline`
/// pass literal-for-literal.
static INLINE: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    [
        (r"^\s{0,3}#{1,6}\s+", ""),
        (r"^\s{0,3}>\s?", ""),
        (r"^(\s*)[-*+]\s+", "$1"),
        (r"!?\[([^\]]*)\]\([^)]*\)", "$1"),
        (r"\*{1,3}([^*]+)\*{1,3}", "$1"),
        (r"_{1,3}([^_]+)_{1,3}", "$1"),
        (r"`([^`]*)`", "$1"),
    ]
    .into_iter()
    .filter_map(|(pattern, replacement)| Regex::new(pattern).ok().map(|re| (re, replacement)))
    .collect()
});

/// Collapses three or more consecutive newlines down to a paragraph break.
static BLANK_RUN: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"\n{3,}").ok());

/// The plain, speakable form of a markdown string: code fences and table rows
/// are dropped whole, and every other line has its markdown syntax stripped so
/// only the words a listener should hear remain.
pub fn speakable(markdown: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut inside_fence = false;
    for line in markdown.split('\n') {
        let trimmed = trim_spaces(line);
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            inside_fence = !inside_fence;
            continue;
        }
        if inside_fence {
            continue;
        }
        if trimmed.starts_with('|') && trimmed.ends_with('|') {
            continue;
        }
        lines.push(strip_inline(line));
    }
    let joined = lines.join("\n");
    let collapsed = match &*BLANK_RUN {
        Some(re) => re.replace_all(&joined, "\n\n").into_owned(),
        None => joined,
    };
    collapsed.trim().to_owned()
}

fn strip_inline(line: &str) -> String {
    let mut text = line.to_owned();
    for (pattern, replacement) in INLINE.iter() {
        text = pattern.replace_all(&text, *replacement).into_owned();
    }
    text
}

/// Trims horizontal whitespace — tab plus every Unicode space separator (Zs) —
/// but never line breaks, matching Swift's `CharacterSet.whitespaces` so that a
/// fence or table marker led by a non-breaking space is still detected.
fn trim_spaces(line: &str) -> &str {
    line.trim_matches(is_horizontal_ws)
}

/// True for Swift `.whitespaces`: tab or a Zs space separator. `char::is_whitespace`
/// (the Unicode White_Space set) adds line breaks and the VT/FF/NEL controls,
/// which `.whitespaces` excludes — so those are filtered back out.
fn is_horizontal_ws(c: char) -> bool {
    c.is_whitespace()
        && !matches!(
            c,
            '\n' | '\r' | '\u{0B}' | '\u{0C}' | '\u{85}' | '\u{2028}' | '\u{2029}'
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_code_fence_and_its_body_are_dropped() {
        let markdown = "before\n```rust\nlet x = 1;\n```\nafter";
        assert_eq!(speakable(markdown), "before\nafter");
    }

    #[test]
    fn a_tilde_fence_toggles_the_same_way() {
        let markdown = "a\n~~~\nhidden\n~~~\nb";
        assert_eq!(speakable(markdown), "a\nb");
    }

    #[test]
    fn table_rows_are_skipped() {
        let markdown = "text\n| col | col |\n| --- | --- |\nmore";
        assert_eq!(speakable(markdown), "text\nmore");
    }

    #[test]
    fn headings_lose_their_hashes() {
        assert_eq!(speakable("## Title"), "Title");
        assert_eq!(speakable("   ###### Deep"), "Deep");
    }

    #[test]
    fn blockquotes_lose_their_marker() {
        assert_eq!(speakable("> quoted"), "quoted");
        assert_eq!(speakable(">no space"), "no space");
    }

    #[test]
    fn list_markers_go_but_indentation_stays() {
        assert_eq!(speakable("- item"), "item");
        assert_eq!(speakable("+ plus"), "plus");
        // The whole-output trim only touches the document edges, so a nested
        // item in the middle keeps its indentation.
        assert_eq!(speakable("top\n  * nested\nend"), "top\n  nested\nend");
    }

    #[test]
    fn links_and_images_keep_only_their_label() {
        assert_eq!(speakable("see [the docs](https://x.y)"), "see the docs");
        assert_eq!(speakable("![alt text](img.png)"), "alt text");
    }

    #[test]
    fn emphasis_and_inline_code_are_unwrapped() {
        assert_eq!(speakable("**bold** and *italic*"), "bold and italic");
        assert_eq!(speakable("_under_ and __strong__"), "under and strong");
        assert_eq!(speakable("run `cargo test` now"), "run cargo test now");
    }

    #[test]
    fn runs_of_blank_lines_collapse_to_one_paragraph_break() {
        let markdown = "a\n\n\n\nb";
        assert_eq!(speakable(markdown), "a\n\nb");
    }

    #[test]
    fn leading_and_trailing_whitespace_is_trimmed() {
        assert_eq!(speakable("\n\n  hello  \n\n"), "hello");
    }

    #[test]
    fn an_empty_string_stays_empty() {
        assert_eq!(speakable(""), "");
    }

    #[test]
    fn a_fence_led_by_a_non_breaking_space_is_still_a_fence() {
        // A non-breaking space (Zs) before the fence must still be trimmed for
        // detection, or the whole code block would leak into the spoken text.
        let markdown = "\u{A0}```\ncode\n```";
        assert_eq!(speakable(markdown), "");
    }

    #[test]
    fn a_table_row_with_a_trailing_nbsp_is_still_skipped() {
        let markdown = "text\n| a | b |\u{A0}\nmore";
        assert_eq!(speakable(markdown), "text\nmore");
    }
}
