//! Packs `runtime/runtimes/` into one gzip-compressed archive at build time, so
//! `bundled.rs` can embed a single small blob instead of the raw tree, which is
//! dominated by `requirements.lock` files. A release build also embeds the
//! pinned llama.cpp build its job fetched (`bundle_llama_cpp`), and a macOS
//! build compiles the Apple Intelligence shim (`build_apple_shim`).
//!
//! Archive format: for each file, in deterministic sorted-by-path order,
//! `u32 LE path_byte_len`, the relative path (forward-slash separated, relative
//! to `runtimes/`), `u32 LE content_len`, the content bytes. The concatenation is
//! gzip-compressed as a whole.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use flate2::Compression;
use flate2::write::GzEncoder;

fn main() {
    let manifest_dir =
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set by cargo");
    let runtimes_root = Path::new(&manifest_dir).join("runtimes");
    println!("cargo:rerun-if-changed=runtimes");

    let mut files = BTreeMap::new();
    collect_files(&runtimes_root, &runtimes_root, &mut files);

    let mut payload = Vec::new();
    for (relative_path, contents) in &files {
        let path_bytes = relative_path.as_bytes();
        payload.extend_from_slice(&(path_bytes.len() as u32).to_le_bytes());
        payload.extend_from_slice(path_bytes);
        payload.extend_from_slice(&(contents.len() as u32).to_le_bytes());
        payload.extend_from_slice(contents);
    }

    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(&payload)
        .expect("writing to an in-memory gzip encoder cannot fail");
    let compressed = encoder
        .finish()
        .expect("finishing an in-memory gzip stream cannot fail");

    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR is set by cargo");
    let archive_path = Path::new(&out_dir).join("runtimes.archive.gz");
    std::fs::write(&archive_path, compressed).expect("writing the archive to OUT_DIR cannot fail");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        build_apple_shim(&manifest_dir, &out_dir);
    }

    bundle_llama_cpp(&out_dir);
}

/// Carry the pinned llama.cpp build when `HEDOS_LLAMA_CPP_ARCHIVE` names its
/// archive for the target being built, which a release job fetches before
/// building, checked against the pin: on macOS llama.cpp's own release
/// archive, on Linux hedos's own build of the pinned source. Anything else
/// fails the build. It is cut down to what `llama-server`
/// needs, compressed with xz into `OUT_DIR`, and the crate is built with
/// `hedos_bundled_llama`, the digest of what it embeds, and the variant.
/// Without the variable (a plain `cargo build`, `cargo install`) nothing is
/// embedded and nothing is fetched: the build never reaches the network.
fn bundle_llama_cpp(out_dir: &str) {
    use kernel::engines::bundle::{carried_build, server_files};
    use kernel::engines::llama_cpp::PINNED;
    use kernel::machine::Os;
    use sha2::{Digest, Sha256};

    println!("cargo:rustc-check-cfg=cfg(hedos_bundled_llama)");
    println!("cargo:rerun-if-env-changed=HEDOS_LLAMA_CPP_ARCHIVE");
    let Some(archive) = std::env::var_os("HEDOS_LLAMA_CPP_ARCHIVE").filter(|path| !path.is_empty())
    else {
        return;
    };
    let archive = PathBuf::from(archive);
    println!("cargo:rerun-if-changed={}", archive.display());

    let os = match std::env::var("CARGO_CFG_TARGET_OS").as_deref() {
        Ok("macos") => Os::Macos,
        Ok("linux") => Os::Linux,
        _ => Os::Other,
    };
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let release = std::fs::read(&archive)
        .unwrap_or_else(|error| panic!("reading {}: {error}", archive.display()));
    let variant = carried_build(&PINNED, os, &arch, &hex::encode(Sha256::digest(&release)))
        .unwrap_or_else(|error| panic!("{}: {error}", archive.display()));
    let server = server_files(&release)
        .unwrap_or_else(|error| panic!("cutting {} down: {error}", archive.display()));
    let mut encoder = liblzma::write::XzEncoder::new(Vec::new(), 9);
    encoder
        .write_all(&server)
        .expect("writing to an in-memory xz encoder cannot fail");
    let compressed = encoder
        .finish()
        .expect("finishing an in-memory xz stream cannot fail");
    std::fs::write(Path::new(out_dir).join("llama-server.tar.xz"), &compressed)
        .expect("writing the llama.cpp bundle to OUT_DIR cannot fail");
    println!("cargo:rustc-cfg=hedos_bundled_llama");
    println!(
        "cargo:rustc-env=HEDOS_BUNDLED_LLAMA_SHA256={}",
        hex::encode(Sha256::digest(&compressed))
    );
    println!(
        "cargo:rustc-env=HEDOS_BUNDLED_LLAMA_VARIANT={}",
        variant.as_str()
    );
}

