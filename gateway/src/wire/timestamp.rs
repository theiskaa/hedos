//! Wire timestamps: the "now" helpers each dialect needs, over the kernel's
//! shared ISO 8601 / epoch-millisecond primitives.

use kernel::time::now_millis;

pub use kernel::time::{iso8601, millis_from_iso8601};

/// The current instant as an ISO 8601 UTC string (Ollama `created_at`).
pub fn now_iso8601() -> String {
    iso8601(now_millis())
}

/// The current time in whole seconds since the Unix epoch (the OpenAI `created`
/// field).
pub fn now_unix_seconds() -> i64 {
    now_millis() / 1000
}

/// The current time in milliseconds since the Unix epoch.
pub fn now_unix_millis() -> i64 {
    now_millis()
}
