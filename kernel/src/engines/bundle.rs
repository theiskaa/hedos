//! Cutting a llama.cpp release archive down to what `llama-server` needs, for
//! the build hedos carries inside itself: the server and the libraries beside
//! it, without the twenty other tools a release ships. Pure work over bytes;
//! the build script runs it and compresses what it makes, and the runtime
//! unpacks that the way it unpacks a release.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use flate2::read::GzDecoder;

use crate::engines::llama_cpp::{Release, Variant, bundled_variant};
use crate::machine::Os;

/// The program the cut-down archive is for.
const SERVER: &str = "llama-server";

/// Why a release archive could not be cut down.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BundleError {
    /// The archive could not be read as a gzipped tar.
    #[error("the archive cannot be read: {0}")]
    Unreadable(String),
    /// An entry is one hedos does not unpack.
    #[error("the archive holds {entry}, which hedos does not unpack: {reason}")]
    Refused {
        /// The entry.
        entry: String,
        /// Why it was refused.
        reason: String,
    },
    /// The archive holds no `llama-server`.
    #[error("the archive holds no llama-server")]
    NoServer,
    /// The archive holds no `LICENSE`, which must travel with the binaries.
    #[error(
        "the archive holds no LICENSE, which llama.cpp's MIT license requires beside its binaries"
    )]
    NoLicense,
}

/// Why an archive cannot be the build a binary for some target carries.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CarryError {
    /// hedos carries no llama.cpp build for the target.
    #[error("hedos carries no llama.cpp build for {os} {arch}")]
    NoBuild {
        /// The target's operating system.
        os: String,
        /// The target's architecture.
        arch: String,
    },
    /// The release pins no archive of the build hedos carries there.
    #[error("llama.cpp {version} pins no {variant} build for {os} {arch}")]
    NotPinned {
        /// The release.
        version: String,
        /// The build hedos carries there.
        variant: String,
        /// The target's operating system.
        os: String,
        /// The target's architecture.
        arch: String,
    },
    /// The archive is not the pinned one.
    #[error("the archive is not {file} as hedos pins it (sha256 {found}, expected {expected})")]
    Digest {
        /// The pinned archive's name.
        file: String,
        /// The archive's digest.
        found: String,
        /// The pinned digest.
        expected: String,
    },
}

/// The build a binary for `os`/`arch` carries, when `archive_sha256` is the
/// digest of the archive it is given: the pinned build of the variant hedos
/// carries there (llama.cpp's own on macOS, hedos's on Linux), and nothing
/// else.
pub fn carried_build(
    release: &Release,
    os: Os,
    arch: &str,
    archive_sha256: &str,
) -> Result<Variant, CarryError> {
    let variant = bundled_variant(os, arch).ok_or_else(|| CarryError::NoBuild {
        os: os.as_str().to_owned(),
        arch: arch.to_owned(),
    })?;
    let asset = release
        .asset(os, arch, variant)
        .ok_or_else(|| CarryError::NotPinned {
            version: release.version.to_owned(),
            variant: variant.as_str().to_owned(),
            os: os.as_str().to_owned(),
            arch: arch.to_owned(),
        })?;
    if archive_sha256 != asset.sha256 {
        return Err(CarryError::Digest {
            file: asset.file.to_owned(),
            found: archive_sha256.to_owned(),
            expected: asset.sha256.to_owned(),
        });
    }
    Ok(variant)
}

/// What one kept entry is.
enum Kept {
    /// A file's bytes and its mode.
    File(Vec<u8>, u32),
    /// A link to a file beside it.
    Link(String),
}

