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

/// A reply read into what it is made of: prose, wrapped, and code, kept as
/// written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    /// A paragraph as wrapped lines of runs.
    Prose(Vec<Vec<Run>>),
    /// A fenced block: its language, its lines unwrapped, and whether its
    /// closing fence has arrived yet.
    Code {
        lang: String,
        lines: Vec<String>,
        open: bool,
    },
}

/// `text` as blocks, prose wrapped to `width` cells. A fence line
/// (```` ``` ````) opens or closes a code block and is not shown itself; the
/// word after an opening fence is the block's language. A code block is not
/// wrapped, so a command keeps its shape; the drawer clips it.
pub fn blocks(text: &str, width: usize) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut code: Option<(String, Vec<String>)> = None;
    for paragraph in text.split('\n') {
        if let Some(rest) = paragraph.trim_start().strip_prefix("```") {
            match code.take() {
                Some((lang, lines)) => blocks.push(Block::Code {
                    lang,
                    lines,
                    open: false,
                }),
                None => code = Some((rest.trim().to_owned(), Vec::new())),
            }
            continue;
        }
        match &mut code {
            Some((_, lines)) => lines.push(paragraph.replace('\t', "    ").replace('\r', "")),
            None => blocks.push(Block::Prose(
                wrap_cells(prose(paragraph), width)
                    .into_iter()
                    .map(runs)
                    .collect(),
            )),
        }
    }
    if let Some((lang, lines)) = code {
        blocks.push(Block::Code {
            lang,
            lines,
            open: true,
        });
    }
    blocks
}

/// `text` as wrapped lines of runs, code included as code runs, for reading
/// a reply back in a test.
#[cfg(test)]
fn lines(text: &str, width: usize) -> Vec<Vec<Run>> {
    blocks(text, width)
        .into_iter()
        .flat_map(|block| match block {
            Block::Prose(lines) => lines,
            Block::Code { lines, .. } => lines
                .into_iter()
                .flat_map(|line| wrap_cells(cells(&line, Emphasis::Code), width))
                .map(runs)
                .collect(),
        })
        .collect()
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
            // A lone `*`, or `**` inside code, is text: the run takes its
            // first character before the next marker is looked for.
            let end = rest
                .char_indices()
                .skip(1)
                .find(|(_, c)| matches!(c, '`' | '*'))
                .map_or(rest.len(), |(index, _)| index);
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
    fn text_in_any_script_is_read_without_a_panic() {
        // A run that opens on a character wider than one byte, at the start
        // of the paragraph and right after a marker.
        let georgian = "დიდი **მადლობა** `კოდი` ჩინური 中文 and emoji 🙂";
        let wrapped = lines(georgian, 12);
        let joined: String = wrapped
            .iter()
            .flat_map(|line| line.iter().map(|run| run.text.as_str()))
            .collect();
        assert_eq!(
            joined.replace(' ', ""),
            georgian.replace(['*', '`', ' '], "")
        );
        assert!(flat(&wrapped).iter().flatten().any(|(text, emphasis)| {
            *text == "მადლობა" && *emphasis == Emphasis::Bold
        }));
        assert!(
            flat(&wrapped)
                .iter()
                .flatten()
                .any(|(text, emphasis)| { *text == "კოდი" && *emphasis == Emphasis::Code })
        );
        assert_eq!(flat(&lines("*", 5)), [vec![("*", Emphasis::Plain)]]);
        assert_eq!(flat(&lines("დ*", 5)), [vec![("დ*", Emphasis::Plain)]]);
        assert_eq!(flat(&lines("# დ", 5)), [vec![("დ", Emphasis::Bold)]]);
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

    #[test]
    fn a_code_block_keeps_its_language_and_its_shape() {
        let read = blocks(
            "say\n```sh\ncurl -s localhost:11434/v1/chat/completions\n```\nend",
            10,
        );
        assert_eq!(read.len(), 3);
        assert!(matches!(&read[0], Block::Prose(lines) if lines.len() == 1));
        assert_eq!(
            read[1],
            Block::Code {
                lang: "sh".to_owned(),
                lines: vec!["curl -s localhost:11434/v1/chat/completions".to_owned()],
                open: false,
            }
        );
        let streaming = blocks("```\nlet x", 40);
        assert_eq!(
            streaming,
            [Block::Code {
                lang: String::new(),
                lines: vec!["let x".to_owned()],
                open: true,
            }]
        );
    }
}
