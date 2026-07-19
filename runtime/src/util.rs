//! Small crate-internal helpers, hand-rolled to avoid pulling in dependencies for
//! trivial work — matching the kernel's own hand-rolled `base64_encode`.

use std::path::Path;

/// The size of the file at `path` in whole mebibytes, or `None` if it can't be
/// stat'd.
pub(crate) fn weights_mb(path: &Path) -> Option<i64> {
    std::fs::metadata(path)
        .ok()
        .map(|meta| (meta.len() / (1 << 20)) as i64)
}

/// Lowercase hex encoding of a byte slice.
pub(crate) fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    out
}

/// Decode a standard RFC 4648 (`+`/`/` alphabet) base64 string, or `None` if it
/// is malformed. Strict: length a multiple of four, padding only at the end, no
/// stray characters.
pub(crate) fn base64_decode(input: &str) -> Option<Vec<u8>> {
    let bytes = input.as_bytes();
    if bytes.is_empty() {
        return Some(Vec::new());
    }
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for chunk in bytes.chunks(4) {
        let mut accumulator: u32 = 0;
        let mut pad = 0u8;
        for (index, &byte) in chunk.iter().enumerate() {
            accumulator <<= 6;
            if byte == b'=' {
                if index < 2 {
                    return None;
                }
                pad += 1;
            } else {
                if pad > 0 {
                    return None;
                }
                accumulator |= base64_sextet(byte)? as u32;
            }
        }
        out.push((accumulator >> 16) as u8);
        if pad < 2 {
            out.push((accumulator >> 8) as u8);
        }
        if pad < 1 {
            out.push(accumulator as u8);
        }
    }
    Some(out)
}

fn base64_sextet(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_roundtrips_and_rejects_malformed_input() {
        assert_eq!(base64_decode("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(base64_decode("Zm9vYmE=").unwrap(), b"fooba");
        assert_eq!(base64_decode("").unwrap(), b"");
        // Not a multiple of four.
        assert!(base64_decode("aGVsbG8").is_none());
        // Stray character.
        assert!(base64_decode("aGVs*G8=").is_none());
        // Padding in an illegal position.
        assert!(base64_decode("a=bc").is_none());
    }
}
