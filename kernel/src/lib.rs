//! The Hedos kernel: the headless core that discovers, identifies, installs,
//! and manages local models. Pure logic and the filesystem — no async runtime,
//! no UI. Higher layers (gateway, cli) are thin shells over this crate.

pub mod persistence;
pub mod profiles;
pub mod records;
pub mod registry;

mod util;

pub use registry::{Registry, RegistryError};
