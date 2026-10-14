//! `hedos upgrade` — bring hedos up to its newest release, the way it was
//! installed: through Homebrew or cargo when one of them put it here, and
//! otherwise by replacing the binary in place with the release's own archive,
//! checked against its published SHA-256 first.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use clap::Args;
use kernel::records::format_bytes;
use kernel::upgrade::{self, Channel, Version};
use tokio::io::AsyncWriteExt;

use crate::error::CliError;
use crate::support::output::Out;
use crate::support::signals;
use crate::support::spinner::Spinner;
use crate::support::upgrade::{self as releases, REPO_URL};

/// The Homebrew formula hedos ships as.
const FORMULA: &str = "theiskaa/tap/hedos";

/// Arguments for `upgrade`.
#[derive(Args)]
pub struct UpgradeArgs {
    /// Only say whether a newer release is out, and how it would be installed.
    #[arg(long)]
    check: bool,
}

/// Run the `upgrade` command.
pub async fn run(args: UpgradeArgs, out: &Out) -> Result<(), CliError> {
    let current = releases::current()
        .ok_or_else(|| CliError::new("this build's version is not a release version"))?;
    let mut spinner = Spinner::start(out);
    spinner.set("looking for the newest release");
    let tag = releases::record_latest()
        .await
        .map_err(|error| CliError::new(format!("could not look up the newest release: {error}")))?;
    spinner.clear();
    let latest = Version::parse(&tag)
        .ok_or_else(|| CliError::new(format!("the newest release, {tag}, has no version")))?;
    let exe = std::env::current_exe().and_then(|exe| exe.canonicalize())?;
    // A debug build is never a release's binary, wherever its target dir is.
    let channel = if cfg!(debug_assertions) {
        Channel::Source
    } else {
        Channel::detect(&exe)
    };
    let report = |upgraded: bool| {
        out.json(&serde_json::json!({
            "current": current.to_string(),
            "latest": latest.to_string(),
            "upgrade_available": latest > current,
            "channel": channel.as_str(),
            "path": exe.display().to_string(),
            "upgraded": upgraded,
        }));
    };

    if latest <= current {
        out.line(&if latest == current {
            format!("hedos {current} is up to date")
        } else {
            format!("hedos {current} is ahead of the newest release, {latest}")
        });
        report(false);
        return Ok(());
    }
    if args.check {
        out.line(releases::available(&latest, &current).trim_start());
        out.line(&format!("  {}", how(&channel, &latest, &exe)));
        report(false);
        return Ok(());
    }

    match &channel {
        Channel::Homebrew { prefix } => {
            delegate(out, "brew", &["upgrade", FORMULA]).await?;
            // brew exits 0 when it finds nothing newer, as it does while the
            // formula has not caught up with the release, so the result is
            // read from the binary its link now points at.
            let installed = prefix.join("bin").join("hedos");
            reports(&installed, &latest).await.map_err(|reported| {
                CliError::new(format!(
                    "brew finished, but {} still reports {reported}: the Homebrew formula for {latest} may not be published yet. Run `brew update` and try again in a few minutes",
                    installed.display()
                ))
            })?;
        }
        Channel::Cargo { root } => {
            let version = latest.to_string();
            let root_arg = root.display().to_string();
            delegate(
                out,
                "cargo",
                &[
                    "install",
                    "hedos",
                    "--version",
                    &version,
                    "--root",
                    &root_arg,
                ],
            )
            .await?;
            let installed = root.join("bin").join("hedos");
            reports(&installed, &latest).await.map_err(|reported| {
                CliError::new(format!(
                    "cargo finished, but {} still reports {reported}",
                    installed.display()
                ))
            })?;
        }
        Channel::Source => {
            return Err(CliError::new(format!(
                "this hedos was built from source ({}); pull the checkout and build it again to get {latest}",
                exe.display()
            )));
        }
        Channel::Standalone => {
            // Ctrl-C is caught for the replacement, so it ends through the
            // scratch dir's cleanup rather than killing the process outright.
            // The copy over the binary is not awaited, so it runs to its end
            // once begun, and a replacement that finishes as Ctrl-C lands is
            // reported as done.
            tokio::select! {
                biased;
                result = replace(out, &exe, &tag, &latest) => result?,
                () = signals::wait_for_ctrl_c() => {
                    return Err(CliError::with_code("upgrade cancelled; nothing was installed", 130));
                }
            }
        }
    }
    out.line(&format!("hedos upgraded from {current} to {latest}"));
    report(true);
    Ok(())
}

/// How `hedos upgrade` would install `latest` on `channel`.
fn how(channel: &Channel, latest: &Version, exe: &Path) -> String {
    match channel {
        Channel::Homebrew { .. } => {
            format!("installed with Homebrew: `hedos upgrade` runs `brew upgrade {FORMULA}`")
        }
        Channel::Cargo { root } => format!(
            "installed with cargo: `hedos upgrade` runs `cargo install hedos --version {latest} --root {}`",
            root.display()
        ),
        Channel::Source => format!(
            "built from source ({}): pull the checkout and build it again",
            exe.display()
        ),
        Channel::Standalone => format!(
            "`hedos upgrade` replaces {} with the release's binary",
            exe.display()
        ),
    }
}