/// `archive`, a llama.cpp release's gzipped tar, cut down to `llama-server`,
/// the `lib*.dylib` and `lib*.so*` files and links beside it in its top-level
/// directory, and its `LICENSE`, written back as a plain tar under that
/// directory. The same input always gives the same bytes: entries sorted by
/// name, no timestamps, no owners.
///
/// Every entry is checked, kept or not: an absolute path, a `..`, or a second
/// top-level directory refuses the archive. A file at the archive's root and
/// a global metadata header are passed over. A kept entry must be a plain file
/// or a link to another kept file beside it; a hard link or anything else
/// refuses the archive too.
pub fn server_files(archive: &[u8]) -> Result<Vec<u8>, BundleError> {
    let unreadable = |error: std::io::Error| BundleError::Unreadable(error.to_string());
    let mut entries = tar::Archive::new(GzDecoder::new(archive));
    let mut top: Option<String> = None;
    let mut kept: BTreeMap<String, Kept> = BTreeMap::new();
    for entry in entries.entries().map_err(unreadable)? {
        let mut entry = entry.map_err(unreadable)?;
        // A global metadata header (a `pax_global_header` some tools write)
        // names no file.
        if matches!(
            entry.header().entry_type(),
            tar::EntryType::XGlobalHeader | tar::EntryType::XHeader
        ) {
            continue;
        }
        let path = entry.path().map_err(unreadable)?.into_owned();
        let shown = path.to_string_lossy().into_owned();
        let refused = |reason: &str| BundleError::Refused {
            entry: shown.clone(),
            reason: reason.to_owned(),
        };
        let parts = normal_parts(&path).ok_or_else(|| refused("its path leaves the archive"))?;
        let Some(first) = parts.first() else {
            continue;
        };
        // A file at the archive's root beside its directory (a readme, a
        // checksum list) is not part of the build.
        if parts.len() == 1 && entry.header().entry_type() != tar::EntryType::Directory {
            continue;
        }
        match &top {
            Some(top) if top != first => {
                return Err(refused("it is outside the archive's directory"));
            }
            Some(_) => {}
            None => top = Some(first.clone()),
        }
        let [_, name] = parts.as_slice() else {
            continue;
        };
        if !wanted(name) {
            continue;
        }
        let kind = entry.header().entry_type();
        if kind.is_file() {
            let mode = entry.header().mode().unwrap_or(0o644) & 0o755;
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).map_err(unreadable)?;
            kept.insert(name.clone(), Kept::File(bytes, mode));
        } else if kind.is_symlink() {
            let target = entry
                .link_name()
                .map_err(unreadable)?
                .ok_or_else(|| refused("the link names nothing"))?;
            let target = match normal_parts(&target).as_deref() {
                Some([only]) => only.clone(),
                _ => return Err(refused("the link leads out of its directory")),
            };
            kept.insert(name.clone(), Kept::Link(target));
        } else {
            return Err(refused("it is not a file or a link"));
        }
    }
    let top = top.ok_or(BundleError::NoServer)?;
    if !matches!(kept.get(SERVER), Some(Kept::File(..))) {
        return Err(BundleError::NoServer);
    }
    for (name, entry) in &kept {
        if let Kept::Link(target) = entry
            && !kept.contains_key(target)
        {
            return Err(BundleError::Refused {
                entry: format!("{top}/{name}"),
                reason: format!("it links to {target}, which is not kept"),
            });
        }
    }
    if !matches!(kept.get(LICENSE), Some(Kept::File(..))) {
        return Err(BundleError::NoLicense);
    }
    write(&top, &kept).map_err(unreadable)
}

/// The license llama.cpp is shared under, which goes wherever its binaries do.
const LICENSE: &str = "LICENSE";

/// Whether `name`, a file in the release's directory, is one `llama-server`
/// runs with, or its license. Each tool's code is a `libllama-<tool>-impl`
/// library of its own; the server needs only its own.
fn wanted(name: &str) -> bool {
    let library = name.starts_with("lib") && (name.ends_with(".dylib") || name.contains(".so"));
    let tool = name
        .strip_prefix("libllama-")
        .and_then(|rest| rest.split_once("-impl."))
        .map(|(tool, _)| tool);
    name == SERVER || name == LICENSE || (library && tool.is_none_or(|tool| tool == "server"))
}

/// `path`'s components as names, or `None` when it is absolute or climbs.
fn normal_parts(path: &Path) -> Option<Vec<String>> {
    let mut parts = Vec::new();
    for part in path.components() {
        match part {
            Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
            Component::CurDir => {}
            _ => return None,
        }
    }
    Some(parts)
}

