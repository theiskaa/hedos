//! The Hedos kernel: the headless core that discovers, identifies, installs,
//! and manages local models. Pure logic and the filesystem — no async runtime,
//! no UI. Higher layers (gateway, cli) are thin shells over this crate.

pub mod artifacts;
pub mod capabilities;
pub mod discovery;
pub mod install;
pub mod jobs;
pub mod manifests;
pub mod persistence;
pub mod profiles;
pub mod records;
pub mod registry;
pub mod removal;
pub mod resolution;
pub mod time;

pub use registry::{Registry, RegistryError};
