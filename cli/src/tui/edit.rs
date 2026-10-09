//! One line of text with a cursor, edited the way a shell line is: the
//! terminal's own keys (Ctrl-A/E, Ctrl-U/W, Option+Delete, the arrows) work
//! on every field of the UI.

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::event::{Edit, Key};

/// A line being typed, with the cursor as a byte offset on a grapheme
/// boundary, so an accent or a joined emoji is never stepped into.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LineEdit {
    text: String,
    cursor: usize,
}

impl LineEdit {
    /// The text as typed.
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// The text without its surrounding whitespace.
    pub fn trimmed(&self) -> &str {
        self.text.trim()
    }

    /// Whether nothing is typed.
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Drop everything.
    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }

    /// Apply `key` if it edits or moves within the line; whether the text
    /// changed (a cursor move does not).
    pub fn apply(&mut self, key: Key) -> bool {
        match key {
            Key::Char(c) => {
                self.text.insert(self.cursor, c);
                self.cursor += c.len_utf8();
                true
            }
            Key::Backspace => {
                let start = self.previous();
                if start == self.cursor {
                    return false;
                }
                self.text.replace_range(start..self.cursor, "");
                self.cursor = start;
                true
            }
            Key::Edit(edit) => self.edit(edit),
            _ => false,
        }
    }

    fn edit(&mut self, edit: Edit) -> bool {
        match edit {
            Edit::Left => self.cursor = self.previous(),
            Edit::Right => self.cursor = self.next(),
            Edit::WordLeft => self.cursor = self.word_start(),
            Edit::WordRight => self.cursor = self.word_end(),
            Edit::Start => self.cursor = 0,
            Edit::End => self.cursor = self.text.len(),
            Edit::Delete => {
                let end = self.next();
                if end == self.cursor {
                    return false;
                }
                self.text.replace_range(self.cursor..end, "");
                return true;
            }
            Edit::KillToStart => {
                if self.cursor == 0 {
                    return false;
                }
                self.text.replace_range(..self.cursor, "");
                self.cursor = 0;
                return true;
            }
            Edit::KillWord => {
                let start = self.word_start();
                if start == self.cursor {
                    return false;
                }
                self.text.replace_range(start..self.cursor, "");
                self.cursor = start;
                return true;
            }
        }
        false
    }

    /// The offset of the grapheme before the cursor, or the cursor at the
    /// start.
    fn previous(&self) -> usize {
        self.text[..self.cursor]
            .grapheme_indices(true)
            .next_back()
            .map_or(self.cursor, |(index, _)| index)
    }

    /// The offset after the grapheme under the cursor, or the cursor at the
    /// end.
    fn next(&self) -> usize {
        self.text[self.cursor..]
            .graphemes(true)
            .next()
            .map_or(self.cursor, |grapheme| self.cursor + grapheme.len())
    }

    /// Where the word before the cursor starts: back over any whitespace,
    /// then to the whitespace before the word.
    fn word_start(&self) -> usize {
        let head = &self.text[..self.cursor];
        let trimmed = head.trim_end();
        trimmed
            .char_indices()
            .rev()
            .find(|(_, c)| c.is_whitespace())
            .map_or(0, |(index, c)| index + c.len_utf8())
    }

    /// Where the word after the cursor ends.
    fn word_end(&self) -> usize {
        let tail = &self.text[self.cursor..];
        let skipped = tail.len() - tail.trim_start().len();
        let end = tail[skipped..]
            .char_indices()
            .find(|(_, c)| c.is_whitespace())
            .map_or(tail.len(), |(index, _)| skipped + index);
        self.cursor + end
    }

    /// The text broken into lines of at most `width` cells, grapheme by
    /// grapheme, and at every line break it holds, with the cursor's row and
    /// column; a cursor at the end of a full line sits at the start of the
    /// next.
    pub fn wrapped(&self, width: usize) -> (Vec<String>, (usize, usize)) {
        let width = width.max(1);
        let mut lines = vec![String::new()];
        let mut used = 0;
        let mut cursor = None;
        for (offset, grapheme) in self.text.grapheme_indices(true) {
            if grapheme == "\n" || grapheme == "\r\n" {
                if offset == self.cursor {
                    cursor = Some((lines.len() - 1, used));
                }
                lines.push(String::new());
                used = 0;
                continue;
            }
            let cells = grapheme.width().max(1);
            if used + cells > width {
                lines.push(String::new());
                used = 0;
            }
            if offset == self.cursor {
                cursor = Some((lines.len() - 1, used));
            }
            if let Some(line) = lines.last_mut() {
                line.push_str(grapheme);
            }
            used += cells;
        }
        let cursor = cursor.unwrap_or_else(|| {
            if used >= width {
                lines.push(String::new());
                (lines.len() - 1, 0)
            } else {
                (lines.len() - 1, used)
            }
        });
        (lines, cursor)
    }

    /// The text before and after the cursor, windowed to `width` cells so the
    /// cursor is always on screen: the window fills backwards from the cursor
    /// first, then forwards with what room is left.
    pub fn view(&self, width: usize) -> (String, String) {
        let mut used = 0;
        let before: String = self.text[..self.cursor]
            .graphemes(true)
            .rev()
            .take_while(|grapheme| {
                let fits = used + grapheme.width() <= width;
                if fits {
                    used += grapheme.width();
                }
                fits
            })
            .collect::<Vec<&str>>()
            .into_iter()
            .rev()
            .collect();
        let after: String = self.text[self.cursor..]
            .graphemes(true)
            .take_while(|grapheme| {
                let fits = used + grapheme.width() < width;
                if fits {
                    used += grapheme.width();
                }
                fits
            })
            .collect();
        (before, after)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str) -> LineEdit {
        let mut line = LineEdit::default();
        for c in text.chars() {
            line.apply(Key::Char(c));
        }
        line
    }

    #[test]
    fn typing_and_deleting_happen_at_the_cursor() {
        let mut line = line("hello");
        line.apply(Key::Edit(Edit::Left));
        line.apply(Key::Edit(Edit::Left));
        assert!(line.apply(Key::Char('L')));
        assert_eq!(line.as_str(), "helLlo");
        assert!(line.apply(Key::Backspace));
        assert!(line.apply(Key::Backspace));
        assert_eq!(line.as_str(), "helo");
        line.apply(Key::Edit(Edit::Start));
        assert!(!line.apply(Key::Backspace));
        assert!(line.apply(Key::Edit(Edit::Delete)));
        assert_eq!(line.as_str(), "elo");
        line.apply(Key::Edit(Edit::End));
        assert!(!line.apply(Key::Edit(Edit::Delete)));
    }

    #[test]
    fn graphemes_move_and_delete_whole() {
        let mut line = line("ae\u{301}x");
        line.apply(Key::Edit(Edit::Left));
        line.apply(Key::Edit(Edit::Left));
        assert_eq!(line.view(10), ("a".to_owned(), "e\u{301}x".to_owned()));
        assert!(line.apply(Key::Edit(Edit::Delete)));
        assert_eq!(line.as_str(), "ax");
        let mut family = super::tests::line("👨\u{200d}👩\u{200d}👧!");
        family.apply(Key::Edit(Edit::Left));
        assert!(family.apply(Key::Backspace));
        assert_eq!(family.as_str(), "!");
        let wide = super::tests::line("日本語");
        assert_eq!(wide.view(4), ("本語".to_owned(), String::new()));
        assert_eq!(wide.view(3), ("語".to_owned(), String::new()));
    }

    #[test]
    fn multibyte_characters_move_and_delete_whole() {
        let mut line = line("a日b");
        line.apply(Key::Edit(Edit::Left));
        assert!(line.apply(Key::Backspace));
        assert_eq!(line.as_str(), "ab");
        line.apply(Key::Char('é'));
        line.apply(Key::Edit(Edit::Left));
        assert!(line.apply(Key::Edit(Edit::Delete)));
        assert_eq!(line.as_str(), "ab");
    }

    #[test]
    fn kill_to_start_and_kill_word_cut_backwards_from_the_cursor() {
        let mut line = line("one two\t three");
        assert!(line.apply(Key::Edit(Edit::KillWord)));
        assert_eq!(line.as_str(), "one two\t ");
        assert!(line.apply(Key::Edit(Edit::KillWord)));
        assert_eq!(line.as_str(), "one ");
        line.apply(Key::Char('x'));
        line.apply(Key::Edit(Edit::Left));
        assert!(line.apply(Key::Edit(Edit::KillToStart)));
        assert_eq!(line.as_str(), "x");
        assert!(!line.apply(Key::Edit(Edit::KillToStart)));
        assert!(!line.apply(Key::Edit(Edit::KillWord)));
    }

    #[test]
    fn word_moves_hop_over_whitespace() {
        let mut line = line("ab  cd");
        line.apply(Key::Edit(Edit::WordLeft));
        line.apply(Key::Char('|'));
        assert_eq!(line.as_str(), "ab  |cd");
        line.apply(Key::Edit(Edit::WordLeft));
        line.apply(Key::Edit(Edit::WordLeft));
        line.apply(Key::Edit(Edit::WordRight));
        line.apply(Key::Char('!'));
        assert_eq!(line.as_str(), "ab!  |cd");
        line.apply(Key::Edit(Edit::WordRight));
        line.apply(Key::Edit(Edit::WordRight));
        line.apply(Key::Char('$'));
        assert_eq!(line.as_str(), "ab!  |cd$");
    }

    #[test]
    fn the_view_keeps_the_cursor_on_screen() {
        let mut line = line("abcdefgh");
        assert_eq!(line.view(4), ("efgh".to_owned(), String::new()));
        line.apply(Key::Edit(Edit::Start));
        assert_eq!(line.view(4), (String::new(), "abc".to_owned()));
        line.apply(Key::Edit(Edit::Right));
        line.apply(Key::Edit(Edit::Right));
        assert_eq!(line.view(4), ("ab".to_owned(), "c".to_owned()));
        assert_eq!(line.view(20), ("ab".to_owned(), "cdefgh".to_owned()));
        assert_eq!(line.view(0), (String::new(), String::new()));
        assert_eq!(LineEdit::default().view(4), (String::new(), String::new()));
    }

    #[test]
    fn a_long_line_wraps_and_the_cursor_follows_it() {
        let mut edit = LineEdit::default();
        for c in "abcdefg".chars() {
            edit.apply(Key::Char(c));
        }
        assert_eq!(
            edit.wrapped(3),
            (
                vec!["abc".to_owned(), "def".to_owned(), "g".to_owned()],
                (2, 1)
            )
        );
        edit.apply(Key::Edit(Edit::Start));
        assert_eq!(edit.wrapped(3).1, (0, 0));
        edit.apply(Key::Edit(Edit::End));
        edit.apply(Key::Backspace);
        assert_eq!(
            edit.wrapped(3),
            (
                vec!["abc".to_owned(), "def".to_owned(), String::new()],
                (2, 0)
            )
        );
        assert_eq!(
            LineEdit::default().wrapped(5),
            (vec![String::new()], (0, 0))
        );
    }

    #[test]
    fn a_line_break_starts_a_new_wrapped_line() {
        let mut edit = LineEdit::default();
        for c in "ab\ncd".chars() {
            edit.apply(Key::Char(c));
        }
        let (lines, cursor) = edit.wrapped(10);
        assert_eq!(lines, ["ab", "cd"]);
        assert_eq!(cursor, (1, 2));
    }
}
