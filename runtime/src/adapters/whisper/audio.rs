//! Decoding the audio a transcribe request carries into mono float samples.
//! Deliberately its own strict WAV decoder — 16-bit integer PCM and 32-bit float
//! only, rejecting everything else — rather than the more permissive
//! `crate::audio` decoder, so transcription accepts a tightly-defined input set.

use kernel::records::JsonValue;

use super::expand_tilde;
use crate::util::base64_decode;

/// Why a transcribe payload could not be turned into audio.
#[derive(Debug, thiserror::Error)]
pub enum TranscriptionError {
    /// The payload was missing, malformed, or in an unsupported encoding.
    #[error("{0}")]
    PayloadInvalid(String),
}

/// Mono float audio samples with the sample rate they were captured at.
#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptionAudio {
    /// The mono samples, one per frame.
    pub samples: Vec<f32>,
    /// The samples' sample rate in Hz.
    pub sample_rate: i64,
}

struct WavFormat {
    audio_format: u16,
    channels: usize,
    sample_rate: i64,
    bits_per_sample: usize,
}

impl TranscriptionAudio {
    /// Decode the audio a transcribe `payload` carries: a WAV file at its `audio`
    /// path, or base64 `pcm` float frames at an explicit `sampleRate`.
    pub fn from(payload: &JsonValue) -> Result<Self, TranscriptionError> {
        let Some(fields) = payload.as_object() else {
            return Err(TranscriptionError::PayloadInvalid(
                "transcribe payload must be an object".to_owned(),
            ));
        };
        if let Some(path) = fields.get("audio").and_then(JsonValue::as_str) {
            return Self::from_wav_file(&expand_tilde(path));
        }
        if let Some(base64) = fields.get("pcm").and_then(JsonValue::as_str) {
            let Some(data) = base64_decode(base64) else {
                return Err(TranscriptionError::PayloadInvalid(
                    "transcribe pcm payload is not valid base64".to_owned(),
                ));
            };
            // Strict: only an integer `sampleRate` is accepted — a float-typed
            // rate is rejected, not coerced (`as_i64` would truncate a `Double`).
            let rate = match fields.get("sampleRate") {
                Some(JsonValue::Int(rate)) if *rate > 0 => *rate,
                _ => {
                    return Err(TranscriptionError::PayloadInvalid(
                        "transcribe pcm payload needs a sampleRate".to_owned(),
                    ));
                }
            };
            return Ok(Self {
                samples: float_samples(&data),
                sample_rate: rate,
            });
        }
        Err(TranscriptionError::PayloadInvalid(
            "transcribe payload must carry an audio path or pcm frames".to_owned(),
        ))
    }

    fn from_wav_file(path: &str) -> Result<Self, TranscriptionError> {
        let data = std::fs::read(path).map_err(|_| {
            TranscriptionError::PayloadInvalid(format!("could not read audio at {path}"))
        })?;
        Self::from_wav_data(&data)
    }

    /// Decode a RIFF/WAVE byte buffer into mono samples.
    pub fn from_wav_data(data: &[u8]) -> Result<Self, TranscriptionError> {
        if data.len() < 12 || &data[0..4] != b"RIFF" || &data[8..12] != b"WAVE" {
            return Err(TranscriptionError::PayloadInvalid(
                "audio is not a RIFF WAVE file".to_owned(),
            ));
        }
        let mut offset = 12;
        let mut format: Option<WavFormat> = None;
        while offset + 8 <= data.len() {
            let chunk_id = &data[offset..offset + 4];
            let chunk_size = read_u32(data, offset + 4) as usize;
            let body = offset + 8;
            if body + chunk_size > data.len() {
                break;
            }
            if chunk_id == b"fmt " && chunk_size >= 16 {
                format = Some(WavFormat {
                    audio_format: read_u16(data, body),
                    channels: read_u16(data, body + 2) as usize,
                    sample_rate: read_u32(data, body + 4) as i64,
                    bits_per_sample: read_u16(data, body + 14) as usize,
                });
            }
            if chunk_id == b"data" {
                let format = match &format {
                    Some(format) if format.channels > 0 && format.sample_rate > 0 => format,
                    _ => {
                        return Err(TranscriptionError::PayloadInvalid(
                            "wave data appears before fmt chunk".to_owned(),
                        ));
                    }
                };
                let interleaved = decode_samples(&data[body..body + chunk_size], format)?;
                return Ok(Self {
                    samples: downmixed(&interleaved, format.channels),
                    sample_rate: format.sample_rate,
                });
            }
            offset = body + chunk_size + (chunk_size % 2);
        }
        Err(TranscriptionError::PayloadInvalid(
            "wave file has no data chunk".to_owned(),
        ))
    }

