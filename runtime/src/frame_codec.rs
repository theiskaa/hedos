//! The length-prefixed framing used to talk to Python sidecars.
//!
//! Each frame on the wire is a little-endian `u32` length, then a one-byte type,
//! then the payload. The length counts the type byte plus the payload. Type `1`
//! is a control frame carrying JSON; type `2` is a binary frame (audio/image
//! bytes). Frames are capped at 16 MiB.

use kernel::records::JsonValue;

/// The maximum size of a single frame, counting the type byte and payload.
pub const MAX_FRAME_BYTES: usize = 16 << 20;

const TYPE_CONTROL: u8 = 1;
const TYPE_BINARY: u8 = 2;

/// A decoded frame.
#[derive(Debug, Clone, PartialEq)]
pub enum Frame {
    /// A control frame carrying a JSON value.
    Control(JsonValue),
    /// A binary frame carrying raw bytes.
    Binary(Vec<u8>),
}

/// Errors from framing.
#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    /// A frame's declared length is zero or exceeds [`MAX_FRAME_BYTES`].
    #[error("oversized frame: {0} bytes")]
    OversizedFrame(usize),

    /// A frame declared a type byte that is neither control nor binary.
    #[error("unknown frame type: {0}")]
    UnknownType(u8),

    /// A control frame's payload was not valid JSON.
    #[error("malformed control frame")]
    MalformedControl,
}

/// Encode a frame to its on-wire bytes.
pub fn encode(frame: &Frame) -> Result<Vec<u8>, FrameError> {
    let (frame_type, payload) = match frame {
        Frame::Control(value) => {
            let payload = serde_json::to_vec(value).map_err(|_| FrameError::MalformedControl)?;
            (TYPE_CONTROL, payload)
        }
        Frame::Binary(data) => (TYPE_BINARY, data.clone()),
    };
    if payload.len() + 1 > MAX_FRAME_BYTES {
        return Err(FrameError::OversizedFrame(payload.len()));
    }
    let length = (payload.len() + 1) as u32;
    let mut out = Vec::with_capacity(4 + 1 + payload.len());
    out.extend_from_slice(&length.to_le_bytes());
    out.push(frame_type);
    out.extend_from_slice(&payload);
    Ok(out)
}

/// A streaming frame decoder. Bytes are appended as they arrive; whole frames
/// are returned as soon as they are complete, with partial frames buffered.
#[derive(Debug, Default)]
pub struct Decoder {
    buffer: Vec<u8>,
}

impl Decoder {
    /// A fresh decoder with an empty buffer.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append received bytes and return every frame now complete.
    pub fn append(&mut self, data: &[u8]) -> Result<Vec<Frame>, FrameError> {
        self.buffer.extend_from_slice(data);
        let mut frames = Vec::new();
        let mut consumed = 0;
        while self.buffer.len() - consumed >= 4 {
            let header = &self.buffer[consumed..consumed + 4];
            let length = u32::from_le_bytes([header[0], header[1], header[2], header[3]]) as usize;
            if !(1..=MAX_FRAME_BYTES).contains(&length) {
                self.buffer.drain(..consumed);
                return Err(FrameError::OversizedFrame(length));
            }
            if self.buffer.len() - consumed < 4 + length {
                break;
            }
            let body = &self.buffer[consumed + 4..consumed + 4 + length];
            let frame_type = body[0];
            let payload = &body[1..];
            let frame = match frame_type {
                TYPE_CONTROL => {
                    let value = serde_json::from_slice::<JsonValue>(payload)
                        .map_err(|_| FrameError::MalformedControl)?;
                    Frame::Control(value)
                }
                TYPE_BINARY => Frame::Binary(payload.to_vec()),
                other => {
                    self.buffer.drain(..consumed);
                    return Err(FrameError::UnknownType(other));
                }
            };
            frames.push(frame);
            consumed += 4 + length;
        }
        self.buffer.drain(..consumed);
        Ok(frames)
    }
}
