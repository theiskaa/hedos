//! uv as hedos installs it on request: the one release a hedos build pins,
//! its build per machine, what hedos writes beside one it installed, and
//! reading the version a `uv` reports. Nothing here downloads or runs
//! anything.

use serde::{Deserialize, Serialize};

use super::InstalledFile;
use crate::machine::Os;

/// The uv release this hedos installs, and its build for each machine with
/// the pinned sha256. The Linux builds are the static musl ones, which start
/// on any glibc and still fetch the glibc Python on a glibc system.
pub const PINNED: Release = Release {
    version: "0.13.4",
    assets: &[
        Asset {
            os: Os::Macos,
            arch: "aarch64",
            file: "uv-aarch64-apple-darwin.tar.gz",
            sha256: "e0b1d4743ce64c55d9f0f4a444afa6b81125680070ee04e853fcd0f6c1ebf8f1",
            bytes: 14_326_193,
        },
        Asset {
            os: Os::Macos,
            arch: "x86_64",
            file: "uv-x86_64-apple-darwin.tar.gz",
            sha256: "7be2dcd316122f4b3e5a0a7889f6248266c14425374e105ff7dc9779184603a6",
            bytes: 17_217_446,
        },
        Asset {
            os: Os::Linux,
            arch: "x86_64",
            file: "uv-x86_64-unknown-linux-musl.tar.gz",
            sha256: "28d4385f9f9d1d1c7654d85b3855e0e2615b2f6cc13136a2edff858bbdc16ff1",
            bytes: 18_450_841,
        },
        Asset {
            os: Os::Linux,
            arch: "aarch64",
            file: "uv-aarch64-unknown-linux-musl.tar.gz",
            sha256: "253a17cadcb659ae77a0fe3b9a2f1c14297528d480c421899e61e64d64f24e8f",
            bytes: 17_045_416,
        },
    ],
};

/// Where uv's release assets are published.
const RELEASES: &str = "https://github.com/astral-sh/uv/releases/download";

/// A uv release as hedos pins it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Release {
    /// The release's version (`0.13.4`), which is also its tag.
    pub version: &'static str,
    /// Its build for each machine hedos installs it on.
    pub assets: &'static [Asset],
}

impl Release {
    /// The build of this release for `os`/`arch`, if there is one.
    pub fn asset(&self, os: Os, arch: &str) -> Option<&'static Asset> {
        self.assets
            .iter()
            .find(|asset| asset.os == os && asset.arch == arch)
    }

    /// Where `file`, one of this release's assets, is downloaded from.
    pub fn url(&self, file: &str) -> String {
        format!("{RELEASES}/{}/{file}", self.version)
    }
}

/// One build of a release: an archive holding `uv` and `uvx` in a directory
/// named for its target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Asset {
    /// The operating system it runs on.
    pub os: Os,
    /// The CPU architecture it runs on (`aarch64`, `x86_64`).
    pub arch: &'static str,
    /// The archive's file name on the release.
    pub file: &'static str,
    /// The archive's sha256, lowercase hex.
    pub sha256: &'static str,
    /// The archive's size in bytes, as the release lists it.
    pub bytes: u64,
}

/// What hedos writes beside a `uv` it installed, in the version's directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Installed {
    /// The release's version, which is also the directory's name.
    pub version: String,
    /// The operating system it was built for ([`Os::as_str`]), so a data
    /// directory carried to another machine does not offer a uv that cannot
    /// start there.
    pub os: String,
    /// The CPU architecture it was built for (`aarch64`, `x86_64`).
    pub arch: String,
    /// The sha256 of the archive it came from.
    pub sha256: String,
    /// When it was installed, in milliseconds since the epoch.
    pub installed_at: i64,
    /// Every file it was put in place with (`uv` and `uvx`), so one lost or
    /// cut short since is not taken for whole.
    pub files: Vec<InstalledFile>,
}

/// The version `uv --version` printed on stdout: `0.13.4` from both
/// `uv 0.13.4 (7456b60bc 2026-10-20 aarch64-apple-darwin)` and
/// `uv 0.13.4 (aarch64-unknown-linux-musl)`.
pub fn parse_version(stdout: &str) -> Option<&str> {
    let mut words = stdout.lines().next()?.split_whitespace();
    if words.next()? != "uv" {
        return None;
    }
    let version = words.next()?;
    well_formed(version).then_some(version)
}

