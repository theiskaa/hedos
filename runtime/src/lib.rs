//! The Hedos runtime layer: executing models. Currently the sidecar wire
//! protocol; the supervisor, governor, and job scheduler build on it.

pub mod adapters;
pub mod audio;
pub mod bench;
pub mod boot;
pub mod chip;
pub mod environment;
pub mod facade;
pub mod frame_codec;
pub mod governed;
pub mod governor;
pub mod install;
pub mod jobs;
pub mod manifests;
pub mod process;
pub mod python_runtime;
pub mod removal;
pub mod resolution;
pub mod settings;
pub mod sidecar;
mod sys;
mod time;
mod util;

pub use facade::Kernel;