    /// The samples resampled to `target_sample_rate` by linear interpolation, or
    /// unchanged if already at that rate (or empty / target non-positive).
    pub fn mono_samples(&self, target_sample_rate: i64) -> Vec<f32> {
        if self.sample_rate == target_sample_rate
            || self.samples.is_empty()
            || target_sample_rate <= 0
        {
            return self.samples.clone();
        }
        let ratio = self.sample_rate as f64 / target_sample_rate as f64;
        let count = ((self.samples.len() as f64 / ratio) as usize).max(1);
        let last = self.samples.len() - 1;
        let mut resampled = Vec::with_capacity(count);
        for index in 0..count {
            let position = index as f64 * ratio;
            let lower = (position as usize).min(last);
            let upper = (lower + 1).min(last);
            let fraction = (position - lower as f64) as f32;
            resampled.push(self.samples[lower] * (1.0 - fraction) + self.samples[upper] * fraction);
        }
        resampled
    }
}

/// Little-endian 32-bit float samples from a byte buffer (trailing partial sample
/// dropped).
fn float_samples(data: &[u8]) -> Vec<f32> {
    data.chunks_exact(4)
        .map(|bytes| f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
        .collect()
}

fn decode_samples(payload: &[u8], format: &WavFormat) -> Result<Vec<f32>, TranscriptionError> {
    match (format.audio_format, format.bits_per_sample) {
        (1, 16) => Ok(payload
            .chunks_exact(2)
            .map(|bytes| i16::from_le_bytes([bytes[0], bytes[1]]) as f32 / i16::MAX as f32)
            .collect()),
        (3, 32) => Ok(float_samples(payload)),
        _ => Err(TranscriptionError::PayloadInvalid(format!(
            "unsupported wave encoding (format {}, {}-bit)",
            format.audio_format, format.bits_per_sample
        ))),
    }
}

/// Average `channels` interleaved samples down to mono.
fn downmixed(interleaved: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return interleaved.to_vec();
    }
    let frames = interleaved.len() / channels;
    (0..frames)
        .map(|frame| {
            let sum: f32 = (0..channels)
                .map(|ch| interleaved[frame * channels + ch])
                .sum();
            sum / channels as f32
        })
        .collect()
}

fn read_u16(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([data[offset], data[offset + 1]])
}

