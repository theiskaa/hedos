//! Upgrading hedos itself: release versions, how the running binary was
//! installed, the remembered result of the last look for a newer release, and
//! checking a downloaded release archive against its published checksum.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::persistence::{self, StoreError};

/// How long a look for a newer release stays fresh before the next one.
pub const CHECK_INTERVAL_MILLIS: i64 = 24 * 60 * 60 * 1000;

/// The file, in the data dir, that remembers the last look for a newer release.
pub const CHECK_FILE: &str = "upgrade-check.json";

/// A release version: `major.minor.patch`, with an optional pre-release after
/// a `-`. A leading `v`, as on the release tags, is accepted. Build metadata
/// after a `+` is dropped, since it never orders one release before another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    /// The major number.
    pub major: u64,
    /// The minor number.
    pub minor: u64,
    /// The patch number.
    pub patch: u64,
    /// The dot-separated pre-release identifiers, empty for a release.
    pub pre: Vec<String>,
}

impl Version {
    /// Read `text` as a version, or `None` when it is not one.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let text = text.strip_prefix('v').unwrap_or(text);
        let text = text.split_once('+').map_or(text, |(core, _)| core);
        let (core, pre) = match text.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (text, None),
        };
        let mut numbers = core.split('.').map(number);
        let (major, minor, patch) = (numbers.next()??, numbers.next()??, numbers.next()??);
        if numbers.next().is_some() {
            return None;
        }
        let pre = match pre {
            Some(pre) => {
                let identifiers: Vec<String> = pre.split('.').map(str::to_owned).collect();
                if identifiers.iter().any(|identifier| {
                    identifier.is_empty()
                        || !identifier
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                }) {
                    return None;
                }
                identifiers
            }
            None => Vec::new(),
        };
        Some(Self {
            major,
            minor,
            patch,
            pre,
        })
    }
}

/// A run of ASCII digits as a number; a leading zero is refused, as semver does.
fn number(text: &str) -> Option<u64> {
    if text.is_empty()
        || !text.bytes().all(|byte| byte.is_ascii_digit())
        || (text.len() > 1 && text.starts_with('0'))
    {
        return None;
    }
    text.parse().ok()
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => Ordering::Equal,
                // A pre-release comes before the release it leads up to.
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => compare_pre(&self.pre, &other.pre),
            })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Semver's pre-release order: identifier by identifier, numbers by value and
/// before words, and a shorter list first when one is a prefix of the other.
fn compare_pre(left: &[String], right: &[String]) -> Ordering {
    for (a, b) in left.iter().zip(right) {
        let order = match (number(a), number(b)) {
            (Some(a), Some(b)) => a.cmp(&b),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => a.cmp(b),
        };
        if order != Ordering::Equal {
            return order;
        }
    }
    left.len().cmp(&right.len())
}

impl fmt::Display for Version {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if !self.pre.is_empty() {
            write!(formatter, "-{}", self.pre.join("."))?;
        }
        Ok(())
    }
}

/// How the running binary was installed, which decides how it is upgraded:
/// a package manager's binary is upgraded through that manager, so its record
/// of what is installed stays true.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    /// Installed by Homebrew, from the `theiskaa/tap` formula.
    Homebrew,
    /// Installed by `cargo install`.
    Cargo,
    /// Run from a cargo build directory, a checkout's `target/`.
    Source,
    /// A binary placed by hand or by the install script, replaced in place.
    Standalone,
}

impl Channel {
    /// The channel of the binary at `exe` (its path with links resolved), read
    /// against `env`, the process environment.
    pub fn detect(exe: &Path, env: &HashMap<String, String>) -> Self {
        // Homebrew links its binaries into `<prefix>/bin`, but the link
        // resolves into the keg, so the keg's `Cellar` is what marks it, on
        // every prefix (`/opt/homebrew`, `/usr/local`, Linuxbrew).
        if has_component(exe, "Cellar") {
            return Self::Homebrew;
        }
        let cargo_home = env
            .get("CARGO_HOME")
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
            .or_else(|| env.get("HOME").map(|home| Path::new(home).join(".cargo")));
        if cargo_home.is_some_and(|cargo| exe.starts_with(cargo.join("bin"))) {
            return Self::Cargo;
        }
        if in_build_dir(exe) {
            return Self::Source;
        }
        Self::Standalone
    }