/// Upgrade through the package manager that installed hedos, its output shown
/// as it runs (on stderr under `--json`, so stdout stays one document). The
/// manager is left to finish if hedos is stopped while it runs: one cut off
/// mid-install is worse than one that completes. Ctrl-C reaches it from the
/// terminal directly.
async fn delegate(out: &Out, program: &str, args: &[&str]) -> Result<(), CliError> {
    let shown = format!("{program} {}", args.join(" "));
    out.err(&format!("running `{shown}`"));
    let mut command = tokio::process::Command::new(program);
    command.args(args);
    if out.is_json() {
        command.stdout(stderr_handle()?);
    }
    let status = command.status().await.map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            CliError::new(format!(
                "`{program}` is not on the PATH, so `{shown}` cannot run"
            ))
        } else {
            CliError::new(format!("could not run `{shown}`: {error}"))
        }
    })?;
    if status.success() {
        Ok(())
    } else {
        Err(CliError::with_code(
            format!("`{shown}` failed ({status})"),
            status.code().unwrap_or(1),
        ))
    }
}

/// This process's stderr, as a child's stdout.
fn stderr_handle() -> Result<Stdio, CliError> {
    #[cfg(unix)]
    {
        use std::os::fd::AsFd;
        let fd = std::io::stderr().as_fd().try_clone_to_owned()?;
        Ok(Stdio::from(fd))
    }
    #[cfg(not(unix))]
    {
        Ok(Stdio::inherit())
    }
}

/// Replace the binary at `exe` with the one in release `tag`'s archive: fetched
/// into a scratch dir, checked against the release's SHA-256, unpacked with
/// `tar` (as the install script does), confirmed to be `latest` by running it,
/// then moved over `exe` in one rename, so the binary at `exe` is never half
/// written.
async fn replace(out: &Out, exe: &Path, tag: &str, latest: &Version) -> Result<(), CliError> {
    let target = upgrade::target_triple()
        .ok_or_else(|| CliError::new("no hedos release is built for this platform"))?;
    let archive = upgrade::archive_name(target);
    let base = format!("{REPO_URL}/releases/download/{tag}");
    let scratch = Scratch::create()?;
    let client = releases::client(true).map_err(CliError::new)?;
    let mut spinner = Spinner::start(out);

    spinner.set(&format!("downloading hedos {latest}"));
    let listing = client
        .get(format!("{base}/{archive}.sha256"))
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|error| {
            CliError::new(format!(
                "could not fetch the checksum for {archive}: {error}"
            ))
        })?
        .text()
        .await
        .map_err(|error| {
            CliError::new(format!(
                "could not read the checksum for {archive}: {error}"
            ))
        })?;
    let expected = upgrade::expected_sha256(&listing, &archive).ok_or_else(|| {
        CliError::new(format!(
            "the release's checksum file names no digest for {archive}"
        ))
    })?;
    let archive_path = scratch.path.join(&archive);
    download(
        &client,
        &format!("{base}/{archive}"),
        &archive_path,
        &mut spinner,
        latest,
    )
    .await
    .map_err(|error| CliError::new(format!("could not download {archive}: {error}")))?;

    spinner.set("checking the download");
    let actual = upgrade::sha256_file(&archive_path)?;
    if actual != expected {
        return Err(CliError::new(format!(
            "{archive} does not match its published checksum (expected {expected}, got {actual}); nothing was installed"
        )));
    }

    spinner.set("unpacking");
    let unpacked = tokio::process::Command::new("tar")
        .arg("-xJf")
        .arg(&archive_path)
        .arg("-C")
        .arg(&scratch.path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .status()
        .await
        .map_err(|error| {
            CliError::new(format!("could not run tar to unpack {archive}: {error}"))
        })?;
    if !unpacked.success() {
        return Err(CliError::new(format!(
            "tar could not unpack {archive} ({unpacked}); does it support .xz?"
        )));
    }
    let binary = scratch.path.join(format!("hedos-{target}")).join("hedos");
    if !binary.is_file() {
        return Err(CliError::new(format!("{archive} holds no hedos binary")));
    }
    reports(&binary, latest).await.map_err(|reported| {
        CliError::new(format!(
            "the downloaded hedos reports {reported} rather than {latest}; nothing was installed"
        ))
    })?;

    spinner.set(&format!("installing to {}", exe.display()));
    install_over(&binary, exe)?;
    spinner.clear();
    Ok(())
}