/// Compile the Swift shim over Apple's `FoundationModels` framework
/// (`shim-apple/shim.swift`) into a dylib in `OUT_DIR`, baking its path into
/// the crate as `HEDOS_APPLE_SHIM_BUILT_DYLIB`. Runs on every macOS build —
/// the bridge is a standard part of macOS binaries, never an opt-in. A
/// toolchain that cannot build it (no Xcode, an SDK without
/// FoundationModels) skips with a warning and bakes an empty path, leaving
/// the runtime to report the model unavailable; a compile failure on a
/// capable SDK is a shim bug and fails the build.
fn build_apple_shim(manifest_dir: &str, out_dir: &str) {
    println!("cargo:rerun-if-changed=shim-apple");
    let bake = |path: &str| println!("cargo:rustc-env=HEDOS_APPLE_SHIM_BUILT_DYLIB={path}");
    let Some(sdk_path) = command_stdout("xcrun", &["--sdk", "macosx", "--show-sdk-path"]) else {
        println!("cargo:warning=Apple Intelligence bridge skipped: no usable macOS SDK");
        bake("");
        return;
    };
    let framework =
        Path::new(sdk_path.trim()).join("System/Library/Frameworks/FoundationModels.framework");
    if !framework.exists() {
        println!(
            "cargo:warning=Apple Intelligence bridge skipped: this SDK has no FoundationModels (needs the macOS 26 SDK)"
        );
        bake("");
        return;
    }
    let dylib = Path::new(out_dir).join("libhedos_apple_shim.dylib");
    let source = Path::new(manifest_dir).join("shim-apple/shim.swift");
    // Always optimized, even in debug builds: nobody steps through the shim,
    // and an unoptimized bridge to the model is only slower.
    let status = std::process::Command::new("xcrun")
        .args(["-sdk", "macosx", "swiftc", "-O", "-emit-library", "-o"])
        .arg(&dylib)
        .arg(&source)
        .status();
    match status {
        Ok(status) if status.success() => {}
        Ok(status) => panic!("swiftc failed with {status} compiling shim-apple/shim.swift"),
        Err(error) => panic!("running xcrun swiftc: {error}"),
    }
    bake(&dylib.display().to_string());
}

/// Run `program` with `args`, returning its stdout on success and `None` when
/// it is missing or fails.
fn command_stdout(program: &str, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new(program)
        .args(args)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Recursively collect every file under `dir`, keyed by its path relative to
/// `root` (forward-slash separated), into `out`. A `BTreeMap` keeps insertion
/// order irrelevant: iteration is always sorted by key, so the archive is
/// byte-identical across builds regardless of the OS directory-read order.
/// Whether `path` is something a test or coverage run left in the tree rather
/// than part of a bundle: bytecode caches, coverage profiles, dotfiles. A
/// manifest-driven bundle's consent hash covers every file shipped beside it,
/// so litter in the archive would change that hash with no change to the source.
fn is_build_litter(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    name == "__pycache__"
        || name.starts_with('.')
        || name.ends_with(".pyc")
        || name.ends_with(".profraw")
}

fn collect_files(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|err| panic!("reading {}: {err}", dir.display()))
        .map(|entry| {
            entry
                .unwrap_or_else(|err| panic!("reading entry in {}: {err}", dir.display()))
                .path()
        })
        .collect();
    entries.sort();

    for path in entries {
        if is_build_litter(&path) {
            continue;
        }
        if path.is_dir() {
            collect_files(root, &path, out);
            continue;
        }
        let relative = path.strip_prefix(root).unwrap_or_else(|err| {
            panic!(
                "stripping {} from {}: {err}",
                root.display(),
                path.display()
            )
        });
        let relative = relative
            .to_str()
            .unwrap_or_else(|| panic!("non-utf8 path: {}", relative.display()))
            .replace(std::path::MAIN_SEPARATOR, "/");
        let contents =
            std::fs::read(&path).unwrap_or_else(|err| panic!("reading {}: {err}", path.display()));
        out.insert(relative, contents);
    }
}