/// `kept` as a tar under `top`, with nothing in it that changes from one
/// build to the next.
fn write(top: &str, kept: &BTreeMap<String, Kept>) -> std::io::Result<Vec<u8>> {
    let mut builder = tar::Builder::new(Vec::new());
    builder.mode(tar::HeaderMode::Deterministic);
    let mut directory = tar::Header::new_gnu();
    directory.set_entry_type(tar::EntryType::Directory);
    directory.set_mode(0o755);
    directory.set_size(0);
    builder.append_data(&mut directory, format!("{top}/"), std::io::empty())?;
    for (name, entry) in kept {
        let path = PathBuf::from(top).join(name);
        let mut header = tar::Header::new_gnu();
        match entry {
            Kept::File(bytes, mode) => {
                header.set_entry_type(tar::EntryType::Regular);
                header.set_mode(*mode);
                header.set_size(bytes.len() as u64);
                builder.append_data(&mut header, &path, bytes.as_slice())?;
            }
            Kept::Link(target) => {
                header.set_entry_type(tar::EntryType::Symlink);
                header.set_mode(0o777);
                header.set_size(0);
                builder.append_link(&mut header, &path, target)?;
            }
        }
    }
    builder.into_inner()
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::GzEncoder;

    /// One tar entry: a path written as is (so `..` and absolute paths can be
    /// tested), and a file's bytes, a directory, a link's target, or a hard
    /// link's.
    enum Item<'a> {
        File(&'a str, &'a [u8]),
        Dir(&'a str),
        Link(&'a str, &'a str),
        Hard(&'a str, &'a str),
    }

    fn archive(items: &[Item]) -> Vec<u8> {
        let mut builder = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::fast()));
        for item in items {
            let mut header = tar::Header::new_old();
            let (path, data, kind): (&str, &[u8], tar::EntryType) = match item {
                Item::File(path, data) => (path, data, tar::EntryType::Regular),
                Item::Dir(path) => (path, &[], tar::EntryType::Directory),
                Item::Link(path, target) => {
                    header.set_link_name(target).unwrap();
                    (path, &[], tar::EntryType::Symlink)
                }
                Item::Hard(path, target) => {
                    header.set_link_name(target).unwrap();
                    (path, &[], tar::EntryType::Link)
                }
            };
            header.as_old_mut().name[..path.len()].copy_from_slice(path.as_bytes());
            header.set_entry_type(kind);
            header.set_mode(0o4755);
            header.set_size(data.len() as u64);
            header.set_cksum();
            builder.append(&header, data).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    /// The names, modes and link targets in a tar.
    fn listing(bytes: &[u8]) -> Vec<(String, u32, Option<String>)> {
        let mut entries = tar::Archive::new(bytes);
        entries
            .entries()
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (
                    entry.path().unwrap().to_string_lossy().into_owned(),
                    entry.header().mode().unwrap(),
                    entry
                        .link_name()
                        .unwrap()
                        .map(|target| target.to_string_lossy().into_owned()),
                )
            })
            .collect()
    }

    fn release() -> Vec<Item<'static>> {
        vec![
            Item::Dir("llama-b11669/"),
            Item::File("llama-b11669/llama-cli", b"cli"),
            Item::File("llama-b11669/libllama.0.7.0.dylib", b"lib"),
            Item::Link("llama-b11669/libllama.0.dylib", "libllama.0.7.0.dylib"),
            Item::File("llama-b11669/llama-server", b"server"),
            Item::File("llama-b11669/libggml-cpu-zen4.so", b"cpu"),
            Item::File("llama-b11669/LICENSE", b"mit"),
            Item::File("llama-b11669/NOTICE.md", b"notes"),
            Item::File("llama-b11669/llama-bench", b"bench"),
            Item::File("llama-b11669/libllama-server-impl.dylib", b"server code"),
            Item::File("llama-b11669/libllama-cli-impl.dylib", b"cli code"),
        ]
    }

    #[test]
    fn a_release_keeps_the_server_and_its_libraries_and_drops_the_tools() {
        let cut = server_files(&archive(&release())).unwrap();
        assert_eq!(
            listing(&cut),
            [
                ("llama-b11669/".to_owned(), 0o755, None),
                ("llama-b11669/LICENSE".to_owned(), 0o755, None),
                ("llama-b11669/libggml-cpu-zen4.so".to_owned(), 0o755, None),
                (
                    "llama-b11669/libllama-server-impl.dylib".to_owned(),
                    0o755,
                    None
                ),
                ("llama-b11669/libllama.0.7.0.dylib".to_owned(), 0o755, None),
                (
                    "llama-b11669/libllama.0.dylib".to_owned(),
                    0o777,
                    Some("libllama.0.7.0.dylib".to_owned())
                ),
                ("llama-b11669/llama-server".to_owned(), 0o755, None),
            ]
        );
    }

    #[test]
    fn the_same_release_gives_the_same_bytes_whatever_its_order() {
        let mut shuffled = release();
        shuffled.reverse();
        let first = server_files(&archive(&release())).unwrap();
        assert_eq!(first, server_files(&archive(&release())).unwrap());
        assert_eq!(first, server_files(&archive(&shuffled)).unwrap());
    }

    #[test]
    fn an_archive_that_reaches_out_or_holds_odd_entries_is_refused() {
        let refused = |items: &[Item]| {
            matches!(
                server_files(&archive(items)),
                Err(BundleError::Refused { .. })
            )
        };
        assert!(refused(&[
            Item::File("llama-b11669/llama-server", b"server"),
            Item::File("llama-b11669/../../escape", b"x"),
        ]));
        assert!(refused(&[Item::File("/etc/llama-server", b"x")]));
        assert!(refused(&[
            Item::File("llama-b11669/llama-server", b"server"),
            Item::File("other/libx.so", b"x"),
        ]));
        assert!(refused(&[
            Item::File("llama-b11669/llama-server", b"server"),
            Item::Link("llama-b11669/libx.dylib", "../../etc/passwd"),
        ]));
        assert!(refused(&[
            Item::File("llama-b11669/llama-server", b"server"),
            Item::Hard("llama-b11669/libx.dylib", "llama-b11669/llama-server"),
        ]));
        assert!(refused(&[
            Item::File("llama-b11669/llama-server", b"server"),
            Item::Link("llama-b11669/libx.dylib", "libgone.dylib"),
        ]));
    }

    #[test]
    fn an_archive_without_its_license_is_refused() {
        assert_eq!(
            server_files(&archive(&[Item::File(
                "llama-b11669/llama-server",
                b"server"
            )])),
            Err(BundleError::NoLicense)
        );
    }

    #[test]
    fn a_file_at_the_root_beside_the_directory_is_passed_over() {
        let mut items = release();
        items.insert(0, Item::File("SHA256SUMS", b"sums"));
        assert_eq!(
            listing(&server_files(&archive(&items)).unwrap()),
            listing(&server_files(&archive(&release())).unwrap())
        );
    }

    #[test]
    fn an_archive_without_a_server_or_unreadable_is_refused() {
        assert_eq!(
            server_files(&archive(&[Item::File("llama-b11669/llama-cli", b"cli")])),
            Err(BundleError::NoServer)
        );
        assert!(matches!(
            server_files(b"not a gzip"),
            Err(BundleError::Unreadable(_))
        ));
    }

    /// A real release archive, named by `HEDOS_LLAMA_CPP_ARCHIVE`, cuts down to
    /// a server that keeps every library it links. Ignored by default, since
    /// it needs the archive on disk.
    #[test]
    #[ignore = "reads a real llama.cpp release archive"]
    fn a_real_release_cuts_down() {
        let path = std::env::var("HEDOS_LLAMA_CPP_ARCHIVE").expect("an archive to read");
        let release = std::fs::read(&path).unwrap();
        let cut = server_files(&release).unwrap();
        let names: Vec<String> = listing(&cut).into_iter().map(|(name, _, _)| name).collect();
        println!(
            "{} -> {} bytes, {} entries: {names:?}",
            release.len(),
            cut.len(),
            names.len()
        );
        assert!(names.iter().any(|name| name.ends_with("/llama-server")));
        assert!(cut.len() < release.len());
    }

    #[test]
    fn a_binary_carries_only_the_pinned_archive_of_its_target() {
        use crate::engines::llama_cpp::{PINNED, Publisher};
        let metal = PINNED
            .asset(Os::Macos, "aarch64", Variant::Metal)
            .expect("pinned");
        assert_eq!(
            carried_build(&PINNED, Os::Macos, "aarch64", metal.sha256),
            Ok(Variant::Metal)
        );
        assert!(matches!(
            carried_build(&PINNED, Os::Macos, "aarch64", &"0".repeat(64)),
            Err(CarryError::Digest { .. })
        ));
        assert!(matches!(
            carried_build(&PINNED, Os::Other, "riscv64", metal.sha256),
            Err(CarryError::NoBuild { .. })
        ));
        let linux = PINNED
            .asset(Os::Linux, "aarch64", Variant::Cpu)
            .expect("pinned");
        assert_eq!(linux.publisher, Publisher::Hedos);
        assert_eq!(
            carried_build(&PINNED, Os::Linux, "aarch64", linux.sha256),
            Ok(Variant::Cpu)
        );
        // llama.cpp's own ubuntu-arm64 archive, which needs glibc 2.38, is
        // not the build hedos carries.
        assert!(matches!(
            carried_build(
                &PINNED,
                Os::Linux,
                "aarch64",
                "9ac6fda2058d35677608a06f751a50122ddfa4ce0ddb7f084466b8e79b6d4a99"
            ),
            Err(CarryError::Digest { .. })
        ));
        // Another arch's archive is not this one's.
        assert!(matches!(
            carried_build(&PINNED, Os::Linux, "x86_64", linux.sha256),
            Err(CarryError::Digest { .. })
        ));
    }
}