/// Whether `version` is numbers joined by dots (at least two), optionally
/// followed by a `-` or `+` and dot-joined letters and digits: `0.13.4`,
/// `0.14.0-rc.1`, not `1garbage`, `0..13.4` or `0.13.4+`.
fn well_formed(version: &str) -> bool {
    let (release, suffix) = match version.find(['-', '+']) {
        Some(at) => (&version[..at], Some(&version[at + 1..])),
        None => (version, None),
    };
    let parts = |text: &str, part: fn(&str) -> bool| text.split('.').all(part);
    let numbers = release.split('.').count() >= 2
        && parts(release, |part| {
            !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())
        });
    let labelled = suffix.is_none_or(|suffix| {
        parts(suffix, |part| {
            !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_alphanumeric())
        })
    });
    numbers && labelled
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pin_has_one_build_for_each_machine_hedos_installs_it_on() {
        for (os, arch) in [
            (Os::Macos, "aarch64"),
            (Os::Macos, "x86_64"),
            (Os::Linux, "x86_64"),
            (Os::Linux, "aarch64"),
        ] {
            let matching = PINNED
                .assets
                .iter()
                .filter(|asset| asset.os == os && asset.arch == arch)
                .count();
            assert_eq!(matching, 1, "{os:?} {arch}");
            let asset = PINNED.asset(os, arch).expect("pinned");
            assert_eq!(asset.sha256.len(), 64, "{}", asset.file);
            assert!(
                asset
                    .sha256
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            );
            assert!(asset.bytes > 0);
        }
        assert_eq!(PINNED.assets.len(), 4);
        assert_eq!(PINNED.asset(Os::Other, "x86_64"), None);
        assert_eq!(PINNED.asset(Os::Linux, "riscv64"), None);
    }

    #[test]
    fn each_machine_gets_its_own_archive_from_the_release_tag() {
        let base = "https://github.com/astral-sh/uv/releases/download/0.13.4";
        for (os, arch, file) in [
            (Os::Macos, "aarch64", "uv-aarch64-apple-darwin.tar.gz"),
            (Os::Macos, "x86_64", "uv-x86_64-apple-darwin.tar.gz"),
            // The static musl builds, which start on any glibc.
            (Os::Linux, "x86_64", "uv-x86_64-unknown-linux-musl.tar.gz"),
            (Os::Linux, "aarch64", "uv-aarch64-unknown-linux-musl.tar.gz"),
        ] {
            let asset = PINNED.asset(os, arch).expect("pinned");
            assert_eq!(asset.file, file, "{os:?} {arch}");
            assert_eq!(PINNED.url(asset.file), format!("{base}/{file}"));
        }
    }

    #[test]
    fn the_version_reads_from_both_shapes_uv_prints_and_nothing_else() {
        assert_eq!(
            parse_version("uv 0.13.4 (7456b60bc 2026-10-20 aarch64-apple-darwin)\n"),
            Some("0.13.4")
        );
        assert_eq!(
            parse_version("uv 0.13.4 (aarch64-unknown-linux-musl)"),
            Some("0.13.4")
        );
        assert_eq!(parse_version("uv 0.14.0-rc.1"), Some("0.14.0-rc.1"));
        assert_eq!(parse_version("uvx 0.13.4"), None);
        assert_eq!(parse_version("uv"), None);
        assert_eq!(parse_version("uv version"), None);
        assert_eq!(parse_version("uv 0.13.4;rm"), None);
        assert_eq!(parse_version(""), None);
        assert_eq!(parse_version("error: unknown\nuv 0.13.4"), None);
        for malformed in [
            "uv 1garbage",
            "uv 0..13.4",
            "uv 0.13.4+",
            "uv 0.13.4-",
            "uv 13",
            "uv .13.4",
            "uv 0.13.4-rc..1",
            "uv 0.13.x",
        ] {
            assert_eq!(parse_version(malformed), None, "{malformed}");
        }
    }
}
