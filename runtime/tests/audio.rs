//! Integration tests for the `audio` PCM↔WAV codec: round-trips, WAV header
//! shape, decoding each supported sample format, channel mixdown, peaks, and
//! duration. Public API only.

use runtime::audio::{duration_ms, pcm_from_wav, peaks, wav_from_pcm};

fn pcm(samples: &[f32]) -> Vec<u8> {
    samples.iter().flat_map(|s| s.to_le_bytes()).collect()
}

fn floats(pcm: &[u8]) -> Vec<f32> {
    pcm.chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

#[test]
fn wav_from_pcm_writes_a_valid_header() {
    let wav = wav_from_pcm(&pcm(&[0.0, 0.5, -0.5]), 24_000);
    assert_eq!(&wav[0..4], b"RIFF");
    assert_eq!(&wav[8..12], b"WAVE");
    assert_eq!(&wav[12..16], b"fmt ");
    assert_eq!(u16::from_le_bytes([wav[22], wav[23]]), 1, "one channel");
    assert_eq!(
        u32::from_le_bytes([wav[24], wav[25], wav[26], wav[27]]),
        24_000
    );
    assert_eq!(
        u16::from_le_bytes([wav[34], wav[35]]),
        16,
        "16 bits per sample"
    );
    assert_eq!(&wav[36..40], b"data");
    assert_eq!(wav.len(), 44 + 3 * 2);
}

#[test]
fn pcm_wav_round_trip_is_near_lossless() {
    let original = [0.0, 0.25, -0.25, 0.75, -0.75, 1.0, -1.0];
    let wav = wav_from_pcm(&pcm(&original), 16_000);
    let (decoded, rate) = pcm_from_wav(&wav).unwrap();
    assert_eq!(rate, 16_000);
    let recovered = floats(&decoded);
    assert_eq!(recovered.len(), original.len());
    for (got, want) in recovered.iter().zip(original.iter()) {
        assert!((got - want).abs() < 1e-3, "{got} vs {want}");
    }
}

#[test]
fn values_are_clamped_before_quantization() {
    let wav = wav_from_pcm(&pcm(&[2.0, -2.0]), 8_000);
    let (decoded, _) = pcm_from_wav(&wav).unwrap();
    let recovered = floats(&decoded);
    assert!((recovered[0] - 1.0).abs() < 1e-3);
    assert!((recovered[1] + 1.0).abs() < 1e-3);
}

fn wav(format: u16, bits: u16, channels: u16, sample_rate: u32, data: &[u8]) -> Vec<u8> {
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&format.to_le_bytes());
    wav.extend_from_slice(&channels.to_le_bytes());
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    let block_align = channels * bits / 8;
    wav.extend_from_slice(&sample_rate.saturating_mul(block_align as u32).to_le_bytes());
    wav.extend_from_slice(&block_align.to_le_bytes());
    wav.extend_from_slice(&bits.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(data.len() as u32).to_le_bytes());
    wav.extend_from_slice(data);
    wav
}

#[test]
fn decodes_8_bit_unsigned() {
    let data = [128u8, 255, 0];
    let (decoded, _) = pcm_from_wav(&wav(1, 8, 1, 8_000, &data)).unwrap();
    let got = floats(&decoded);
    assert!((got[0] - 0.0).abs() < 1e-3);
    assert!((got[1] - (127.0 / 128.0)).abs() < 1e-3);
    assert!((got[2] + 1.0).abs() < 1e-3);
}

#[test]
fn decodes_32_bit_float() {
    let data: Vec<u8> = [0.5f32, -0.5]
        .iter()
        .flat_map(|s| s.to_le_bytes())
        .collect();
    let (decoded, _) = pcm_from_wav(&wav(3, 32, 1, 44_100, &data)).unwrap();
    let got = floats(&decoded);
    assert!((got[0] - 0.5).abs() < 1e-6);
    assert!((got[1] + 0.5).abs() < 1e-6);
}

#[test]
fn mixes_stereo_down_to_mono() {
    let mut data = Vec::new();
    data.extend_from_slice(&1000i16.to_le_bytes());
    data.extend_from_slice(&(-1000i16).to_le_bytes());
    let (decoded, _) = pcm_from_wav(&wav(1, 16, 2, 22_050, &data)).unwrap();
    let got = floats(&decoded);
    assert_eq!(
        got.len(),
        1,
        "one interleaved stereo frame becomes one mono sample"
    );
    assert!(got[0].abs() < 1e-3, "left and right cancel");
}

#[test]
fn rejects_non_wav_input() {
    assert!(pcm_from_wav(b"not a wav file at all really").is_none());
    assert!(pcm_from_wav(&[0u8; 10]).is_none());
    let mut unknown = wav(7, 16, 1, 8_000, &[0, 0]);
    unknown[20] = 7;
    assert!(pcm_from_wav(&unknown).is_none(), "unknown format code");
}