fn read_u32(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn wav_16bit_mono(sample_rate: u32, samples: &[i16]) -> Vec<u8> {
        let mut out = Vec::new();
        let data_len = (samples.len() * 2) as u32;
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data_len).to_le_bytes());
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(b"fmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes()); // PCM
        out.extend_from_slice(&1u16.to_le_bytes()); // mono
        out.extend_from_slice(&sample_rate.to_le_bytes());
        out.extend_from_slice(&(sample_rate * 2).to_le_bytes()); // byte rate
        out.extend_from_slice(&2u16.to_le_bytes()); // block align
        out.extend_from_slice(&16u16.to_le_bytes()); // bits
        out.extend_from_slice(b"data");
        out.extend_from_slice(&data_len.to_le_bytes());
        for sample in samples {
            out.extend_from_slice(&sample.to_le_bytes());
        }
        out
    }

    #[test]
    fn it_decodes_16bit_pcm_wav_to_normalized_mono_floats() {
        let wav = wav_16bit_mono(16_000, &[0, i16::MAX, i16::MIN]);
        let audio = TranscriptionAudio::from_wav_data(&wav).unwrap();
        assert_eq!(audio.sample_rate, 16_000);
        assert_eq!(audio.samples.len(), 3);
        assert!((audio.samples[0] - 0.0).abs() < 1e-6);
        assert!((audio.samples[1] - 1.0).abs() < 1e-4);
        assert!(audio.samples[2] < -0.9);
    }

    #[test]
    fn it_rejects_a_non_riff_buffer() {
        assert!(TranscriptionAudio::from_wav_data(b"not a wav at all").is_err());
    }

    #[test]
    fn it_rejects_an_unsupported_wav_encoding() {
        // 8-bit PCM: valid RIFF but an encoding the strict decoder refuses.
        let mut wav = wav_16bit_mono(16_000, &[1, 2]);
        // Patch bits-per-sample (offset 34) to 8 and audio-format stays 1.
        wav[34] = 8;
        let error = TranscriptionAudio::from_wav_data(&wav).unwrap_err();
        assert!(format!("{error}").contains("unsupported wave encoding"));
    }

    #[test]
    fn it_downmixes_stereo_to_mono() {
        // Stereo interleaved L,R per frame; averaged to mono.
        let interleaved = [1.0f32, 3.0, 2.0, 4.0];
        assert_eq!(downmixed(&interleaved, 2), vec![2.0, 3.0]);
    }

    #[test]
    fn from_reads_the_pcm_base64_path() {
        // Two f32 LE samples (1.0, -1.0) → base64.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&1.0f32.to_le_bytes());
        bytes.extend_from_slice(&(-1.0f32).to_le_bytes());
        let base64 = encode_base64(&bytes);
        let mut fields = BTreeMap::new();
        fields.insert("pcm".to_owned(), JsonValue::String(base64));
        fields.insert("sampleRate".to_owned(), JsonValue::Int(22_050));
        let audio = TranscriptionAudio::from(&JsonValue::Object(fields)).unwrap();
        assert_eq!(audio.sample_rate, 22_050);
        assert_eq!(audio.samples, vec![1.0, -1.0]);
    }

    #[test]
    fn from_rejects_pcm_without_a_sample_rate() {
        let mut fields = BTreeMap::new();
        fields.insert("pcm".to_owned(), JsonValue::String("AAAAAA==".to_owned()));
        let error = TranscriptionAudio::from(&JsonValue::Object(fields)).unwrap_err();
        assert!(format!("{error}").contains("sampleRate"));
    }

    #[test]
    fn from_rejects_a_float_typed_sample_rate() {
        // Only an integer rate is accepted; a float-typed rate is rejected.
        let mut fields = BTreeMap::new();
        fields.insert("pcm".to_owned(), JsonValue::String("AAAAAA==".to_owned()));
        fields.insert("sampleRate".to_owned(), JsonValue::Double(22_050.0));
        let error = TranscriptionAudio::from(&JsonValue::Object(fields)).unwrap_err();
        assert!(format!("{error}").contains("sampleRate"));
    }

    #[test]
    fn mono_samples_resamples_by_ratio() {
        let audio = TranscriptionAudio {
            samples: vec![0.0, 1.0, 2.0, 3.0],
            sample_rate: 8_000,
        };
        // Downsample 8k→4k halves the count.
        let resampled = audio.mono_samples(4_000);
        assert_eq!(resampled.len(), 2);
        // Same rate is a passthrough.
        assert_eq!(audio.mono_samples(8_000), audio.samples);
    }

    /// Standard base64 encode, for building test payloads.
    fn encode_base64(bytes: &[u8]) -> String {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let b0 = chunk[0] as u32;
            let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
            let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
            let triple = (b0 << 16) | (b1 << 8) | b2;
            out.push(ALPHABET[((triple >> 18) & 0x3f) as usize] as char);
            out.push(ALPHABET[((triple >> 12) & 0x3f) as usize] as char);
            out.push(if chunk.len() > 1 {
                ALPHABET[((triple >> 6) & 0x3f) as usize] as char
            } else {
                '='
            });
            out.push(if chunk.len() > 2 {
                ALPHABET[(triple & 0x3f) as usize] as char
            } else {
                '='
            });
        }
        out
    }
}
