//! Which `llama-server` hedos runs, and what version it is. One place decides
//! for every surface: the path the config names, else the build hedos
//! installed when asked to, else the build hedos carries, else the one on
//! PATH or where Homebrew puts it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use kernel::engines::llama_cpp::{
    INSTALLED_FILE, InstallSource, Installed, LlamaVersion, PINNED, Variant, install_dir_name,
    parse_llama_version,
};

use super::bundled::bundled;
use crate::boot::HedosDirs;
use crate::machine::{Stream, run_bounded_reading};
use crate::settings::SettingsStore;

/// How long `llama-server --version` may take. It answers in tens of
/// milliseconds; one still going after this is wedged.
const VERSION_TIMEOUT: Duration = Duration::from_secs(3);

/// The program llama.cpp serves models with.
const PROGRAM: &str = "llama-server";

/// Where a `llama-server` was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The path `[engines] llama_server` names.
    Config,
    /// A build hedos installed because someone asked for it.
    Hedos,
    /// The build hedos carries inside itself, unpacked or to be unpacked
    /// the first time a server starts.
    Bundled,
    /// PATH, or where Homebrew installs it.
    Path,
}

impl Source {
    /// The stable string form.
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Config => "config",
            Source::Hedos => "hedos",
            Source::Bundled => "bundled",
            Source::Path => "path",
        }
    }
}

/// A `llama-server` hedos can run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlamaServer {
    /// The executable.
    pub path: PathBuf,
    /// Where it was found.
    pub from: Source,
}

/// Where hedos keeps the llama.cpp builds it installs, under the data dir.
pub fn hedos_root(data: &Path) -> PathBuf {
    data.join("engines").join("llama.cpp")
}

/// The `llama-server` to run, the first of: `configured` when it names an
/// executable; a build hedos installed under `data` because someone asked for
/// it (a CUDA build, say); the build hedos carries, where it is or will be
/// unpacked; one on the PATH or in Homebrew's places. Every build of hedos's
/// own comes before the PATH, so an older llama.cpp installed there is never
/// run by default; `configured` is how to choose one over hedos's.
pub fn locate(configured: &str, data: &Path) -> Option<LlamaServer> {
    locate_carrying(configured, data, bundled().map(|bundle| bundle.variant))
}

/// [`locate`], for a binary that carries the build of `carried`, or none.
pub(crate) fn locate_carrying(
    configured: &str,
    data: &Path,
    carried: Option<Variant>,
) -> Option<LlamaServer> {
    let configured = Path::new(configured.trim());
    // Absolute only: a bare name would be looked up on PATH when run, not
    // where it was checked.
    if configured.is_absolute() && kernel::fs::is_executable(configured) {
        return Some(LlamaServer {
            path: configured.to_path_buf(),
            from: Source::Config,
        });
    }
    if let Some((dir, _)) = installed(data, InstallSource::Requested) {
        return Some(LlamaServer {
            path: dir.join(PROGRAM),
            from: Source::Hedos,
        });
    }
    let carried = match carried {
        Some(variant) => Some(
            hedos_root(data)
                .join(install_dir_name(
                    PINNED.build,
                    variant,
                    InstallSource::Bundled,
                ))
                .join(PROGRAM),
        ),
        // A build an earlier hedos carried and unpacked is still hedos's own.
        None => installed(data, InstallSource::Bundled).map(|(dir, _)| dir.join(PROGRAM)),
    };
    if let Some(path) = carried {
        return Some(LlamaServer {
            path,
            from: Source::Bundled,
        });
    }
    on_path().map(|path| LlamaServer {
        path,
        from: Source::Path,
    })
}

/// The `llama-server` on the PATH, or in Homebrew's places.
pub fn on_path() -> Option<PathBuf> {
    kernel::fs::find_on_path(PROGRAM)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            ["/opt/homebrew/bin", "/usr/local/bin"]
                .into_iter()
                .map(|dir| Path::new(dir).join(PROGRAM))
                .find(|candidate| kernel::fs::is_executable(candidate))
        })
}

/// [`locate`] with this process's settings and data dir.
pub fn located() -> Option<LlamaServer> {
    let settings = SettingsStore::discover().load();
    locate(&settings.engines.llama_server, &HedosDirs::detect().data)
}

