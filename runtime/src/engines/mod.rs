//! The engines hedos runs models on, as this machine has them: where each is,
//! and what version.

pub mod bundled;
pub mod install;
pub mod llama_cpp;

use std::collections::HashMap;
use std::path::PathBuf;

use kernel::engines::llama_cpp::published_build_blocked;
use kernel::machine::Os;

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

/// Whether llama.cpp's own builds, the ones `hedos engines install` fetches,
/// start on this system, or why not and what runs instead. On Linux they need
/// a newer glibc than hedos does.
pub fn published_builds_run_here() -> Result<(), String> {
    if Os::current() != Os::Linux {
        return Ok(());
    }
    let Some(reason) = glibc().as_deref().and_then(published_build_blocked) else {
        return Ok(());
    };
    let instead = if bundled::bundled().is_some() {
        "the CPU build hedos carries runs here"
    } else {
        "build llama.cpp for this system and name its llama-server in `[engines] llama_server`"
    };
    Err(format!("{reason}; {instead}"))
}
