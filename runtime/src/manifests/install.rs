//! Commands a manifest downloads instead of finding on the machine. A manifest's
//! `[install]` pins, per platform, a release archive and its sha256; approving
//! the runtime fetches this platform's archive, checks it against the pin, and
//! unpacks the one binary it names into the data directory, where the command
//! runs it as `{bin}`. The pin is part of the manifest, so the approval, which
//! covers the manifest's files, covers exactly the binary that runs.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use flate2::read::GzDecoder;
use kernel::manifests::{InstallTarget, RuntimeManifest};
use sha2::{Digest, Sha256};

use super::slug;

/// The largest archive downloaded: a release of one command is far smaller.
const ARCHIVE_CAP: u64 = 256 * 1024 * 1024;
/// How long connecting to the host may take, and the whole download.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(600);

/// Why a manifest's command could not be installed or found.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InstallError {
    /// The manifest pins no build for this machine.
    #[error("{id} has no build for {target}")]
    NoBuild { id: String, target: String },
    /// The archive could not be fetched.
    #[error("downloading {url}: {reason}")]
    Download { url: String, reason: String },
    /// The archive is not the one the manifest pins.
    #[error("{url} does not match the manifest's sha256 (expected {expected}, got {found})")]
    Checksum {
        url: String,
        expected: String,
        found: String,
    },
    /// The archive does not hold the binary the manifest names.
    #[error("{url} has no {binary}")]
    NoBinary { url: String, binary: String },
    /// Writing the binary into place failed.
    #[error("installing {path}: {reason}")]
    Write { path: String, reason: String },
}

/// The Rust target triple of this machine, as release archives name builds.
pub fn host_target() -> String {
    let os = match std::env::consts::OS {
        "macos" => "apple-darwin",
        "linux" => "unknown-linux-gnu",
        other => other,
    };
    format!("{}-{os}", std::env::consts::ARCH)
}

/// The build `manifest` pins for this machine.
pub fn host_build(manifest: &RuntimeManifest) -> Result<&InstallTarget, InstallError> {
    let target = host_target();
    manifest
        .install
        .as_ref()
        .and_then(|install| install.targets.get(&target))
        .ok_or(InstallError::NoBuild {
            id: manifest.id.clone(),
            target,
        })
}

/// Where `manifest`'s binary for `build` lives under `bin_root`: a directory
/// per runtime and per pinned archive, so a new pin never runs an old binary.
fn location(manifest: &RuntimeManifest, build: &InstallTarget, bin_root: &Path) -> PathBuf {
    let name = Path::new(&build.binary)
        .file_name()
        .map_or_else(|| slug(&manifest.id).into(), ToOwned::to_owned);
    bin_root
        .join(slug(&manifest.id))
        .join(&build.sha256[..16])
        .join(name)
}

/// The installed binary of `manifest` for this machine, if it is in place.
pub fn installed(manifest: &RuntimeManifest, bin_root: &Path) -> Option<PathBuf> {
    let build = host_build(manifest).ok()?;
    let path = location(manifest, build, bin_root);
    path.is_file().then_some(path)
}

/// Install `manifest`'s binary for this machine under `bin_root`, unless it
/// already is: download the pinned archive, check its sha256, and unpack the
/// binary. Returns where it is.
pub async fn install(manifest: &RuntimeManifest, bin_root: &Path) -> Result<PathBuf, InstallError> {
    if let Some(path) = installed(manifest, bin_root) {
        return Ok(path);
    }
    let build = host_build(manifest)?.clone();
    let archive = download(&build.url).await?;
    let shown = bin_root.to_string_lossy().into_owned();
    let manifest = manifest.clone();
    let bin_root = bin_root.to_owned();
    tokio::task::spawn_blocking(move || unpack(&manifest, &build, &archive, &bin_root))
        .await
        .map_err(|error| InstallError::Write {
            path: shown,
            reason: error.to_string(),
        })?
}

/// The bytes at `url`, refused past [`ARCHIVE_CAP`].
async fn download(url: &str) -> Result<Vec<u8>, InstallError> {
    let failed = |reason: String| InstallError::Download {
        url: url.to_owned(),
        reason,
    };
    let client = reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(DOWNLOAD_TIMEOUT)
        .build()
        .map_err(|error| failed(error.to_string()))?;
    let mut response = client
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|error| failed(error.to_string()))?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| failed(error.to_string()))?
    {
        bytes.extend_from_slice(&chunk);
        if bytes.len() as u64 > ARCHIVE_CAP {
            return Err(failed(format!(
                "larger than {} MiB",
                ARCHIVE_CAP / 1024 / 1024
            )));
        }
    }
    Ok(bytes)
}

