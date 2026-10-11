//! What a Hugging Face model card says a model is: the `pipeline_tag` and the
//! `tags` in the YAML front matter at the top of a snapshot's `README.md`.
//!
//! Only those two keys are wanted, and both come in a few plain shapes (a
//! scalar, a block list, a flow list), so they are read line by line rather
//! than through a YAML parser.

use std::io::{BufRead, BufReader};
use std::path::Path;

/// The most of a card read while looking for the end of its front matter. The
/// keys wanted sit near the top; past this, whatever was seen is kept.
const FRONT_MATTER_LIMIT: usize = 64 * 1024;

/// The front matter of a model card, lowercased.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelCard {
    /// The task the card files the model under (`text-to-audio`).
    pub pipeline_tag: Option<String>,
    /// The card's free-form tags (`music-generation`).
    pub tags: Vec<String>,
}

impl ModelCard {
    /// The card in `dir`'s `README.md`, when it opens with front matter. Reads
    /// up to the closing `---`, or [`FRONT_MATTER_LIMIT`] bytes.
    pub fn read(dir: &Path) -> Option<Self> {
        let file = crate::fs::open_regular(&dir.join("README.md")).ok()?;
        let mut reader = BufReader::new(file);
        let mut text = String::new();
        let mut line = Vec::new();
        while text.len() < FRONT_MATTER_LIMIT {
            line.clear();
            if reader.read_until(b'\n', &mut line).ok()? == 0 {
                break;
            }
            let decoded = String::from_utf8_lossy(&line);
            let closes = !text.is_empty() && decoded.trim_end() == "---";
            text.push_str(&decoded);
            if closes {
                break;
            }
        }
        Self::parse(&text)
    }

    /// The front matter of card `text`: `None` unless it opens with a `---`
    /// line. Reading stops at the closing `---`, or at the end of `text`.
    pub fn parse(text: &str) -> Option<Self> {
        let mut lines = text.trim_start_matches('\u{feff}').lines();
        if lines.next()?.trim_end() != "---" {
            return None;
        }
        let mut card = Self::default();
        let mut list = List::None;
        for line in lines {
            if line.trim_end() == "---" {
                break;
            }
            match list {
                List::Block => {
                    let item = line.trim_start();
                    if item.is_empty() || item.starts_with('#') {
                        continue;
                    }
                    if let Some(item) = item.strip_prefix('-') {
                        card.push_tag(item);
                        continue;
                    }
                    list = List::None;
                }
                List::Flow => {
                    let (items, closed) = match line.split_once(']') {
                        Some((items, _)) => (items, true),
                        None => (line, false),
                    };
                    items.split(',').for_each(|item| card.push_tag(item));
                    if closed {
                        list = List::None;
                    }
                    continue;
                }
                List::None => {}
            }
            if let Some(value) = line.strip_prefix("pipeline_tag:") {
                card.pipeline_tag = Some(scalar(value)).filter(|tag| !tag.is_empty());
            } else if let Some(value) = line.strip_prefix("tags:") {
                let value = value.trim();
                if let Some(flow) = value.strip_prefix('[') {
                    let (items, closed) = match flow.split_once(']') {
                        Some((items, _)) => (items, true),
                        None => (flow, false),
                    };
                    items.split(',').for_each(|item| card.push_tag(item));
                    if !closed {
                        list = List::Flow;
                    }
                } else if scalar(value).is_empty() {
                    list = List::Block;
                }
            }
        }
        Some(card)
    }

    /// Whether the card carries `tag`.
    pub fn has_tag(&self, tag: &str) -> bool {
        self.tags.iter().any(|carried| carried == tag)
    }

    fn push_tag(&mut self, raw: &str) {
        let tag = scalar(raw);
        if !tag.is_empty() {
            self.tags.push(tag);
        }
    }
}

/// Which kind of `tags:` list the lines being read belong to.
#[derive(Clone, Copy)]
enum List {
    None,
    /// `- item` lines.
    Block,
    /// A `[a, b` flow list still waiting for its `]`.
    Flow,
}

