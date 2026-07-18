//! Integration tests for the sidecar `frame_codec`: encode/decode round-trips,
//! frames split across appends, multiple frames per append, and the error cases
//! (oversized, unknown type, malformed control). Public API only.

use kernel::records::JsonValue;
use runtime::frame_codec::{Decoder, Frame, FrameError, MAX_FRAME_BYTES, encode};

fn control(json: &str) -> Frame {
    Frame::Control(serde_json::from_str(json).unwrap())
}

#[test]
fn control_frame_round_trips() {
    let frame = control(r#"{"event":"ready","sample_rate":24000}"#);
    let bytes = encode(&frame).unwrap();
    let mut decoder = Decoder::new();
    assert_eq!(decoder.append(&bytes).unwrap(), vec![frame]);
}

#[test]
fn binary_frame_round_trips() {
    let frame = Frame::Binary(vec![0, 1, 2, 250, 255]);
    let bytes = encode(&frame).unwrap();
    let mut decoder = Decoder::new();
    assert_eq!(decoder.append(&bytes).unwrap(), vec![frame]);
}

#[test]
fn encoded_length_counts_type_byte_and_payload() {
    let bytes = encode(&Frame::Binary(vec![9, 9, 9])).unwrap();
    let length = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    assert_eq!(length, 4, "3 payload bytes plus the type byte");
    assert_eq!(bytes.len(), 4 + 4);
}

#[test]
fn a_frame_split_across_appends_is_buffered() {
    let bytes = encode(&control(r#"{"event":"text","text":"hi"}"#)).unwrap();
    let (head, tail) = bytes.split_at(3);
    let mut decoder = Decoder::new();
    assert!(
        decoder.append(head).unwrap().is_empty(),
        "incomplete frame yields nothing"
    );
    let frames = decoder.append(tail).unwrap();
    assert_eq!(frames.len(), 1);
}

#[test]
fn multiple_frames_in_one_append_are_all_returned() {
    let mut wire = Vec::new();
    wire.extend(encode(&control(r#"{"n":1}"#)).unwrap());
    wire.extend(encode(&Frame::Binary(vec![7])).unwrap());
    wire.extend(encode(&control(r#"{"n":2}"#)).unwrap());
    let mut decoder = Decoder::new();
    let frames = decoder.append(&wire).unwrap();
    assert_eq!(frames.len(), 3);
    assert_eq!(frames[1], Frame::Binary(vec![7]));
}

#[test]
fn byte_at_a_time_feeding_reassembles_frames() {
    let mut wire = Vec::new();
    wire.extend(encode(&control(r#"{"a":true}"#)).unwrap());
    wire.extend(encode(&Frame::Binary(vec![1, 2, 3])).unwrap());
    let mut decoder = Decoder::new();
    let mut frames = Vec::new();
    for byte in wire {
        frames.extend(decoder.append(&[byte]).unwrap());
    }
    assert_eq!(frames.len(), 2);
}

#[test]
fn zero_length_frame_is_rejected() {
    let mut decoder = Decoder::new();
    let bytes = 0u32.to_le_bytes();
    assert!(matches!(
        decoder.append(&bytes),
        Err(FrameError::OversizedFrame(0))
    ));
}

#[test]
fn oversized_length_is_rejected() {
    let mut decoder = Decoder::new();
    let bytes = ((MAX_FRAME_BYTES + 1) as u32).to_le_bytes();
    assert!(matches!(
        decoder.append(&bytes),
        Err(FrameError::OversizedFrame(_))
    ));
}

#[test]
fn unknown_type_is_rejected() {
    let mut decoder = Decoder::new();
    let mut wire = 2u32.to_le_bytes().to_vec();
    wire.push(99);
    wire.push(b'x');
    assert!(matches!(
        decoder.append(&wire),
        Err(FrameError::UnknownType(99))
    ));
}

#[test]
fn malformed_control_json_is_rejected() {
    let mut decoder = Decoder::new();
    let payload = b"{not json";
    let mut wire = ((payload.len() + 1) as u32).to_le_bytes().to_vec();
    wire.push(1);
    wire.extend_from_slice(payload);
    assert!(matches!(
        decoder.append(&wire),
        Err(FrameError::MalformedControl)
    ));
}

#[test]
fn encoding_an_oversized_payload_is_rejected() {
    let payload = vec![0u8; MAX_FRAME_BYTES];
    assert!(matches!(
        encode(&Frame::Binary(payload)),
        Err(FrameError::OversizedFrame(_))
    ));
}

#[test]
fn empty_append_yields_no_frames() {
    let mut decoder = Decoder::new();
    assert!(decoder.append(&[]).unwrap().is_empty());
}

#[test]
fn control_frame_preserves_int_vs_float() {
    let frame = Frame::Control(JsonValue::Array(vec![
        JsonValue::Int(3),
        JsonValue::Double(3.5),
    ]));
    let bytes = encode(&frame).unwrap();
    assert_eq!(Decoder::new().append(&bytes).unwrap(), vec![frame]);
}
