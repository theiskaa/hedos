//! Upgrading hedos itself: release versions, how the running binary was
//! installed, the remembered result of the last look for a newer release, and
//! checking a downloaded release archive against its published checksum.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::persistence::{self, StoreError};

/// How long a look for a newer release stays fresh before the next one.
pub const CHECK_INTERVAL_MILLIS: i64 = 24 * 60 * 60 * 1000;

/// The file, in the data dir, that remembers the last look for a newer release.
pub const CHECK_FILE: &str = "upgrade-check.json";

/// The lock, in the data dir, held while a process claims the next look.
const CLAIM_LOCK: &str = "upgrade-check.lock";

/// A release version: `major.minor.patch`, with an optional pre-release after
/// a `-`. A leading `v`, as on the release tags, is accepted. Build metadata
/// after a `+` is checked and dropped, since it never orders one release
/// before another.
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
        let text = match text.split_once('+') {
            Some((core, build)) if build.split('.').all(well_formed) => core,
            Some(_) => return None,
            None => text,
        };
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
                let valid = identifiers.iter().all(|identifier| {
                    well_formed(identifier)
                        && !(is_numeric(identifier)
                            && identifier.len() > 1
                            && identifier.starts_with('0'))
                });
                if !valid {
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
    if !is_numeric(text) || (text.len() > 1 && text.starts_with('0')) {
        return None;
    }
    text.parse().ok()
}

fn is_numeric(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit())
}

/// A pre-release or build identifier: one or more ASCII letters, digits or
/// hyphens.
fn well_formed(identifier: &str) -> bool {
    !identifier.is_empty()
        && identifier
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
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
        // Numeric identifiers carry no leading zeros, so the longer is the
        // larger, and two of one length order as text, at any size.
        let order = match (is_numeric(a), is_numeric(b)) {
            (true, true) => a.len().cmp(&b.len()).then_with(|| a.cmp(b)),
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            (false, false) => a.cmp(b),
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Channel {
    /// Installed by Homebrew, from the `theiskaa/tap` formula.
    Homebrew {
        /// The Homebrew prefix, whose `bin/hedos` links to the installed keg.
        prefix: PathBuf,
    },
    /// Installed by `cargo install`.
    Cargo {
        /// The install root, whose `bin/` holds the binary.
        root: PathBuf,
    },
    /// Run from a cargo build directory.
    Source,
    /// A binary placed by hand or by the install script, replaced in place.
    Standalone,
}

impl Channel {
    /// The channel of the binary at `exe`, its path with links resolved.
    pub fn detect(exe: &Path) -> Self {
        // Homebrew links its binaries into `<prefix>/bin`, but the link
        // resolves into the keg, `<prefix>/Cellar/hedos/<version>/bin`, on
        // every prefix (`/opt/homebrew`, `/usr/local`, Linuxbrew).
        if let Some(prefix) = homebrew_prefix(exe) {
            return Self::Homebrew { prefix };
        }
        if let Some(root) = cargo_root(exe) {
            return Self::Cargo { root };
        }
        if in_build_dir(exe) {
            return Self::Source;
        }
        Self::Standalone
    }

    /// The channel's name, as `--json` reports it.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Homebrew { .. } => "homebrew",
            Self::Cargo { .. } => "cargo",
            Self::Source => "source",
            Self::Standalone => "standalone",
        }
    }
}

/// The prefix above the `Cellar` that holds `exe`, if one does.
fn homebrew_prefix(exe: &Path) -> Option<PathBuf> {
    exe.ancestors()
        .find(|dir| dir.file_name().is_some_and(|name| name == "Cellar"))
        .and_then(Path::parent)
        .map(Path::to_path_buf)
}

/// The cargo install root holding `exe`: the directory above its `bin/` whose
/// install records (`.crates2.json`, or the older `.crates.toml`) list the
/// `hedos` binary. Reading the records rather than comparing with `CARGO_HOME`
/// covers a `--root` install and a cargo home reached through a link.
fn cargo_root(exe: &Path) -> Option<PathBuf> {
    let bin = exe.parent()?;
    if bin.file_name()? != "bin" {
        return None;
    }
    let root = bin.parent()?;
    let listed = match crate::fs::read_regular(&root.join(".crates2.json")) {
        Ok(bytes) => serde_json::from_slice::<CargoInstalls>(&bytes)
            .is_ok_and(|records| records.lists_hedos()),
        Err(_) => crate::fs::read_regular(&root.join(".crates.toml"))
            .is_ok_and(|bytes| String::from_utf8_lossy(&bytes).contains("\"hedos ")),
    };
    listed.then(|| root.to_path_buf())
}

/// The part of cargo's `.crates2.json` that says which binaries came from
/// which package.
#[derive(Deserialize)]
struct CargoInstalls {
    installs: HashMap<String, CargoInstall>,
}

#[derive(Deserialize)]
struct CargoInstall {
    #[serde(default)]
    bins: Vec<String>,
}

impl CargoInstalls {
    fn lists_hedos(&self) -> bool {
        self.installs.iter().any(|(package, install)| {
            package.starts_with("hedos ") && install.bins.iter().any(|bin| bin == "hedos")
        })
    }
}