/// A YAML scalar as written, lowercased: the text between its quotes when
/// quoted, else the text before any ` #` comment, trimmed.
fn scalar(raw: &str) -> String {
    let raw = raw.trim();
    let value = match raw.chars().next() {
        Some(quote @ ('"' | '\'')) => raw[1..].split(quote).next().unwrap_or_default(),
        _ => match raw.find(" #") {
            Some(comment) => &raw[..comment],
            None if raw.starts_with('#') => "",
            None => raw,
        },
    };
    value.trim().to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_block_list_and_the_pipeline_tag() {
        let card = ModelCard::parse(
            "---\nlicense: cc-by-nc-4.0\nlanguage:\n- zh\n- en\npipeline_tag: text-to-audio\n\
             tags:\n- arxiv:2610.05336\n- music-generation\n- custom_code\n---\n# YuE2\n",
        )
        .expect("front matter");
        assert_eq!(card.pipeline_tag.as_deref(), Some("text-to-audio"));
        assert!(card.has_tag("music-generation"));
        assert!(card.has_tag("custom_code"));
        assert!(!card.has_tag("zh"), "a list under another key is not tags");
    }

    #[test]
    fn reads_indented_quoted_and_flow_lists() {
        let card = ModelCard::parse(
            "---\ntags:\n  - \"Text-To-Speech\"\n  - 'TTS'\npipeline_tag: \"text-to-speech\"\n---\n",
        )
        .expect("front matter");
        assert_eq!(card.tags, vec!["text-to-speech", "tts"]);
        assert_eq!(card.pipeline_tag.as_deref(), Some("text-to-speech"));

        let flow = ModelCard::parse("---\ntags: [music, text-to-audio]\n---\n").unwrap();
        assert_eq!(flow.tags, vec!["music", "text-to-audio"]);
    }

    #[test]
    fn comments_blank_lines_and_crlf_do_not_change_what_is_read() {
        let card = ModelCard::parse(
            "---\r\npipeline_tag: \"text-to-speech\"  # filed as a voice\r\n\
             tags:\r\n  - music # a domain tag\r\n\r\n  # more below\r\n  - tts\r\nlicense: mit\r\n---\r\n",
        )
        .expect("front matter");
        assert_eq!(card.pipeline_tag.as_deref(), Some("text-to-speech"));
        assert_eq!(card.tags, vec!["music", "tts"]);

        let bare = ModelCard::parse("---\npipeline_tag: text-to-audio # x\n---\n").unwrap();
        assert_eq!(bare.pipeline_tag.as_deref(), Some("text-to-audio"));
    }

    #[test]
    fn a_flow_list_may_run_over_lines() {
        let card =
            ModelCard::parse("---\ntags: [music,\n  text-to-audio,\n  stems]\nlicense: mit\n---\n")
                .unwrap();
        assert_eq!(card.tags, vec!["music", "text-to-audio", "stems"]);
    }

    #[test]
    fn a_card_without_front_matter_has_none() {
        assert_eq!(ModelCard::parse("# A model\npipeline_tag: x\n"), None);
        assert_eq!(ModelCard::parse(""), None);
    }

    #[test]
    fn an_unclosed_front_matter_keeps_what_was_read() {
        let card = ModelCard::parse("---\npipeline_tag: text-to-audio\nmodel-index:\n").unwrap();
        assert_eq!(card.pipeline_tag.as_deref(), Some("text-to-audio"));
    }

    #[test]
    fn a_byte_order_mark_is_skipped() {
        let card = ModelCard::parse("\u{feff}---\npipeline_tag: text-to-music\n---\n").unwrap();
        assert_eq!(card.pipeline_tag.as_deref(), Some("text-to-music"));
    }

    #[test]
    fn reading_stops_at_the_end_of_the_front_matter() {
        let dir = std::env::temp_dir().join(format!("hedos-card-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let body = "x".repeat(200 * 1024);
        std::fs::write(
            dir.join("README.md"),
            format!(
                "---\npipeline_tag: text-to-audio\n---\n{body}\npipeline_tag: text-generation\n"
            ),
        )
        .unwrap();
        let card = ModelCard::read(&dir);
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(card.unwrap().pipeline_tag.as_deref(), Some("text-to-audio"));
    }
}