#[test]
fn duration_reflects_sample_count() {
    assert_eq!(duration_ms(&pcm(&[0.0; 16_000]), 16_000), 1000);
    assert_eq!(duration_ms(&pcm(&[0.0; 8_000]), 16_000), 500);
    assert_eq!(duration_ms(&pcm(&[0.0; 100]), 0), 0);
}

#[test]
fn peaks_are_normalized_and_bucketed() {
    let mut samples = vec![0.1f32; 100];
    samples[50] = 0.8;
    let result = peaks(&pcm(&samples), 10);
    assert_eq!(result.len(), 10);
    let max = result.iter().copied().fold(0f64, f64::max);
    assert!(
        (max - 1.0).abs() < 1e-9,
        "the loudest bucket normalizes to 1.0"
    );
    assert!(result.iter().all(|&value| (0.0..=1.0).contains(&value)));
}

#[test]
fn peaks_handles_empty_and_silent_input() {
    assert_eq!(peaks(&[], 5), vec![0.0; 5]);
    assert_eq!(peaks(&pcm(&[0.0; 20]), 4), vec![0.0; 4]);
    assert_eq!(peaks(&pcm(&[0.5; 20]), 0), Vec::<f64>::new());
}

#[test]
fn decodes_24_bit_with_sign_extension() {
    let data = [0, 0, 0x80, 0xFF, 0xFF, 0x7F, 0, 0, 0];
    let (decoded, _) = pcm_from_wav(&wav(1, 24, 1, 8_000, &data)).unwrap();
    let got = floats(&decoded);
    assert!((got[0] + 1.0).abs() < 1e-6, "full-negative 24-bit is -1.0");
    assert!((got[1] - (8_388_607.0 / 8_388_608.0)).abs() < 1e-6);
    assert!(got[2].abs() < 1e-6);
}

#[test]
fn decodes_32_bit_signed_int() {
    let mut data = Vec::new();
    data.extend_from_slice(&i32::MIN.to_le_bytes());
    data.extend_from_slice(&(i32::MAX / 2).to_le_bytes());
    let (decoded, _) = pcm_from_wav(&wav(1, 32, 1, 48_000, &data)).unwrap();
    let got = floats(&decoded);
    assert!((got[0] + 1.0).abs() < 1e-6, "i32::MIN maps to -1.0");
    assert!((got[1] - 0.5).abs() < 1e-3);
}

#[test]
fn truncated_fmt_chunk_is_rejected_without_panicking() {
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&40u32.to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&10u32.to_le_bytes());
    wav.extend_from_slice(&[0u8; 10]);
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&4u32.to_le_bytes());
    wav.extend_from_slice(&[0u8; 4]);
    while wav.len() <= 44 {
        wav.push(0);
    }
    assert!(pcm_from_wav(&wav).is_none());
}

#[test]
fn oversized_chunk_size_breaks_cleanly() {
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&100u32.to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&u32::MAX.to_le_bytes());
    while wav.len() <= 44 {
        wav.push(0);
    }
    assert!(
        pcm_from_wav(&wav).is_none(),
        "a chunk larger than the file must not panic"
    );
}

#[test]
fn skips_unknown_and_odd_length_chunks_before_data() {
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    let mut body = Vec::new();
    body.extend_from_slice(b"WAVE");
    body.extend_from_slice(b"fmt ");
    body.extend_from_slice(&16u32.to_le_bytes());
    body.extend_from_slice(&1u16.to_le_bytes());
    body.extend_from_slice(&1u16.to_le_bytes());
    body.extend_from_slice(&8_000u32.to_le_bytes());
    body.extend_from_slice(&16_000u32.to_le_bytes());
    body.extend_from_slice(&2u16.to_le_bytes());
    body.extend_from_slice(&16u16.to_le_bytes());
    body.extend_from_slice(b"LIST");
    body.extend_from_slice(&3u32.to_le_bytes());
    body.extend_from_slice(&[1, 2, 3]);
    body.push(0);
    body.extend_from_slice(b"data");
    body.extend_from_slice(&4u32.to_le_bytes());
    body.extend_from_slice(&100i16.to_le_bytes());
    body.extend_from_slice(&200i16.to_le_bytes());
    wav.extend_from_slice(&(body.len() as u32).to_le_bytes());
    wav.extend_from_slice(&body);

    let (decoded, rate) = pcm_from_wav(&wav).unwrap();
    assert_eq!(rate, 8_000);
    assert_eq!(
        floats(&decoded).len(),
        2,
        "the padded odd chunk kept alignment"
    );
}

#[test]
fn minimal_44_byte_wav_is_rejected() {
    let bytes = wav(1, 16, 1, 8_000, &[]);
    assert_eq!(bytes.len(), 44);
    assert!(
        pcm_from_wav(&bytes).is_none(),
        "a header-only WAV has no samples"
    );
}

#[test]
fn implausible_sample_rate_is_rejected() {
    assert!(pcm_from_wav(&wav(1, 16, 1, u32::MAX, &[0, 0])).is_none());
}