/// Whether `exe` sits in a cargo profile directory, which cargo fills with
/// its `.fingerprint/` and `deps/` beside the binaries, wherever the target
/// dir is. A `target/<profile>/` or `target/<triple>/<profile>/` path counts
/// even with those gone.
fn in_build_dir(exe: &Path) -> bool {
    let Some(dir) = exe.parent() else {
        return false;
    };
    if dir.join(".fingerprint").is_dir() && dir.join("deps").is_dir() {
        return true;
    }
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
    } else if cfg!(all(
        target_os = "linux",
        target_env = "gnu",
        target_arch = "aarch64"
    )) {
        Some("aarch64-unknown-linux-gnu")
    } else if cfg!(all(
        target_os = "linux",
        target_env = "gnu",
        target_arch = "x86_64"
    )) {
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

    /// Claim the next look at `now` for this process, when the last one in
    /// `data_dir` is due: the claim is remembered as the look's start, so the
    /// processes that read it after do not start looks of their own. Only one
    /// process at a time may claim; one that finds the claim being made by
    /// another stands aside. `false` when there is nothing to claim, someone
    /// else is claiming, or the claim cannot be written.
    pub fn claim(data_dir: &Path, now: i64) -> bool {
        if std::fs::create_dir_all(data_dir).is_err() {
            return false;
        }
        let Ok(lock) = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(data_dir.join(CLAIM_LOCK))
        else {
            return false;
        };
        if fs2::FileExt::try_lock_exclusive(&lock).is_err() {
            return false;
        }
        let mut check = Self::load(data_dir).unwrap_or_default();
        if !check.is_due(now) {
            return false;
        }
        check.checked_at = now;
        check.save(data_dir).is_ok()
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
            "1.0.0-01",
            "1.0.0+",
            "1.0.0+a..b",
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
            "1.0.0-rc.99999999999999999999",
            "1.0.0-rc.2a",
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

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hedos-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn channels_are_told_apart_by_where_the_binary_lives() {
        let detect = |path: &Path| Channel::detect(path);
        assert_eq!(
            detect(Path::new("/opt/homebrew/Cellar/hedos/1.5.2/bin/hedos")),
            Channel::Homebrew {
                prefix: PathBuf::from("/opt/homebrew")
            }
        );
        assert_eq!(
            detect(Path::new(
                "/home/linuxbrew/.linuxbrew/Cellar/hedos/1.5.2/bin/hedos"
            )),
            Channel::Homebrew {
                prefix: PathBuf::from("/home/linuxbrew/.linuxbrew")
            }
        );
        assert_eq!(
            detect(Path::new("/Users/me/dev/hedos/target/release/hedos")),
            Channel::Source
        );
        assert_eq!(
            detect(Path::new(
                "/Users/me/dev/hedos/target/aarch64-apple-darwin/dist/hedos"
            )),
            Channel::Source
        );
        assert_eq!(
            detect(Path::new("/Users/me/.local/bin/hedos")),
            Channel::Standalone
        );
    }

    #[test]
    fn a_cargo_install_is_known_by_its_records_at_any_root() {
        let root = scratch("cargo-root");
        std::fs::create_dir_all(root.join("bin")).unwrap();
        let exe = root.join("bin/hedos");
        assert_eq!(Channel::detect(&exe), Channel::Standalone);

        std::fs::write(
            root.join(".crates2.json"),
            r#"{"installs":{"hedos 1.5.2 (registry+https://github.com/rust-lang/crates.io-index)":{"bins":["hedos"]}}}"#,
        )
        .unwrap();
        assert_eq!(Channel::detect(&exe), Channel::Cargo { root: root.clone() });

        std::fs::write(
            root.join(".crates2.json"),
            r#"{"installs":{"exa 0.10.1 (registry+https://github.com/rust-lang/crates.io-index)":{"bins":["exa"]}}}"#,
        )
        .unwrap();
        assert_eq!(Channel::detect(&exe), Channel::Standalone);

        std::fs::remove_file(root.join(".crates2.json")).unwrap();
        std::fs::write(
            root.join(".crates.toml"),
            "[v1]\n\"hedos 1.5.2 (registry+https://github.com/rust-lang/crates.io-index)\" = [\"hedos\"]\n",
        )
        .unwrap();
        assert_eq!(Channel::detect(&exe), Channel::Cargo { root: root.clone() });
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_build_in_any_target_dir_is_a_source_build() {
        let profile = scratch("build-dir").join("release");
        std::fs::create_dir_all(profile.join(".fingerprint")).unwrap();
        std::fs::create_dir_all(profile.join("deps")).unwrap();
        assert_eq!(Channel::detect(&profile.join("hedos")), Channel::Source);
        std::fs::remove_dir_all(profile.parent().unwrap()).unwrap();
    }

    #[test]
    fn only_one_process_claims_a_due_look() {
        let dir = scratch("claim");
        let now = 20 * CHECK_INTERVAL_MILLIS;
        assert!(UpgradeCheck::claim(&dir, now));
        assert!(
            !UpgradeCheck::claim(&dir, now + 1),
            "a claimed look is not due again"
        );
        let held = std::fs::OpenOptions::new()
            .write(true)
            .open(dir.join(CLAIM_LOCK))
            .unwrap();
        fs2::FileExt::lock_exclusive(&held).unwrap();
        let later = now + CHECK_INTERVAL_MILLIS;
        assert!(
            !UpgradeCheck::claim(&dir, later),
            "a claim being made elsewhere is stood aside for"
        );
        fs2::FileExt::unlock(&held).unwrap();
        assert!(UpgradeCheck::claim(&dir, later));
        assert_eq!(UpgradeCheck::load(&dir).unwrap().checked_at, later);
        std::fs::remove_dir_all(&dir).unwrap();
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
