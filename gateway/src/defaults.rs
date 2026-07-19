//! The gateway's fixed defaults: the loopback port, body/connection limits, and
//! the queue-depth and retry tuning shared by the handlers.

use std::ops::RangeInclusive;

/// The default loopback port the gateway listens on.
pub const PORT: u16 = 43367;

/// The range of ports the gateway will accept as a configured override.
pub const PORT_RANGE: RangeInclusive<u16> = 1024..=65535;

/// The maximum number of simultaneous client connections.
pub const MAX_CONNECTIONS: usize = 128;

/// The maximum accepted request body size, in bytes (2 MiB).
pub const MAX_BODY_BYTES: usize = 2_097_152;

/// The most inference requests allowed to queue before the gateway sheds load.
pub const INFERENCE_QUEUE_DEPTH_CAP: usize = 4;

/// The `Retry-After` hint, in seconds, when the runtime is saturated.
pub const SATURATED_RETRY_AFTER_SECONDS: u32 = 1;

/// The `Retry-After` hint, in seconds, when a request is queued behind others.
pub const QUEUED_RETRY_AFTER_SECONDS: u32 = 5;

/// The OpenAI-compatible base URL a client should point at for `port`.
pub fn base_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/v1")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_base_url_is_loopback_v1() {
        assert_eq!(base_url(43367), "http://127.0.0.1:43367/v1");
    }

    #[test]
    fn the_default_port_is_in_range() {
        assert!(PORT_RANGE.contains(&PORT));
    }
}