    /// The channel's name, as `--json` reports it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Homebrew => "homebrew",
            Self::Cargo => "cargo",
            Self::Source => "source",
            Self::Standalone => "standalone",
        }
    }
}

fn has_component(path: &Path, name: &str) -> bool {
    path.components()
        .any(|component| matches!(component, Component::Normal(part) if part == name))
}

/// Whether `exe` sits in a cargo profile directory: `target/<profile>/` or,
/// for a cross build, `target/<triple>/<profile>/`.
fn in_build_dir(exe: &Path) -> bool {
    let mut ancestors = exe.ancestors().skip(2).take(2);
    ancestors.any(|dir| dir.file_name().is_some_and(|name| name == "target"))
}

/// The release target triple this binary was built for, as the release
/// archives are named, or `None` on a platform no release is built for.
pub fn target_triple() -> Option<&'static str> {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some("aarch64-apple-darwin")
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        Some("x86_64-apple-darwin")
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        Some("aarch64-unknown-linux-gnu")
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some("x86_64-unknown-linux-gnu")
    } else {
        None
    }
}

/// The release archive for `target`, as each release publishes it.
pub fn archive_name(target: &str) -> String {
    format!("hedos-{target}.tar.xz")
}

/// The last look for a newer release: when it was made and the newest release
/// it found, remembered so a command can say a newer one is out without
/// asking the network itself.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpgradeCheck {
    /// When the look was last started, in milliseconds since the Unix epoch.
    pub checked_at: i64,
    /// The newest release's tag, as of the last look that got an answer.
    pub latest: Option<String>,
}

impl UpgradeCheck {
    /// The remembered look in `data_dir`, or `None` when there is none or it
    /// cannot be read.
    pub fn load(data_dir: &Path) -> Option<Self> {
        persistence::read_json(&data_dir.join(CHECK_FILE))
            .ok()
            .flatten()
    }

    /// Remember this look in `data_dir`.
    pub fn save(&self, data_dir: &Path) -> Result<(), StoreError> {
        persistence::write_json_atomic(&data_dir.join(CHECK_FILE), self)
    }

    /// Whether the last look is old enough, at `now`, for another.
    pub fn is_due(&self, now: i64) -> bool {
        now.saturating_sub(self.checked_at) >= CHECK_INTERVAL_MILLIS || now < self.checked_at
    }

    /// The newest release, when it is newer than `current`.
    pub fn newer_than(&self, current: &Version) -> Option<Version> {
        self.latest
            .as_deref()
            .and_then(Version::parse)
            .filter(|latest| latest > current)
    }
}

/// The SHA-256 that `listing`, a release's `.sha256` file, gives for
/// `file_name`: each line is `<hex> <name>`, the name optionally marked
/// binary with a `*`. A lone digest with no name is taken as the file's.
pub fn expected_sha256(listing: &str, file_name: &str) -> Option<String> {
    listing.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        let digest = fields.next()?;
        let named = fields.next().map(|name| name.trim_start_matches('*'));
        let is_digest = digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit());
        (is_digest && named.is_none_or(|name| name == file_name))
            .then(|| digest.to_ascii_lowercase())
    })
}

