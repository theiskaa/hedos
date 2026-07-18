//! The streamed output vocabulary: `GenerationStats` serde shape and the chunk
//! / job-event value types.

use kernel::capabilities::{AudioFrame, CapabilityChunk, GenerationStats};
use kernel::jobs::JobRuntimeEvent;

#[test]
fn generation_stats_serializes_camel_case_and_omits_none() {
    let stats = GenerationStats {
        prompt_tokens: Some(12),
        completion_tokens: Some(3),
        duration_ms: Some(200),
        ..Default::default()
    };
    let json = serde_json::to_value(&stats).expect("serialize");
    assert_eq!(json["promptTokens"], 12);
    assert_eq!(json["completionTokens"], 3);
    assert_eq!(json["durationMs"], 200);
    assert!(json.get("ttftMs").is_none(), "None fields are omitted");
    assert!(json.get("finishReason").is_none());
    assert_eq!(json["tokenCountsEstimated"], false);
}

#[test]
fn generation_stats_defaults_the_estimated_flag_when_absent() {
    let stats: GenerationStats =
        serde_json::from_str(r#"{"promptTokens": 5}"#).expect("deserialize");
    assert_eq!(stats.prompt_tokens, Some(5));
    assert_eq!(stats.completion_tokens, None);
    assert!(
        !stats.token_counts_estimated,
        "absent flag defaults to false"
    );
}

#[test]
fn generation_stats_round_trips() {
    let stats = GenerationStats {
        prompt_tokens: Some(1),
        completion_tokens: Some(2),
        duration_ms: Some(3),
        ttft_ms: Some(4),
        load_ms: Some(5),
        finish_reason: Some("stop".to_owned()),
        token_counts_estimated: true,
    };
    let json = serde_json::to_string(&stats).expect("serialize");
    let back: GenerationStats = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(stats, back);
}

#[test]
fn capability_chunk_and_audio_frame_carry_their_payloads() {
    let audio = AudioFrame::new(vec![1, 2, 3], 24_000);
    assert_eq!(audio.sample_rate, 24_000);
    let chunk = CapabilityChunk::Audio(audio.clone());
    assert_eq!(chunk, CapabilityChunk::Audio(audio));

    let seg = CapabilityChunk::Segment {
        text: "hi".to_owned(),
        start_ms: 0,
        end_ms: 100,
    };
    assert_ne!(seg, CapabilityChunk::Text("hi".to_owned()));
    assert_eq!(
        CapabilityChunk::Done(Some(GenerationStats::default())),
        CapabilityChunk::Done(Some(GenerationStats::default()))
    );
}

#[test]
fn job_runtime_event_equality() {
    assert_eq!(JobRuntimeEvent::Started, JobRuntimeEvent::Started);
    assert_eq!(
        JobRuntimeEvent::Progress {
            step: 1,
            total_steps: 4
        },
        JobRuntimeEvent::Progress {
            step: 1,
            total_steps: 4
        }
    );
    assert_ne!(
        JobRuntimeEvent::Result {
            data: vec![1],
            file_extension: "png".to_owned(),
        },
        JobRuntimeEvent::Preview(vec![1]),
    );
}
