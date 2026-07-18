//! A minimal GGUF header reader: enough to pull the architecture, context
//! length, and chat-template presence without loading the weights.
//!
//! Values are little-endian. The reader streams over a buffered file handle and
//! seeks past values it does not need, so it never reads the tensor data.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use crate::resolution::format::GgufFacts;

const MAX_KV_PAIRS: u64 = 512;
const MAX_STRING_LEN: u64 = 1 << 16;
const MAX_STRING_ARRAY_LEN: u64 = 1 << 24;

const TYPE_UINT32: u32 = 4;
const TYPE_INT32: u32 = 5;
const TYPE_STRING: u32 = 8;
const TYPE_ARRAY: u32 = 9;
const TYPE_UINT64: u32 = 10;
const TYPE_INT64: u32 = 11;

/// Whether the file begins with the GGUF magic bytes.
pub fn has_gguf_magic(path: &Path) -> bool {
    first_four(path) == Some(*b"GGUF")
}

/// Whether the file begins with the legacy GGML magic bytes (`lmgg` is the
/// little-endian byte order of the `ggml` magic).
pub fn has_ggml_magic(path: &Path) -> bool {
    first_four(path) == Some(*b"lmgg")
}

/// The `general.architecture` value from a GGUF header, if any.
pub fn gguf_general_architecture(path: &Path) -> Option<String> {
    gguf_facts(path)?.architecture
}

/// Read the architecture, context length, and chat-template presence from a GGUF
/// header. Returns `None` if the file is not a valid GGUF (v2+) header.
pub fn gguf_facts(path: &Path) -> Option<GgufFacts> {
    let mut reader = Reader::open(path)?;
    if reader.read_array::<4>()? != *b"GGUF" {
        return None;
    }
    let version = reader.read_u32()?;
    if version < 2 {
        return None;
    }
    let _tensor_count = reader.read_u64()?;
    let kv_count = reader.read_u64()?;

    let mut architecture: Option<String> = None;
    let mut context_lengths: BTreeMap<String, i64> = BTreeMap::new();
    let mut has_chat_template = false;

    for _ in 0..kv_count.min(MAX_KV_PAIRS) {
        let Some(key) = reader.read_string() else {
            break;
        };
        let Some(value_type) = reader.read_u32() else {
            break;
        };

        if key == "general.architecture" {
            if value_type == TYPE_STRING {
                let Some(value) = reader.read_string() else {
                    break;
                };
                architecture = Some(value);
            } else if !reader.skip_value(value_type) {
                break;
            }
        } else if key == "tokenizer.chat_template" {
            has_chat_template = true;
            if !reader.skip_value(value_type) {
                break;
            }
        } else if key.ends_with(".context_length") {
            match read_integer(&mut reader, value_type) {
                Some(value) => {
                    if value > 0 {
                        context_lengths.insert(key, value);
                    }
                }
                None => {
                    if !reader.skip_value(value_type) {
                        break;
                    }
                }
            }
        } else if !reader.skip_value(value_type) {
            break;
        }
    }

    let context_length = architecture
        .as_ref()
        .and_then(|arch| {
            context_lengths
                .get(&format!("{arch}.context_length"))
                .copied()
        })
        .or_else(|| {
            if context_lengths.len() == 1 {
                context_lengths.values().copied().next()
            } else {
                None
            }
        });

    Some(GgufFacts {
        architecture,
        context_length,
        has_chat_template,
    })
}

fn first_four(path: &Path) -> Option<[u8; 4]> {
    let mut file = File::open(path).ok()?;
    let mut buffer = [0u8; 4];
    file.read_exact(&mut buffer).ok()?;
    Some(buffer)
}

fn read_integer(reader: &mut Reader, value_type: u32) -> Option<i64> {
    match value_type {
        TYPE_UINT32 => reader.read_u32().map(i64::from),
        TYPE_INT32 => reader.read_i32().map(i64::from),
        TYPE_UINT64 => reader
            .read_u64()
            .map(|value| value.min(i64::MAX as u64) as i64),
        TYPE_INT64 => reader.read_i64(),
        _ => None,
    }
}

fn scalar_width(value_type: u32) -> Option<u64> {
    match value_type {
        0 | 1 | 7 => Some(1), // uint8 / int8 / bool
        2..=3 => Some(2),     // uint16 / int16
        4..=6 => Some(4),     // uint32 / int32 / float32
        10..=12 => Some(8),   // uint64 / int64 / float64
        _ => None,
    }
}

struct Reader {
    inner: BufReader<File>,
}

impl Reader {
    fn open(path: &Path) -> Option<Self> {
        Some(Self {
            inner: BufReader::new(File::open(path).ok()?),
        })
    }

    fn read_bytes(&mut self, count: usize) -> Option<Vec<u8>> {
        let mut buffer = vec![0u8; count];
        self.inner.read_exact(&mut buffer).ok()?;
        Some(buffer)
    }

    fn read_array<const N: usize>(&mut self) -> Option<[u8; N]> {
        let mut buffer = [0u8; N];
        self.inner.read_exact(&mut buffer).ok()?;
        Some(buffer)
    }

    fn read_u32(&mut self) -> Option<u32> {
        self.read_array::<4>().map(u32::from_le_bytes)
    }

    fn read_i32(&mut self) -> Option<i32> {
        self.read_array::<4>().map(i32::from_le_bytes)
    }

    fn read_u64(&mut self) -> Option<u64> {
        self.read_array::<8>().map(u64::from_le_bytes)
    }

    fn read_i64(&mut self) -> Option<i64> {
        self.read_array::<8>().map(i64::from_le_bytes)
    }

    fn read_string(&mut self) -> Option<String> {
        let length = self.read_u64()?;
        if length > MAX_STRING_LEN {
            return None;
        }
        let bytes = self.read_bytes(length as usize)?;
        Some(String::from_utf8_lossy(&bytes).into_owned())
    }

    fn skip(&mut self, count: u64) -> bool {
        if count == 0 {
            return true;
        }
        if count > i64::MAX as u64 {
            return false;
        }
        self.inner.seek(SeekFrom::Current(count as i64)).is_ok()
    }

    fn skip_value(&mut self, value_type: u32) -> bool {
        if let Some(width) = scalar_width(value_type) {
            return self.skip(width);
        }
        match value_type {
            TYPE_STRING => match self.read_u64() {
                Some(length) => self.skip(length),
                None => false,
            },
            TYPE_ARRAY => self.skip_array(),
            _ => false,
        }
    }

    fn skip_array(&mut self) -> bool {
        let Some(element_type) = self.read_u32() else {
            return false;
        };
        let Some(count) = self.read_u64() else {
            return false;
        };
        if let Some(width) = scalar_width(element_type) {
            return match count.checked_mul(width) {
                Some(total) => self.skip(total),
                None => false,
            };
        }
        if element_type != TYPE_STRING || count > MAX_STRING_ARRAY_LEN {
            return false;
        }
        for _ in 0..count {
            let Some(length) = self.read_u64() else {
                return false;
            };
            if !self.skip(length) {
                return false;
            }
        }
        true
    }
}