/// The build from `source` hedos keeps under `data` and what its
/// `installed.json` says: the pinned build when it is there, else the newest,
/// and of one build the one installed last. Only a directory of its own counts
/// (not a link out of the root), named for the build, variant and source its
/// record says, with an executable `llama-server` in it.
pub fn installed(data: &Path, source: InstallSource) -> Option<(PathBuf, Installed)> {
    let entries = std::fs::read_dir(hedos_root(data)).ok()?;
    let mut builds: Vec<(u64, PathBuf, Installed)> = entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| {
            let dir = entry.path();
            let bytes = kernel::fs::read_regular(&dir.join(INSTALLED_FILE)).ok()?;
            let record = serde_json::from_slice::<Installed>(&bytes)
                .ok()
                .filter(|record| record.source == source)?;
            let number = kernel::engines::llama_cpp::parse_build_tag(&record.build)?;
            let named = entry.file_name().to_str()
                == Some(install_dir_name(&record.build, record.variant, record.source).as_str());
            (named && kernel::fs::is_executable(&dir.join(PROGRAM)))
                .then_some((number, dir, record))
        })
        .collect();
    // Of two of one build (an earlier variant whose removal failed), the one
    // installed last.
    builds.sort_by_key(|(number, _, record)| {
        (record.build == PINNED.build, *number, record.installed_at)
    });
    builds.pop().map(|(_, dir, record)| (dir, record))
}

/// Each `llama-server` asked for its version, with the modification time and
/// size it was asked at, and what it said.
type Versions = HashMap<PathBuf, ((SystemTime, u64), Option<LlamaVersion>)>;

