//! A small standard-base64 decoder for the image `data:` URIs the chat surfaces
//! accept. Hand-rolled to match the kernel's hand-rolled encoder rather than pull
//! in a dependency for one wire concern.

/// Decode a standard RFC 4648 (`+`/`/` alphabet) base64 string, or `None` if it
/// is malformed. Strict: the input must be a multiple of four characters with
/// padding only at the end, and no stray characters (including whitespace).
pub fn decode(input: &str) -> Option<Vec<u8>> {
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
                // Padding is only valid in the last two positions, and once it
                // starts every remaining character must also be padding.
                if index < 2 {
                    return None;
                }
                pad += 1;
            } else {
                if pad > 0 {
                    return None;
                }
                accumulator |= sextet(byte)? as u32;
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

/// The 6-bit value of a base64 alphabet character, or `None` if it isn't one.
fn sextet(byte: u8) -> Option<u8> {
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
    fn decodes_known_vectors() {
        assert_eq!(decode("").unwrap(), b"");
        assert_eq!(decode("Zg==").unwrap(), b"f");
        assert_eq!(decode("Zm8=").unwrap(), b"fo");
        assert_eq!(decode("Zm9v").unwrap(), b"foo");
        assert_eq!(decode("Zm9vYg==").unwrap(), b"foob");
        assert_eq!(decode("Zm9vYmFy").unwrap(), b"foobar");
    }

    #[test]
    fn round_trips_all_byte_values() {
        // A tiny reference encoder to check the decoder against.
        let source: Vec<u8> = (0u8..=255).collect();
        const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut encoded = String::new();
        for chunk in source.chunks(3) {
            let b0 = chunk[0] as u32;
            let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
            let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
            let triple = (b0 << 16) | (b1 << 8) | b2;
            encoded.push(A[((triple >> 18) & 0x3f) as usize] as char);
            encoded.push(A[((triple >> 12) & 0x3f) as usize] as char);
            encoded.push(if chunk.len() > 1 {
                A[((triple >> 6) & 0x3f) as usize] as char
            } else {
                '='
            });
            encoded.push(if chunk.len() > 2 {
                A[(triple & 0x3f) as usize] as char
            } else {
                '='
            });
        }
        assert_eq!(decode(&encoded).unwrap(), source);
    }

    #[test]
    fn rejects_malformed_input() {
        assert_eq!(decode("Zg="), None); // wrong length
        assert_eq!(decode("Zg=A"), None); // char after padding
        assert_eq!(decode("===="), None); // padding in positions 0/1
        assert_eq!(decode("Zm9v!==="), None); // stray character, length a multiple of 4
        assert_eq!(decode("Zm9 "), None); // embedded whitespace, length a multiple of 4
    }
}