/// The SHA-256 of the file at `path`, in lowercase hex.
pub fn sha256_file(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    }

    #[test]
    fn versions_read_with_or_without_the_tag_prefix() {
        assert_eq!(version("v1.5.2"), version("1.5.2"));
        assert_eq!(version("1.6.0-rc.1").pre, vec!["rc", "1"]);
        assert_eq!(version("1.6.0+build.7"), version("1.6.0"));
        assert_eq!(version("2.0.0-beta.2").to_string(), "2.0.0-beta.2");
        for bad in [
            "",
            "1.5",
            "1.5.2.1",
            "01.5.2",
            "1.x.2",
            "1.5.2-",
            "1.5.2-a..b",
            "latest",
        ] {
            assert_eq!(Version::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn versions_order_as_semver_does() {
        let ordered = [
            "1.0.0-alpha",
            "1.0.0-alpha.1",
            "1.0.0-alpha.beta",
            "1.0.0-beta",
            "1.0.0-beta.2",
            "1.0.0-beta.11",
            "1.0.0-rc.1",
            "1.0.0",
            "1.0.1",
            "1.2.0",
            "1.10.0",
            "2.0.0",
        ];
        for pair in ordered.windows(2) {
            assert!(version(pair[0]) < version(pair[1]), "{pair:?}");
        }
    }

    #[test]
    fn channels_are_told_apart_by_where_the_binary_lives() {
        let home = env(&[("HOME", "/Users/me")]);
        let detect =
            |path: &str, env: &HashMap<String, String>| Channel::detect(Path::new(path), env);
        assert_eq!(
            detect("/opt/homebrew/Cellar/hedos/1.5.2/bin/hedos", &home),
            Channel::Homebrew
        );
        assert_eq!(
            detect(
                "/home/linuxbrew/.linuxbrew/Cellar/hedos/1.5.2/bin/hedos",
                &home
            ),
            Channel::Homebrew
        );
        assert_eq!(detect("/Users/me/.cargo/bin/hedos", &home), Channel::Cargo);
        assert_eq!(
            detect(
                "/opt/cargo/bin/hedos",
                &env(&[("HOME", "/Users/me"), ("CARGO_HOME", "/opt/cargo")])
            ),
            Channel::Cargo
        );
        assert_eq!(
            detect("/Users/me/dev/hedos/target/release/hedos", &home),
            Channel::Source
        );
        assert_eq!(
            detect(
                "/Users/me/dev/hedos/target/aarch64-apple-darwin/dist/hedos",
                &home
            ),
            Channel::Source
        );
        assert_eq!(
            detect("/Users/me/.local/bin/hedos", &home),
            Channel::Standalone
        );
        assert_eq!(detect("/usr/local/bin/hedos", &home), Channel::Standalone);
    }

    #[test]
    fn a_look_is_due_a_day_after_the_last_and_reports_only_newer_releases() {
        let check = UpgradeCheck {
            checked_at: 1_000,
            latest: Some("v1.6.0".to_owned()),
        };
        assert!(!check.is_due(1_000 + CHECK_INTERVAL_MILLIS - 1));
        assert!(check.is_due(1_000 + CHECK_INTERVAL_MILLIS));
        assert!(
            check.is_due(999),
            "a clock set back does not hold the next look off"
        );
        assert_eq!(check.newer_than(&version("1.5.2")), Some(version("1.6.0")));
        assert_eq!(check.newer_than(&version("1.6.0")), None);
        assert_eq!(check.newer_than(&version("1.7.0")), None);
        assert_eq!(UpgradeCheck::default().newer_than(&version("0.0.1")), None);
    }

    #[test]
    fn the_look_is_remembered_in_the_data_dir() {
        let dir = std::env::temp_dir().join(format!("hedos-upgrade-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(UpgradeCheck::load(&dir), None);
        let check = UpgradeCheck {
            checked_at: 42,
            latest: Some("v1.6.0".to_owned()),
        };
        check.save(&dir).unwrap();
        assert_eq!(UpgradeCheck::load(&dir), Some(check));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_checksum_listing_is_read_for_the_named_archive() {
        let digest = "562fe8181e10f3f47d638f29ba930d365561db07481d7e31d4c9f6a633e42eaf";
        let archive = "hedos-aarch64-apple-darwin.tar.xz";
        assert_eq!(
            expected_sha256(&format!("{digest} *{archive}\n"), archive).as_deref(),
            Some(digest)
        );
        assert_eq!(
            expected_sha256(&format!("{}  {archive}", digest.to_uppercase()), archive).as_deref(),
            Some(digest)
        );
        assert_eq!(expected_sha256(digest, archive).as_deref(), Some(digest));
        assert_eq!(
            expected_sha256(
                &format!("{digest} *hedos-x86_64-apple-darwin.tar.xz"),
                archive
            ),
            None
        );
        assert_eq!(expected_sha256("not a digest", archive), None);
    }

    #[test]
    fn a_file_hashes_to_its_sha256() {
        let path = std::env::temp_dir().join(format!("hedos-sha-{}", std::process::id()));
        std::fs::write(&path, b"abc").unwrap();
        assert_eq!(
            sha256_file(&path).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        std::fs::remove_file(&path).unwrap();
    }
}
