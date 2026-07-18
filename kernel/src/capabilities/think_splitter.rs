//! Separating a model's "thinking" spans (delimited by tags like
//! `<think>…</think>`) from its visible text as the stream arrives.

use crate::capabilities::held_suffix_len;

/// A run of separated output: either visible text or hidden thinking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Piece {
    /// Visible text.
    Text(String),
    /// Thinking to hide or fold away.
    Thinking(String),
}

/// A matching pair of open/close thinking delimiters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagPair {
    /// The opening delimiter.
    pub open: String,
    /// The closing delimiter.
    pub close: String,
}

/// The default thinking delimiters recognized.
pub fn default_pairs() -> Vec<TagPair> {
    vec![
        TagPair {
            open: "<think>".to_owned(),
            close: "</think>".to_owned(),
        },
        TagPair {
            open: "<|START_THINKING|>".to_owned(),
            close: "<|END_THINKING|>".to_owned(),
        },
    ]
}

/// Whether `text` contains any default thinking delimiter.
pub fn has_visible_tags(text: &str) -> bool {
    default_pairs()
        .iter()
        .any(|pair| text.contains(&pair.open) || text.contains(&pair.close))
}

enum Mode {
    Text,
    Thinking(String),
}

/// A streaming splitter that separates thinking spans from visible text.
pub struct ThinkSplitter {
    pairs: Vec<TagPair>,
    open_tags: Vec<String>,
    mode: Mode,
    buffer: String,
}

impl Default for ThinkSplitter {
    fn default() -> Self {
        Self::with_pairs(default_pairs())
    }
}

impl ThinkSplitter {
    /// A splitter using the default delimiter pairs.
    pub fn new() -> Self {
        Self::default()
    }

    /// A splitter using the given delimiter pairs. Pairs with an empty open or
    /// close delimiter are dropped — an empty delimiter would match everywhere
    /// and make no progress.
    pub fn with_pairs(pairs: Vec<TagPair>) -> Self {
        let pairs: Vec<TagPair> = pairs
            .into_iter()
            .filter(|pair| !pair.open.is_empty() && !pair.close.is_empty())
            .collect();
        let open_tags = pairs.iter().map(|pair| pair.open.clone()).collect();
        Self {
            pairs,
            open_tags,
            mode: Mode::Text,
            buffer: String::new(),
        }
    }

    /// Feed a chunk and return the pieces that can be classified now. A partial
    /// delimiter at the end of the buffer is held back until more text arrives.
    pub fn feed(&mut self, chunk: &str) -> Vec<Piece> {
        self.buffer.push_str(chunk);
        let mut output = Vec::new();
        loop {
            match &self.mode {
                Mode::Text => {
                    let opening = self
                        .pairs
                        .iter()
                        .filter_map(|pair| {
                            self.buffer
                                .find(&pair.open)
                                .map(|at| (at, pair.open.len(), &pair.close))
                        })
                        .min_by_key(|(at, _, _)| *at);
                    match opening {
                        Some((at, open_len, close)) => {
                            let close = close.clone();
                            let before = self.buffer[..at].to_owned();
                            if !before.is_empty() {
                                output.push(Piece::Text(before));
                            }
                            self.buffer.drain(..at + open_len);
                            self.mode = Mode::Thinking(close);
                        }
                        None => {
                            Self::drain_prefix(
                                &mut self.buffer,
                                &self.open_tags,
                                false,
                                &mut output,
                            );
                            break;
                        }
                    }
                }
                Mode::Thinking(close) => {
                    let close = close.clone();
                    match self.buffer.find(&close) {
                        Some(at) => {
                            let before = self.buffer[..at].to_owned();
                            if !before.is_empty() {
                                output.push(Piece::Thinking(before));
                            }
                            self.buffer.drain(..at + close.len());
                            self.mode = Mode::Text;
                        }
                        None => {
                            Self::drain_prefix(&mut self.buffer, &[close], true, &mut output);
                            break;
                        }
                    }
                }
            }
        }
        output
    }

    /// Emit any buffered text, classified by the current mode.
    pub fn flush(&mut self) -> Vec<Piece> {
        if self.buffer.is_empty() {
            return Vec::new();
        }
        let buffer = std::mem::take(&mut self.buffer);
        match &self.mode {
            Mode::Thinking(_) => vec![Piece::Thinking(buffer)],
            Mode::Text => vec![Piece::Text(buffer)],
        }
    }

    fn drain_prefix(buffer: &mut String, tags: &[String], thinking: bool, output: &mut Vec<Piece>) {
        let emit_len = buffer.len() - held_suffix_len(buffer, tags);
        if emit_len > 0 {
            let emit = buffer[..emit_len].to_owned();
            output.push(if thinking {
                Piece::Thinking(emit)
            } else {
                Piece::Text(emit)
            });
            buffer.drain(..emit_len);
        }
    }
}