/// Check `archive` against `build`'s pin and unpack its binary into place for
/// `manifest` under `bin_root`, executable. The binary is written beside its
/// final name and renamed, so a half-written one is never run.
pub fn unpack(
    manifest: &RuntimeManifest,
    build: &InstallTarget,
    archive: &[u8],
    bin_root: &Path,
) -> Result<PathBuf, InstallError> {
    let found = hex::encode(Sha256::digest(archive));
    if found != build.sha256 {
        return Err(InstallError::Checksum {
            url: build.url.clone(),
            expected: build.sha256.clone(),
            found,
        });
    }
    let path = location(manifest, build, bin_root);
    let write_failed = |reason: String| InstallError::Write {
        path: path.to_string_lossy().into_owned(),
        reason,
    };
    let mut entries = tar::Archive::new(GzDecoder::new(archive));
    let mut binary = None;
    for entry in entries
        .entries()
        .map_err(|error| write_failed(error.to_string()))?
    {
        let mut entry = entry.map_err(|error| write_failed(error.to_string()))?;
        let named = entry
            .path()
            .is_ok_and(|inside| inside == Path::new(&build.binary));
        if named {
            let mut bytes = Vec::new();
            entry
                .read_to_end(&mut bytes)
                .map_err(|error| write_failed(error.to_string()))?;
            binary = Some(bytes);
            break;
        }
    }
    let binary = binary.ok_or_else(|| InstallError::NoBinary {
        url: build.url.clone(),
        binary: build.binary.clone(),
    })?;
    let directory = path.parent().unwrap_or(bin_root);
    std::fs::create_dir_all(directory).map_err(|error| write_failed(error.to_string()))?;
    let partial = path.with_extension("partial");
    std::fs::write(&partial, binary).map_err(|error| write_failed(error.to_string()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&partial, std::fs::Permissions::from_mode(0o755))
            .map_err(|error| write_failed(error.to_string()))?;
    }
    std::fs::rename(&partial, &path).map_err(|error| write_failed(error.to_string()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::Write;

    use flate2::Compression;
    use flate2::write::GzEncoder;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "hedos-install-{name}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |elapsed| elapsed.as_nanos())
            ));
            std::fs::create_dir_all(&path).expect("scratch");
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A `.tar.gz` holding `files`.
    fn archive(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut builder = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::fast()));
        for (path, bytes) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, path, *bytes)
                .expect("append");
        }
        let mut gz = builder.into_inner().expect("tar");
        gz.flush().expect("flush");
        gz.finish().expect("gzip")
    }

    fn manifest(archive: &[u8]) -> RuntimeManifest {
        let sha = hex::encode(Sha256::digest(archive));
        RuntimeManifest::parse(
            &format!(
                r#"
id = "cli:tool"
modalities = ["text"]
capabilities = ["extract"]
execution = "sync"
[invoke]
command = "{{bin}} json"
[install]
version = "1.0.0"
[install.targets.{target}]
url = "https://example.invalid/tool.tar.gz"
sha256 = "{sha}"
binary = "tool-1.0.0/tool"
"#,
                target = host_target()
            ),
            None,
        )
        .expect("a manifest")
    }

    #[test]
    fn a_pinned_archive_unpacks_its_binary_executable() {
        let scratch = Scratch::new("unpack");
        let bytes = archive(&[
            ("tool-1.0.0/README", b"read me"),
            ("tool-1.0.0/tool", b"#!/bin/sh\n"),
        ]);
        let manifest = manifest(&bytes);
        assert!(installed(&manifest, &scratch.0).is_none());
        let build = host_build(&manifest).expect("a build for this machine");
        let path = unpack(&manifest, build, &bytes, &scratch.0).expect("unpacked");
        assert_eq!(std::fs::read(&path).expect("binary"), b"#!/bin/sh\n");
        assert_eq!(installed(&manifest, &scratch.0), Some(path.clone()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path)
                .expect("metadata")
                .permissions()
                .mode();
            assert_eq!(mode & 0o111, 0o111, "executable");
        }
    }

    #[test]
    fn an_archive_that_is_not_the_pinned_one_is_refused() {
        let scratch = Scratch::new("checksum");
        let pinned = archive(&[("tool-1.0.0/tool", b"good")]);
        let manifest = manifest(&pinned);
        let swapped = archive(&[("tool-1.0.0/tool", b"evil")]);
        let build = host_build(&manifest).expect("a build");
        let refused = unpack(&manifest, build, &swapped, &scratch.0);
        assert!(
            matches!(refused, Err(InstallError::Checksum { .. })),
            "{refused:?}"
        );
        assert!(
            installed(&manifest, &scratch.0).is_none(),
            "nothing was written"
        );
    }

    #[test]
    fn an_archive_without_the_binary_says_so() {
        let scratch = Scratch::new("missing");
        let bytes = archive(&[("tool-1.0.0/README", b"read me")]);
        let manifest = manifest(&bytes);
        let build = host_build(&manifest).expect("a build");
        let refused = unpack(&manifest, build, &bytes, &scratch.0);
        assert!(
            matches!(refused, Err(InstallError::NoBinary { .. })),
            "{refused:?}"
        );
    }

    #[test]
    fn a_machine_without_a_build_is_named() {
        let bytes = archive(&[("tool-1.0.0/tool", b"x")]);
        let mut manifest = manifest(&bytes);
        if let Some(install) = manifest.install.as_mut() {
            let build = install.targets.values().next().cloned().expect("a build");
            install.targets.clear();
            install
                .targets
                .insert("sparc-sun-solaris".to_owned(), build);
        }
        let missing = host_build(&manifest);
        assert!(
            matches!(&missing, Err(InstallError::NoBuild { target, .. }) if *target == host_target()),
            "{missing:?}"
        );
    }
}
