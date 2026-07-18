//! How to launch and talk to one sidecar process.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

/// The default audio sample rate assumed until a sidecar's ready handshake says
/// otherwise.
pub const DEFAULT_SAMPLE_RATE: i64 = 24_000;

/// Everything needed to spawn a sidecar and drive one request against it.
#[derive(Debug, Clone)]
pub struct SidecarSpec {
    /// The key this sidecar is tracked under; the same spec always addresses the
    /// same process.
    pub runtime_id: String,
    /// The executable to launch.
    pub executable: PathBuf,
    /// Its argv (not including the executable itself).
    pub arguments: Vec<String>,
    /// Environment overrides layered over the scrubbed process environment.
    pub environment: BTreeMap<String, String>,
    /// The working directory to launch in, if any.
    pub working_directory: Option<PathBuf>,
    /// How long to wait for the `ready` handshake (the model loads in this
    /// window).
    pub ready_timeout: Duration,
    /// How long a request may make no progress before the sidecar is killed.
    pub frame_timeout: Duration,
    /// Whether a cancelled stream sends a cooperative `cancel` op (and keeps the
    /// sidecar warm) instead of hard-killing it.
    pub cooperative_cancel: bool,
    /// After a cooperative cancel, how long to wait for the sidecar to
    /// acknowledge before killing it.
    pub cancel_grace_timeout: Duration,
}

impl SidecarSpec {
    /// A spec for `runtime_id` launching `executable` with `arguments` and the
    /// standard timeouts (180 s ready, 600 s frame, 10 s cancel grace,
    /// non-cooperative cancel).
    pub fn new(runtime_id: impl Into<String>, executable: PathBuf, arguments: Vec<String>) -> Self {
        Self {
            runtime_id: runtime_id.into(),
            executable,
            arguments,
            environment: BTreeMap::new(),
            working_directory: None,
            ready_timeout: Duration::from_secs(180),
            frame_timeout: Duration::from_secs(600),
            cooperative_cancel: false,
            cancel_grace_timeout: Duration::from_secs(10),
        }
    }
}
