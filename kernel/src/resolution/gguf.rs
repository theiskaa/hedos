//! A minimal GGUF header reader: enough to pull the architecture, context
//! length, chat-template presence, file type, pooling, decision-model marker,
//! and classifier head without loading the weights.
//!
//! Values are little-endian. The reader streams over a buffered file handle and
//! seeks past values it does not need, so it never reads the tensor data: the
//! tensor names it looks at sit in the header, ahead of the data.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use crate::resolution::format::{GgufFacts, GgufPooling};

const MAX_KV_PAIRS: u64 = 512;
const MAX_STRING_LEN: u64 = 1 << 16;
const MAX_STRING_ARRAY_LEN: u64 = 1 << 24;
const MAX_TENSORS: u64 = 1 << 16;
/// llama.cpp's `GGML_MAX_NAME`: it refuses a file naming a tensor in this many
/// bytes or more, so a longer name is malformed, and the tensor infos read for
/// a classifier head stay a few MiB at most.
const MAX_TENSOR_NAME: u64 = 64;
const MAX_TENSOR_DIMS: u32 = 8;

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

/// Read the architecture, context length, chat-template presence, the
/// quantization, the pooling, the decision type, and whether a classifier head
/// is among the tensors from a GGUF header. Returns `None` if the file is not a
/// valid GGUF (v2+) header.
pub fn gguf_facts(path: &Path) -> Option<GgufFacts> {
    let mut reader = Reader::open(path)?;
    if reader.read_array::<4>()? != *b"GGUF" {
        return None;
    }
    let version = reader.read_u32()?;
    if version < 2 {
        return None;
    }
    let tensor_count = reader.read_u64()?;
    let kv_count = reader.read_u64()?;

    let mut architecture: Option<String> = None;
    let mut context_lengths: BTreeMap<String, i64> = BTreeMap::new();
    let mut poolings: BTreeMap<String, i64> = BTreeMap::new();
    let mut decisions: BTreeMap<String, String> = BTreeMap::new();
    let mut has_chat_template = false;
    let mut file_type: Option<i64> = None;
    let mut kvs_read = 0u64;

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
        } else if key == "general.file_type" {
            match read_integer(&mut reader, value_type) {
                Some(value) => file_type = Some(value),
                None => {
                    if !reader.skip_value(value_type) {
                        break;
                    }
                }
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
        } else if is_model_key(&key, "pooling_type") {
            match read_integer(&mut reader, value_type) {
                Some(value) => {
                    poolings.insert(key, value);
                }
                None => {
                    if !reader.skip_value(value_type) {
                        break;
                    }
                }
            }
        } else if is_model_key(&key, "decision.type") && value_type == TYPE_STRING {
            let Some(value) = reader.read_string() else {
                break;
            };
            decisions.insert(key, value);
        } else if !reader.skip_value(value_type) {
            break;
        }
        kvs_read += 1;
    }
    // The tensor infos follow the last value, so they can be found only when
    // every value before them was read or skipped.
    let has_classifier_head = kvs_read == kv_count && reader.has_classifier_head(tensor_count);

    let context_length = own_value(architecture.as_deref(), "context_length", &context_lengths)
        .or_else(|| lone_value(&context_lengths));
    // llama.cpp reads pooling and the decision type only under the file's own
    // architecture, so a key named for another one says nothing about it.
    let pooling = own_value(architecture.as_deref(), "pooling_type", &poolings)
        .and_then(GgufPooling::from_llama);
    let decision = own_value(architecture.as_deref(), "decision.type", &decisions);

    Some(GgufFacts {
        architecture,
        context_length,
        has_chat_template,
        quantization: file_type.and_then(file_type_name).map(str::to_owned),
        pooling,
        decision,
        has_classifier_head,
    })
}

/// Whether `key` is `{arch}.{name}` for some architecture, and not a key nested
/// deeper under it: `modern-bert.classifier.pooling_type` describes a
/// classifier head, not how the model itself pools.
fn is_model_key(key: &str, name: &str) -> bool {
    key.strip_suffix(name)
        .and_then(|prefix| prefix.strip_suffix('.'))
        .is_some_and(|arch| !arch.is_empty() && !arch.contains('.'))
}

/// The `{architecture}.{suffix}` entry of `values`.
fn own_value<T: Clone>(
    architecture: Option<&str>,
    suffix: &str,
    values: &BTreeMap<String, T>,
) -> Option<T> {
    architecture.and_then(|arch| values.get(&format!("{arch}.{suffix}")).cloned())
}

/// The only entry of `values`; several entries are no answer.
fn lone_value<T: Clone>(values: &BTreeMap<String, T>) -> Option<T> {
    if values.len() == 1 {
        values.values().next().cloned()
    } else {
        None
    }
}

/// llama.cpp's guessed-type bit: set when the converter inferred the file type
/// rather than being told it, and no part of the type itself.
const FILE_TYPE_GUESSED: i64 = 1024;

/// The name llama.cpp gives a `general.file_type` value (its `llama_ftype`
/// enum); `None` for a value the table does not know, the retired ones
/// included.
fn file_type_name(value: i64) -> Option<&'static str> {
    Some(match value & !FILE_TYPE_GUESSED {
        0 => "F32",
        1 => "F16",
        2 => "Q4_0",
        3 => "Q4_1",
        7 => "Q8_0",
        8 => "Q5_0",
        9 => "Q5_1",
        10 => "Q2_K",
        11 => "Q3_K_S",
        12 => "Q3_K_M",
        13 => "Q3_K_L",
        14 => "Q4_K_S",
        15 => "Q4_K_M",
        16 => "Q5_K_S",
        17 => "Q5_K_M",
        18 => "Q6_K",
        19 => "IQ2_XXS",
        20 => "IQ2_XS",
        21 => "Q2_K_S",
        22 => "IQ3_XS",
        23 => "IQ3_XXS",
        24 => "IQ1_S",
        25 => "IQ4_NL",
        26 => "IQ3_S",
        27 => "IQ3_M",
        28 => "IQ2_S",
        29 => "IQ2_M",
        30 => "IQ4_XS",
        31 => "IQ1_M",
        32 => "BF16",
        36 => "TQ1_0",
        37 => "TQ2_0",
        38 => "MXFP4_MOE",
        _ => return None,
    })
}

fn first_four(path: &Path) -> Option<[u8; 4]> {
    let mut file = crate::fs::open_regular(path).ok()?;
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
            inner: BufReader::new(crate::fs::open_regular(path).ok()?),
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
        // Relative, so a skip that lands inside what is buffered keeps the
        // buffer: a vocabulary's hundreds of thousands of short strings then
        // cost no seek and no refill each.
        self.inner.seek_relative(count as i64).is_ok()
    }

    /// Whether one of the next `count` tensor infos names a classifier head.
    /// A malformed info, a name too long included, ends the search with no
    /// head found.
    fn has_classifier_head(&mut self, count: u64) -> bool {
        for _ in 0..count.min(MAX_TENSORS) {
            let Some(length) = self.read_u64().filter(|length| *length < MAX_TENSOR_NAME) else {
                return false;
            };
            let Some(name) = self.read_bytes(length as usize) else {
                return false;
            };
            if name == b"cls.weight" || name == b"cls.output.weight" {
                return true;
            }
            let Some(dims) = self.read_u32().filter(|dims| *dims <= MAX_TENSOR_DIMS) else {
                return false;
            };
            for _ in 0..dims {
                if self.read_u64().is_none() {
                    return false;
                }
            }
            // The element type and the offset into the data.
            if self.read_u32().is_none() || self.read_u64().is_none() {
                return false;
            }
        }
        false
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
