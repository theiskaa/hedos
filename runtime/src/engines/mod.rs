//! The engines hedos runs models on, as this machine has them: where each is,
//! and what version.

pub mod bundled;
pub mod install;
pub mod llama_cpp;

use std::collections::HashMap;
use std::path::PathBuf;

use kernel::engines::llama_cpp::{Asset, PINNED, Variant};
use kernel::machine::Machine;

/// The `ollama` binary hedos would start, if it finds one.
pub fn ollama_binary() -> Option<PathBuf> {
    let environment: HashMap<String, String> = std::env::vars_os()
        .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)))
        .collect();
    crate::install::ollama::daemon_binary(&environment)
}

/// Whether something answers where Ollama's daemon listens.
pub fn ollama_listening() -> bool {
    crate::install::ollama::daemon_listening_at(crate::install::ollama::DEFAULT_BASE_URL)
}

/// The `uv` that prepares the Python runtimes, if there is one.
pub fn uv_binary() -> Option<PathBuf> {
    crate::environment::uv_binary()
}

/// The version of the GNU C library this process runs on (`2.35`).
#[cfg(all(target_os = "linux", target_env = "gnu"))]
pub fn glibc() -> Option<String> {
    // SAFETY: `gnu_get_libc_version` takes nothing and returns a pointer to a
    // NUL-terminated string glibc keeps for the life of the process.
    let version = unsafe { std::ffi::CStr::from_ptr(libc::gnu_get_libc_version()) };
    version.to_str().ok().map(str::to_owned)
}

/// The version of the GNU C library this process runs on: none, off Linux.
#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
pub fn glibc() -> Option<String> {
    None
}

/// The llama.cpp build that suits `machine`, given this system's glibc: on a
/// Linux too old for llama.cpp's own builds, hedos's CPU build.
pub fn variant_for(machine: &Machine) -> Option<Variant> {
    Variant::default_for(machine, glibc().as_deref())
}

/// Whether `asset` starts on this system, or why not and what runs instead:
/// llama.cpp's own Linux builds need a newer glibc than hedos does.
pub fn runs_here(asset: &Asset) -> Result<(), String> {
    let glibc = glibc();
    let Some(reason) = asset.blocked(glibc.as_deref()) else {
        return Ok(());
    };
    let cpu = PINNED
        .asset(asset.os, asset.arch, Variant::Cpu)
        .filter(|cpu| cpu.blocked(glibc.as_deref()).is_none());
    let instead = if bundled::bundled().is_some() {
        "the CPU build hedos carries runs here"
    } else if cpu.is_some() {
        "hedos's own CPU build runs here: hedos engines install llama.cpp --variant cpu"
    } else {
        "build llama.cpp for this system and name its llama-server in `[engines] llama_server`"
    };
    Err(format!("{reason}; {instead}"))
}