/// The version the `llama-server` at `path` reports, asked once per file:
/// it is asked again only after the file changes.
pub fn version(path: &Path) -> Option<LlamaVersion> {
    static SEEN: Mutex<Option<Versions>> = Mutex::new(None);
    let signature = std::fs::metadata(path)
        .ok()
        .and_then(|meta| Some((meta.modified().ok()?, meta.len())));
    if let (Some(signature), Ok(seen)) = (signature, SEEN.lock())
        && let Some((when, version)) = seen.as_ref().and_then(|seen| seen.get(path))
        && *when == signature
    {
        return version.clone();
    }
    let version = run_bounded_reading(path, &["--version"], VERSION_TIMEOUT, Stream::Stderr)
        .as_deref()
        .and_then(parse_llama_version);
    if let (Some(signature), Ok(mut seen)) = (signature, SEEN.lock()) {
        seen.get_or_insert_with(HashMap::new)
            .insert(path.to_path_buf(), (signature, version.clone()));
    }
    version
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("hedos-llama-locate-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// An executable shell script at `path` that prints `version` to stderr.
    fn fake_server(path: &Path, version: &str) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, format!("#!/bin/sh\necho 'version: {version}' >&2\n")).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn install(data: &Path, build: &str, variant: Variant) -> PathBuf {
        install_from(data, build, variant, InstallSource::Requested)
    }

    fn install_from(data: &Path, build: &str, variant: Variant, source: InstallSource) -> PathBuf {
        let dir = hedos_root(data).join(install_dir_name(build, variant, source));
        fake_server(&dir.join(PROGRAM), "0.7.0 (build 11669, commit x)");
        let record = Installed {
            version: "0.7.0".to_owned(),
            build: build.to_owned(),
            variant,
            sha256: "0".repeat(64),
            installed_at: 0,
            source,
            files: Vec::new(),
        };
        std::fs::write(
            dir.join(INSTALLED_FILE),
            serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
        dir
    }

    #[test]
    fn the_configured_path_wins_then_hedos_build_then_path() {
        let scratch = Scratch::new("order");
        let data = scratch.0.join("data");
        let configured = scratch.0.join("custom/llama-server");
        fake_server(&configured, "1 (x)");
        let ours = install(&data, PINNED.build, Variant::Metal);

        let found = locate(configured.to_str().unwrap(), &data).unwrap();
        assert_eq!(
            (found.path, found.from),
            (configured.clone(), Source::Config)
        );

        let found = locate("", &data).unwrap();
        assert_eq!(
            (found.path, found.from),
            (ours.join(PROGRAM), Source::Hedos)
        );

        // A configured path that is not there is passed over.
        let found = locate("/no/such/llama-server", &data).unwrap();
        assert_eq!(found.from, Source::Hedos);
    }

    #[test]
    fn the_carried_build_comes_after_a_requested_one_and_before_the_path() {
        let scratch = Scratch::new("carried");
        let data = scratch.0.join("data");
        let carried = hedos_root(&data)
            .join(install_dir_name(
                PINNED.build,
                Variant::Cpu,
                InstallSource::Bundled,
            ))
            .join(PROGRAM);

        // Carried and not unpacked yet: it is still the one hedos will run,
        // ahead of any llama-server on the PATH.
        let found = locate_carrying("", &data, Some(Variant::Cpu)).unwrap();
        assert_eq!((found.path, found.from), (carried.clone(), Source::Bundled));

        let configured = scratch.0.join("custom/llama-server");
        fake_server(&configured, "1 (x)");
        let found = locate_carrying(configured.to_str().unwrap(), &data, Some(Variant::Cpu));
        assert_eq!(found.map(|found| found.from), Some(Source::Config));

        let requested = install(&data, PINNED.build, Variant::Vulkan);
        let found = locate_carrying("", &data, Some(Variant::Cpu)).unwrap();
        assert_eq!(
            (found.path, found.from),
            (requested.join(PROGRAM), Source::Hedos)
        );
    }

    #[test]
    fn a_build_an_earlier_hedos_unpacked_is_run_by_one_that_carries_none() {
        let scratch = Scratch::new("leftover");
        let data = scratch.0.join("data");
        assert!(locate_carrying("", &data, None).is_none_or(|found| found.from == Source::Path));
        let unpacked = install_from(&data, PINNED.build, Variant::Metal, InstallSource::Bundled);
        let found = locate_carrying("", &data, None).unwrap();
        assert_eq!(
            (found.path, found.from),
            (unpacked.join(PROGRAM), Source::Bundled)
        );
        assert_eq!(installed(&data, InstallSource::Requested), None);
    }

    #[test]
    fn the_pinned_build_is_preferred_over_a_newer_leftover() {
        let scratch = Scratch::new("pinned");
        install(&scratch.0, "b99999", Variant::Cpu);
        let pinned = install(&scratch.0, PINNED.build, Variant::Metal);
        let (dir, record) = installed(&scratch.0, InstallSource::Requested).unwrap();
        assert_eq!(dir, pinned);
        assert_eq!(record.variant, Variant::Metal);

        // A directory with no record is not an install.
        fake_server(&hedos_root(&scratch.0).join("stray").join(PROGRAM), "1 (x)");
        assert_eq!(
            installed(&scratch.0, InstallSource::Requested).unwrap().0,
            pinned
        );
    }

    #[test]
    fn of_two_variants_of_one_build_the_one_installed_last_wins() {
        let scratch = Scratch::new("tie");
        let older = install(&scratch.0, PINNED.build, Variant::Metal);
        let newer = install(&scratch.0, PINNED.build, Variant::Cpu);
        let stamp = |dir: &Path, at: i64| {
            let path = dir.join(INSTALLED_FILE);
            let mut record: Installed =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            record.installed_at = at;
            std::fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
        };
        stamp(&older, 1);
        stamp(&newer, 2);
        assert_eq!(
            installed(&scratch.0, InstallSource::Requested).unwrap().0,
            newer
        );
        stamp(&older, 3);
        assert_eq!(
            installed(&scratch.0, InstallSource::Requested).unwrap().0,
            older
        );
    }

    #[test]
    fn a_directory_misnamed_for_its_record_or_a_link_is_not_an_install() {
        let scratch = Scratch::new("planted");
        let data = scratch.0.join("data");
        let elsewhere = install(&scratch.0.join("other"), PINNED.build, Variant::Metal);
        std::fs::create_dir_all(hedos_root(&data)).unwrap();
        std::os::unix::fs::symlink(
            &elsewhere,
            hedos_root(&data).join(install_dir_name(
                PINNED.build,
                Variant::Metal,
                InstallSource::Requested,
            )),
        )
        .unwrap();
        assert_eq!(installed(&data, InstallSource::Requested), None);

        let misnamed = install(&data, PINNED.build, Variant::Cpu);
        std::fs::rename(&misnamed, hedos_root(&data).join("b11669-vulkan")).unwrap();
        assert_eq!(installed(&data, InstallSource::Requested), None);
    }

    #[test]
    fn a_configured_path_must_be_absolute() {
        let scratch = Scratch::new("relative");
        assert!(
            locate("llama-server", &scratch.0).is_none_or(|found| found.from != Source::Config)
        );
    }

    #[test]
    fn a_version_is_read_from_stderr_and_kept_until_the_file_changes() {
        let scratch = Scratch::new("version");
        let server = scratch.0.join(PROGRAM);
        fake_server(&server, "0.7.0-dev (build 11756, commit d80619397)");
        assert_eq!(version(&server).map(|v| v.build), Some(11756));
        fake_server(&server, "10050 (b15ca938a) and more");
        assert_eq!(version(&server).map(|v| v.build), Some(10050));
        assert_eq!(version(&scratch.0.join("missing")), None);
    }
}
