//! The little markdown a reply carries, read before the text is wrapped so
//! the markers never take cells and a wrap can never split one: `**bold**`,
//! `# headings`, fenced and inline code (inside which nothing is a marker).

use super::wrap::{Cell, cells, wrap_cells};

/// How a stretch of a reply is set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Emphasis {
    Plain,
    Bold,
    Code,
}

/// A stretch of one line set one way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    pub text: String,
    pub emphasis: Emphasis,
}

/// `text` as wrapped lines of runs, `width` cells each at most. A fence line
/// (```` ``` ````) opens or closes a code block and is not shown itself.
pub fn lines(text: &str, width: usize) -> Vec<Vec<Run>> {
    let mut lines = Vec::new();
    let mut fenced = false;
    for paragraph in text.split('\n') {
        if paragraph.trim_start().starts_with("```") {
            fenced = !fenced;
            continue;
        }
        let cells = if fenced {
            cells(paragraph, Emphasis::Code)
        } else {
            prose(paragraph)
        };
        lines.extend(wrap_cells(cells, width).into_iter().map(runs));
    }
    lines
}

/// One paragraph of prose as cells: a heading is bold throughout, otherwise
/// `**` toggles bold and a backtick opens inline code until the next one.
fn prose(paragraph: &str) -> Vec<Cell<Emphasis>> {
    let hashes = paragraph.chars().take_while(|c| *c == '#').count();
    if hashes > 0 && paragraph[hashes..].starts_with(' ') {
        return cells(paragraph[hashes + 1..].trim_start(), Emphasis::Bold);
    }
    let mut out = Vec::new();
    let mut bold = false;
    let mut code = false;
    let mut rest = paragraph;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix('`') {
            code = !code;
            rest = after;
        } else if !code && let Some(after) = rest.strip_prefix("**") {
            bold = !bold;
            rest = after;
        } else {
            let end = rest[1..]
                .find(['`', '*'])
                .map_or(rest.len(), |index| index + 1);
            let emphasis = if code {
                Emphasis::Code
            } else if bold {
                Emphasis::Bold
            } else {
                Emphasis::Plain
            };
            out.extend(cells(&rest[..end], emphasis));
            rest = &rest[end..];
        }
    }
    out
}

/// Adjacent cells set the same way, joined.
fn runs(line: Vec<Cell<Emphasis>>) -> Vec<Run> {
    let mut runs: Vec<Run> = Vec::new();
    for cell in line {
        match runs.last_mut() {
            Some(run) if run.emphasis == cell.style => run.text.push_str(&cell.grapheme),
            _ => runs.push(Run {
                text: cell.grapheme,
                emphasis: cell.style,
            }),
        }
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(lines: &[Vec<Run>]) -> Vec<Vec<(&str, Emphasis)>> {
        lines
            .iter()
            .map(|line| {
                line.iter()
                    .map(|run| (run.text.as_str(), run.emphasis))
                    .collect()
            })
            .collect()
    }

    #[test]
    fn bold_markers_are_dropped_and_take_no_cells() {
        let wrapped = lines("1. **Talk:** say it", 40);
        assert_eq!(
            flat(&wrapped),
            [vec![
                ("1. ", Emphasis::Plain),
                ("Talk:", Emphasis::Bold),
                (" say it", Emphasis::Plain)
            ]]
        );
        assert_eq!(flat(&lines("ab**cdefgh**", 3)).len(), 3);
        assert_eq!(
            flat(&lines("ab**cdefgh**", 3))[0],
            [("ab", Emphasis::Plain), ("c", Emphasis::Bold)]
        );
    }

    #[test]
    fn bold_carries_across_a_wrap_but_not_a_paragraph() {
        let wrapped = lines("**one two**\nthree", 5);
        assert_eq!(
            flat(&wrapped),
            [
                vec![("one", Emphasis::Bold)],
                vec![("two", Emphasis::Bold)],
                vec![("three", Emphasis::Plain)]
            ]
        );
        assert_eq!(
            flat(&lines("**open\nplain", 10))[1],
            [("plain", Emphasis::Plain)]
        );
    }

    #[test]
    fn code_keeps_its_stars() {
        let wrapped = lines("use `f(**kw)` here\n```\nx = a ** 2\n```\n**b**", 40);
        assert_eq!(
            flat(&wrapped),
            [
                vec![
                    ("use ", Emphasis::Plain),
                    ("f(**kw)", Emphasis::Code),
                    (" here", Emphasis::Plain)
                ],
                vec![("x = a ** 2", Emphasis::Code)],
                vec![("b", Emphasis::Bold)]
            ]
        );
    }

    #[test]
    fn a_heading_is_bold_without_its_hashes() {
        assert_eq!(
            flat(&lines("## Title", 40)),
            [vec![("Title", Emphasis::Bold)]]
        );
        assert_eq!(
            flat(&lines("#notatag", 40)),
            [vec![("#notatag", Emphasis::Plain)]]
        );
    }
}
