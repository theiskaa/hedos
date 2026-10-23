//! The llama.cpp build a release build of hedos carries inside itself: the
//! pinned `llama-server` and the libraries it loads, cut down from the release
//! archive and compressed with xz by the build script, which checked the
//! archive against the pin. A build made without one carries nothing.

use kernel::engines::llama_cpp::Variant;

/// The llama.cpp build this binary carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bundled {
    /// The cut-down build, as a tar compressed with xz.
    pub archive: &'static [u8],
    /// The sha256 of `archive`, lowercase hex, as the build wrote it.
    pub sha256: &'static str,
    /// The build it is.
    pub variant: Variant,
}

/// The llama.cpp build this binary carries, if it carries one.
#[cfg(hedos_bundled_llama)]
pub fn bundled() -> Option<Bundled> {
    Some(Bundled {
        archive: include_bytes!(concat!(env!("OUT_DIR"), "/llama-server.tar.xz")),
        sha256: env!("HEDOS_BUNDLED_LLAMA_SHA256"),
        variant: Variant::parse(env!("HEDOS_BUNDLED_LLAMA_VARIANT"))?,
    })
}

/// The llama.cpp build this binary carries: none, since it was built without
/// one.
#[cfg(not(hedos_bundled_llama))]
pub fn bundled() -> Option<Bundled> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(hedos_bundled_llama))]
    #[test]
    fn a_build_made_without_an_archive_carries_nothing() {
        assert_eq!(bundled(), None);
    }

    #[cfg(hedos_bundled_llama)]
    #[test]
    fn a_build_made_with_one_carries_what_its_digest_says() {
        use sha2::{Digest, Sha256};
        let carried = bundled().expect("a bundle");
        assert_eq!(hex::encode(Sha256::digest(carried.archive)), carried.sha256);
    }
}
