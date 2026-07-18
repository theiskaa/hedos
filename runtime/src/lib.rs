//! The Hedos runtime layer: executing models. Currently the sidecar wire
//! protocol; the supervisor, governor, and job scheduler build on it.

pub mod adapters;
pub mod audio;
pub mod environment;
pub mod frame_codec;
pub mod governed;
pub mod governor;
pub mod jobs;
pub mod process;
pub mod python_runtime;
pub mod sidecar;
