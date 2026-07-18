//! PCM ↔ WAV conversion for generated speech.
//!
//! "PCM" here is a byte buffer of little-endian 32-bit float samples, mono. WAV
//! output is 16-bit mono PCM; WAV input is decoded from 8/16/24/32-bit integer
//! or 32-bit float, with multiple channels mixed down to mono.

/// The largest sample rate accepted when decoding — well above any real audio,
/// but low enough that derived byte-rate math cannot overflow.
const MAX_SAMPLE_RATE: u32 = 4_000_000;

/// Encode float PCM as a 16-bit mono WAV file.
pub fn wav_from_pcm(pcm: &[u8], sample_rate: u32) -> Vec<u8> {
    let samples = int16_samples(pcm);
    let data_size = u32::try_from(samples.len() * 2).unwrap_or(u32::MAX);
    let mut wav = Vec::with_capacity(44 + data_size as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&36u32.saturating_add(data_size).to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&sample_rate.saturating_mul(2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_size.to_le_bytes());
    for sample in samples {
        wav.extend_from_slice(&sample.to_le_bytes());
    }
    wav
}

/// Decode a WAV file to float PCM and its sample rate, mixing any channels down
/// to mono. Returns `None` if the file is not a WAV this decoder understands.
pub fn pcm_from_wav(wav: &[u8]) -> Option<(Vec<u8>, u32)> {
    if wav.len() <= 44 || &wav[0..4] != b"RIFF" || &wav[8..12] != b"WAVE" {
        return None;
    }
    let mut offset = 12;
    let (mut format, mut channels, mut sample_rate, mut bits) = (0u16, 1u16, 0u32, 0u16);
    let mut samples: Option<&[u8]> = None;
    while offset + 8 <= wav.len() {
        let chunk_id = &wav[offset..offset + 4];
        let chunk_size = read_u32(wav, offset + 4) as usize;
        let body = offset + 8;
        let Some(end) = body.checked_add(chunk_size) else {
            break;
        };
        if end > wav.len() {
            break;
        }
        if chunk_id == b"fmt " && chunk_size >= 16 {
            format = read_u16(wav, body);
            channels = read_u16(wav, body + 2);
            sample_rate = read_u32(wav, body + 4);
            bits = read_u16(wav, body + 14);
        } else if chunk_id == b"data" {
            samples = Some(&wav[body..end]);
        }
        let Some(next) = end.checked_add(chunk_size % 2) else {
            break;
        };
        offset = next;
    }

    let samples = samples?;
    if sample_rate == 0 || sample_rate > MAX_SAMPLE_RATE || channels == 0 {
        return None;
    }
    let channel_count = channels as usize;
    let floats = match (format, bits) {
        (1, 16) => mix(samples, 2, channel_count, |b| {
            i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0
        }),
        (3, 32) => mix(samples, 4, channel_count, |b| {
            f32::from_le_bytes([b[0], b[1], b[2], b[3]])
        }),
        (1, 8) => mix(samples, 1, channel_count, |b| {
            (b[0] as i32 - 128) as f32 / 128.0
        }),
        (1, 24) => mix(samples, 3, channel_count, |b| {
            let mut value = b[0] as i32 | (b[1] as i32) << 8 | (b[2] as i32) << 16;
            if value & 0x80_0000 != 0 {
                value |= 0xFF00_0000u32 as i32; // sign-extend the 24-bit sample into i32
            }
            value as f32 / 8_388_608.0
        }),
        (1, 32) => mix(samples, 4, channel_count, |b| {
            let value =
                b[0] as i32 | (b[1] as i32) << 8 | (b[2] as i32) << 16 | (b[3] as i32) << 24;
            value as f32 / 2_147_483_648.0
        }),
        _ => return None,
    };

    let mut pcm = Vec::with_capacity(floats.len() * 4);
    for sample in floats {
        pcm.extend_from_slice(&sample.to_le_bytes());
    }
    Some((pcm, sample_rate))
}

/// Summarize float PCM into `buckets` normalized peak magnitudes in `0.0..=1.0`.
pub fn peaks(pcm: &[u8], buckets: usize) -> Vec<f64> {
    let samples = float_samples(pcm);
    if samples.is_empty() || buckets == 0 {
        return vec![0.0; buckets];
    }
    let bucket_size = (samples.len() / buckets).max(1);
    let mut result = Vec::with_capacity(buckets);
    for index in 0..buckets {
        let start = index * bucket_size;
        if start >= samples.len() {
            result.push(0.0);
            continue;
        }
        let end = (start + bucket_size).min(samples.len());
        let peak = samples[start..end]
            .iter()
            .fold(0f32, |peak, &sample| peak.max(sample.abs()));
        result.push(peak.min(1.0) as f64);
    }
    let top = result.iter().copied().fold(0f64, f64::max);
    if top <= 0.0 {
        return result;
    }
    result.iter().map(|value| value / top).collect()
}

/// The duration of float PCM at `sample_rate`, in milliseconds.
pub fn duration_ms(pcm: &[u8], sample_rate: u32) -> i64 {
    if sample_rate == 0 {
        return 0;
    }
    let samples = (pcm.len() / 4) as i64;
    samples.saturating_mul(1000) / sample_rate as i64
}

fn mix(
    data: &[u8],
    bytes_per_sample: usize,
    channels: usize,
    decode: impl Fn(&[u8]) -> f32,
) -> Vec<f32> {
    let frame = bytes_per_sample * channels;
    if frame == 0 {
        return Vec::new();
    }
    let mut floats = Vec::with_capacity(data.len() / frame);
    let mut offset = 0;
    while offset + frame <= data.len() {
        let mut sum = 0f32;
        for channel in 0..channels {
            let start = offset + channel * bytes_per_sample;
            sum += decode(&data[start..start + bytes_per_sample]);
        }
        floats.push(sum / channels as f32);
        offset += frame;
    }
    floats
}

fn float_samples(pcm: &[u8]) -> Vec<f32> {
    pcm.chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}

fn int16_samples(pcm: &[u8]) -> Vec<i16> {
    pcm.chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .map(|sample| (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
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