/// Stream `url` into the file at `path`, the spinner counting the bytes.
async fn download(
    client: &reqwest::Client,
    url: &str,
    path: &Path,
    spinner: &mut Spinner,
    latest: &Version,
) -> Result<(), String> {
    let mut response = client
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|error| error.to_string())?;
    let total = response.content_length();
    // Off a terminal the spinner writes each message as a line of its own, so
    // the byte count is shown only where it redraws in place.
    let live = std::io::stderr().is_terminal();
    let mut file = tokio::fs::File::create(path)
        .await
        .map_err(|error| error.to_string())?;
    let mut received: u64 = 0;
    while let Some(chunk) = response.chunk().await.map_err(|error| error.to_string())? {
        file.write_all(&chunk)
            .await
            .map_err(|error| error.to_string())?;
        received += chunk.len() as u64;
        if !live {
            continue;
        }
        let progress = match total {
            Some(total) => format!(
                "{} of {}",
                format_bytes(received as i64),
                format_bytes(total as i64)
            ),
            None => format_bytes(received as i64),
        };
        spinner.set(&format!("downloading hedos {latest} ({progress})"));
    }
    file.flush().await.map_err(|error| error.to_string())
}

/// Run `binary` and check it reports `latest`: `Err` says what it reported
/// instead, or why it did not run.
async fn reports(binary: &Path, latest: &Version) -> Result<(), String> {
    let output = tokio::process::Command::new(binary)
        .arg("--version")
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|error| format!("nothing (it does not run here: {error})"))?;
    let reported = String::from_utf8_lossy(&output.stdout);
    let says_latest = reported
        .split_whitespace()
        .filter_map(Version::parse)
        .any(|version| &version == latest);
    if output.status.success() && says_latest {
        Ok(())
    } else {
        Err(format!("`{}`", reported.trim()))
    }
}

/// Copy `binary` beside `exe` into a file made fresh for it, make it
/// executable, and rename it over `exe`. The rename is atomic within the
/// directory, and a process still running the old binary keeps it open until
/// it exits.
fn install_over(binary: &Path, exe: &Path) -> Result<(), CliError> {
    let dir = exe
        .parent()
        .ok_or_else(|| CliError::new(format!("{} has no directory", exe.display())))?;
    let (staged, file) =
        fresh_file(dir, ".hedos-upgrade").map_err(|error| refused(dir, exe, error))?;
    let result = (|| {
        let mut file = file;
        std::io::copy(&mut std::fs::File::open(binary)?, &mut file)?;
        file.sync_all()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o755))?;
        }
        drop(file);
        std::fs::rename(&staged, exe)
    })();
    result.map_err(|error| {
        let _ = std::fs::remove_file(&staged);
        refused(dir, exe, error)
    })
}

fn refused(dir: &Path, exe: &Path, error: std::io::Error) -> CliError {
    if error.kind() == std::io::ErrorKind::PermissionDenied {
        CliError::new(format!(
            "cannot write to {}: {error}. Run `hedos upgrade` as a user that may, or reinstall with `curl -fsSL https://hedos.ai/install | bash`",
            dir.display()
        ))
    } else {
        CliError::new(format!("could not install over {}: {error}", exe.display()))
    }
}

/// A name under `dir` starting with `stem` that nothing holds yet, created
/// here and now: an existing file, directory or link of that name is never
/// reused or followed, so nothing placed there in advance can be written
/// through or swapped in.
fn fresh_name<T>(
    dir: &Path,
    stem: &str,
    mut create: impl FnMut(&Path) -> std::io::Result<T>,
) -> std::io::Result<(PathBuf, T)> {
    let mut last = None;
    for attempt in 0..16u32 {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.subsec_nanos());
        let path = dir.join(format!("{stem}-{}-{nanos}-{attempt}", std::process::id()));
        match create(&path) {
            Ok(made) => return Ok((path, made)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => last = Some(error),
            Err(error) => return Err(error),
        }
    }
    Err(last.unwrap_or_else(|| std::io::Error::from(std::io::ErrorKind::AlreadyExists)))
}

fn fresh_file(dir: &Path, stem: &str) -> std::io::Result<(PathBuf, std::fs::File)> {
    fresh_name(dir, stem, |path| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o700);
        options.open(path)
    })
}

/// A scratch directory for the download, private to this user and made fresh
/// for this upgrade, removed however the upgrade ends.
struct Scratch {
    path: PathBuf,
}

impl Scratch {
    fn create() -> Result<Self, CliError> {
        let (path, ()) = fresh_name(&std::env::temp_dir(), "hedos-upgrade", |path| {
            let mut builder = std::fs::DirBuilder::new();
            #[cfg(unix)]
            std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
            builder.create(path)
        })?;
        Ok(Self { path })
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Look up the newest release and remember it, as the process each command
/// starts at most once a day does. Its outcome is only ever the file it
/// writes.
pub async fn check() -> Result<(), CliError> {
    let _ = releases::record_latest().await;
    Ok(())
}
