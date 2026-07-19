//! Reassembling UTF-8 text from a byte stream whose chunks may split a multi-byte
//! character. A token-by-token model backend hands back raw bytes; this holds an
//! incomplete trailing sequence until the rest arrives so no `?` glyphs leak out.

/// Buffers a trailing partial UTF-8 sequence between feeds.
#[derive(Debug, Default)]
pub struct Utf8StreamAssembler {
    tail: Vec<u8>,
}

impl Utf8StreamAssembler {
    /// A fresh assembler.
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed the next chunk, returning whatever complete text is now available. A
    /// partial character at the end is held back for the next `feed`.
    pub fn feed(&mut self, bytes: &[u8]) -> String {
        let mut buffer = std::mem::take(&mut self.tail);
        buffer.extend_from_slice(bytes);
        if buffer.is_empty() {
            return String::new();
        }
        let held = incomplete_suffix_length(&buffer);
        if held > 0 {
            self.tail = buffer.split_off(buffer.len() - held);
        }
        if buffer.is_empty() {
            return String::new();
        }
        String::from_utf8_lossy(&buffer).into_owned()
    }

    /// Emit any held-back trailing bytes, decoding them lossily. Call once the
    /// stream has ended.
    pub fn flush(&mut self) -> String {
        if self.tail.is_empty() {
            return String::new();
        }
        let remainder = String::from_utf8_lossy(&self.tail).into_owned();
        self.tail.clear();
        remainder
    }
}

/// The number of bytes a lead byte announces its sequence to be, or `None` if it
/// isn't a valid UTF-8 lead byte.
fn expected_length(lead: u8) -> Option<usize> {
    match lead {
        0x00..=0x7F => Some(1),
        0xC2..=0xDF => Some(2),
        0xE0..=0xEF => Some(3),
        0xF0..=0xF4 => Some(4),
        _ => None,
    }
}

/// How many trailing bytes form an incomplete sequence that should be held back,
/// looking at most three bytes back for a lead byte whose sequence isn't yet whole.
fn incomplete_suffix_length(buffer: &[u8]) -> usize {
    for back in 1..=buffer.len().min(3) {
        let byte = buffer[buffer.len() - back];
        if byte & 0xC0 == 0x80 {
            // A continuation byte; keep walking back to the lead.
            continue;
        }
        return match expected_length(byte) {
            Some(length) if length > back => back,
            _ => 0,
        };
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_ascii_passes_straight_through() {
        let mut assembler = Utf8StreamAssembler::new();
        assert_eq!(assembler.feed(b"hello"), "hello");
        assert_eq!(assembler.flush(), "");
    }

    #[test]
    fn a_two_byte_char_split_across_feeds_is_reassembled() {
        // "é" = 0xC3 0xA9.
        let mut assembler = Utf8StreamAssembler::new();
        assert_eq!(assembler.feed(&[0xC3]), "");
        assert_eq!(assembler.feed(&[0xA9]), "é");
    }

    #[test]
    fn a_four_byte_emoji_split_byte_by_byte_is_reassembled() {
        // "😀" = 0xF0 0x9F 0x98 0x80.
        let mut assembler = Utf8StreamAssembler::new();
        assert_eq!(assembler.feed(&[0xF0]), "");
        assert_eq!(assembler.feed(&[0x9F]), "");
        assert_eq!(assembler.feed(&[0x98]), "");
        assert_eq!(assembler.feed(&[0x80]), "😀");
    }

    #[test]
    fn complete_text_before_a_partial_tail_is_emitted() {
        // "ab" then the first byte of "é".
        let mut assembler = Utf8StreamAssembler::new();
        assert_eq!(assembler.feed(&[b'a', b'b', 0xC3]), "ab");
        assert_eq!(assembler.feed(&[0xA9]), "é");
    }

    #[test]
    fn a_multi_char_chunk_decodes_whole() {
        let mut assembler = Utf8StreamAssembler::new();
        assert_eq!(assembler.feed("héllo".as_bytes()), "héllo");
    }

    #[test]
    fn flush_emits_a_dangling_partial_sequence_lossily() {
        let mut assembler = Utf8StreamAssembler::new();
        assert_eq!(assembler.feed(&[0xF0]), "");
        // The stream ended mid-character: flush yields a replacement char.
        assert_eq!(assembler.flush(), "\u{FFFD}");
    }

    #[test]
    fn an_invalid_lead_byte_is_not_held() {
        // 0xFF is never a valid lead; it should be decoded (lossily) now, not held.
        let mut assembler = Utf8StreamAssembler::new();
        assert_eq!(assembler.feed(&[0xFF]), "\u{FFFD}");
        assert!(assembler.flush().is_empty());
    }
}
