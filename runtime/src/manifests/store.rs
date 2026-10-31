//! Loading user-installed runtime manifests from a `runtimes.d` directory, and
//! the catalog that wraps it. Each entry is either a bare `*.toml` (invoke-only
//! runtimes) or a directory holding `manifest.toml` beside its files; loading
//! validates ids, provenance, and structure, computing a consent hash over each
//! runtime's files so a later change is detectable.

use std::collections::HashSet;
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};

use kernel::manifests::{RuntimeManifest, RuntimeProvenance};
use sha2::{Digest, Sha256};

/// Whether a load hashes each runtime's directory for consent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Hashing {
    /// Hash it, for approving a runtime to run.
    Hash,
    /// Leave it, for asking only what a runtime declares.
    Skip,
}

/// The manifests a load produced and the human-readable issues it skipped or
/// warned on.
#[derive(Debug, Default)]
pub struct StoreLoad {
    /// The runtimes that loaded and passed validation.
    pub manifests: Vec<RuntimeManifest>,
    /// One line per rejected or flagged entry.
    pub issues: Vec<String>,
}

/// The on-disk store of user runtime manifests under a `runtimes.d` directory.
pub struct UserRuntimeStore {
    directory: PathBuf,
}

impl UserRuntimeStore {
    /// A store rooted at `directory`.
    pub fn new(directory: PathBuf) -> Self {
        Self { directory }
    }

    /// Load every manifest under the directory, in filename order, rejecting ids
    /// in `reserved_ids`, duplicates, community runtimes without a `[vm]` section,
    /// and loose `[serve]`/`[env]` manifests; a manifest with no detect rule loads
    /// but is flagged.
    pub fn load(&self, reserved_ids: &HashSet<String>) -> StoreLoad {
        self.load_entries(reserved_ids, Origin::User, Hashing::Hash)
    }

    /// Load the manifest-driven runtimes out of a directory of shipped bundles.
    /// Most bundles there are driven by a hand-coded adapter and keep their
    /// `manifest.toml` as documentation: those carry a reserved id or no detect
    /// rule, and are passed over without an issue. What is left goes through the
    /// same gates as a user manifest.
    pub fn load_shipped(&self, reserved_ids: &HashSet<String>) -> StoreLoad {
        self.load_entries(reserved_ids, Origin::Shipped, Hashing::Hash)
    }

    /// [`load`](Self::load) (`shipped` false) or
    /// [`load_shipped`](Self::load_shipped) through the same gates, hashing
    /// each runtime's directory for consent only as `hashing` says.
    pub(crate) fn load_hashing(
        &self,
        reserved_ids: &HashSet<String>,
        shipped: bool,
        hashing: Hashing,
    ) -> StoreLoad {
        let origin = if shipped {
            Origin::Shipped
        } else {
            Origin::User
        };
        self.load_entries(reserved_ids, origin, hashing)
    }

    fn load_entries(
        &self,
        reserved_ids: &HashSet<String>,
        origin: Origin,
        hashing: Hashing,
    ) -> StoreLoad {
        let mut entries: Vec<PathBuf> = match std::fs::read_dir(&self.directory) {
            Ok(read_dir) => read_dir
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| !is_hidden(path))
                .collect(),
            Err(_) => return StoreLoad::default(),
        };
        entries.sort_by(|a, b| file_name(a).cmp(file_name(b)));

        let mut load = StoreLoad::default();
        let mut seen: HashSet<String> = HashSet::new();
        for entry in entries {
            let Some((manifest_path, manifest_dir, label)) = entry_target(&entry, origin.label())
            else {
                continue;
            };
            // A shipped runtime is always a folder; the loose TOML beside the
            // bundles is the sidecars' lint configuration, not a manifest.
            if origin == Origin::Shipped && manifest_dir.is_none() {
                continue;
            }
            let text = match std::fs::read_to_string(&manifest_path) {
                Ok(text) => text,
                Err(error) => {
                    load.issues.push(format!("{label}: {error}"));
                    continue;
                }
            };
            let mut manifest = match RuntimeManifest::parse(&text, manifest_dir.clone()) {
                Ok(manifest) => manifest,
                Err(error) => {
                    load.issues.push(format!("{label}: {error}"));
                    continue;
                }
            };
            if origin == Origin::Shipped
                && (manifest.detect.is_none() || reserved_ids.contains(&manifest.id))
            {
                continue;
            }
            if let Some(directory) = &manifest_dir {
                manifest.provenance = RuntimeProvenance::read(directory);
            }

            let is_community = manifest
                .provenance
                .as_ref()
                .is_some_and(RuntimeProvenance::is_community);
            if is_community && manifest.vm.is_none() {
                load.issues.push(format!(
                    "{label}: community runtimes run contained, and \"{}\" has no [vm] section",
                    manifest.id
                ));
                continue;
            }
            if reserved_ids.contains(&manifest.id) {
                load.issues
                    .push(format!("{label}: id \"{}\" is reserved", manifest.id));
                continue;
            }
            if seen.contains(&manifest.id) {
                load.issues.push(format!(
                    "{label}: duplicate id \"{}\", keeping the first",
                    manifest.id
                ));
                continue;
            }
            if manifest_dir.is_none() && (manifest.serve.is_some() || manifest.env.is_some()) {
                load.issues.push(format!(
                    "{label}: [serve] and [env] manifests must live in a directory beside their files"
                ));
                continue;
            }
            if manifest.detect.is_none() {
                load.issues.push(format!(
                    "{label}: manifest \"{}\" has no detect rule and will never match a model",
                    manifest.id
                ));
            }
            // Hash only the manifests that survive the gates — a rejected one is
            // discarded, so hashing its whole directory would be wasted I/O.
            if hashing == Hashing::Hash {
                manifest.content_hash =
                    Some(consent_hash(&text, manifest_dir.as_deref(), &manifest));
            }
            seen.insert(manifest.id.clone());
            load.manifests.push(manifest);
        }
        load
    }
}

/// Where a directory of manifests came from, which decides how an entry that
/// cannot bid is treated.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Origin {
    User,
    Shipped,
}

impl Origin {
    /// The directory name an issue line names the entry under.
    fn label(self) -> &'static str {
        match self {
            Self::User => "runtimes.d",
            Self::Shipped => "bundled",
        }
    }
}

/// Resolve a `runtimes.d` entry to the manifest file to read, its directory (if
/// the entry is a runtime folder), and a display label — or `None` if the entry
/// is neither a `manifest.toml` folder nor a bare `*.toml`.
fn entry_target(entry: &Path, root: &str) -> Option<(PathBuf, Option<PathBuf>, String)> {
    let name = file_name(entry);
    if entry.is_dir() {
        let manifest_path = entry.join("manifest.toml");
        if !manifest_path.is_file() {
            return None;
        }
        let label = format!("{root}/{name}/manifest.toml");
        Some((manifest_path, Some(entry.to_path_buf()), label))
    } else {
        let is_toml = entry
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("toml"));
        if !is_toml {
            return None;
        }
        Some((entry.to_path_buf(), None, format!("{root}/{name}")))
    }
}

/// A hash over everything a runtime consists of, so a later edit invalidates
/// any prior trust: the manifest text, the declared entrypoint/lockfile, the
/// lockfile's CPU variant where the walk below would not reach it, and every
/// other file beside it. Without a directory it is just the manifest text
/// hash.
fn consent_hash(text: &str, directory: Option<&Path>, manifest: &RuntimeManifest) -> String {
    let Some(directory) = directory else {
        let mut hasher = Sha256::new();
        hasher.update(text.as_bytes());
        return hex::encode(hasher.finalize());
    };

    let mut files: Vec<(Vec<u8>, PathBuf)> = Vec::new();
    collect_files(directory, directory, &mut files);
    files.retain(|(relative, _)| relative != b"manifest.toml");
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut hasher = Sha256::new();
    absorb(&mut hasher, b"manifest.toml", Some(text.as_bytes()));
    if let Some(serve) = &manifest.serve {
        let content = std::fs::read(directory.join(&serve.entrypoint)).ok();
        absorb(&mut hasher, serve.entrypoint.as_bytes(), content.as_deref());
    }
    if let Some(env) = &manifest.env {
        let lockfile = directory.join(&env.lockfile);
        let content = std::fs::read(&lockfile).ok();
        absorb(&mut hasher, env.lockfile.as_bytes(), content.as_deref());
        // Found from the joined path, and taken only as a regular file, as
        // the build finds it. One the walk hashed under that very name, or
        // none, adds nothing here, so a runtime keeps the hash it was
        // approved by. Reached any other way
        // (through `..`, or in another case on a disk that ignores case) it
        // is hashed again: consent is asked once more rather than tie the
        // hash to a path that could lead elsewhere.
        if let Some(variant) = crate::environment::cpu_variant(&lockfile)
            && !walked_as_named(&variant, directory, &files)
            && variant.is_file()
            && let Ok(content) = std::fs::read(&variant)
        {
            let name = variant.strip_prefix(directory).unwrap_or(&variant);
            absorb(&mut hasher, name.as_os_str().as_bytes(), Some(&content));
        }
    }

    for (relative, path) in files {
        let content = std::fs::read(&path).ok();
        absorb(&mut hasher, &relative, content.as_deref());
    }
    hex::encode(hasher.finalize())
}

/// Whether the walk hashed `path` under the name it is read by: below
/// `directory` it is plain names only, and it is a path the walk recorded.
/// The walk takes no symlink, so the build then reads that very entry, which
/// cannot go or change without the walk seeing it. A `..` on the way, a
/// hidden or symlinked directory, or a name in another case is never one.
fn walked_as_named(path: &Path, directory: &Path, files: &[(Vec<u8>, PathBuf)]) -> bool {
    let Ok(below) = path.strip_prefix(directory) else {
        return false;
    };
    let mut at = directory.to_path_buf();
    for component in below.components() {
        match component {
            Component::Normal(name) => at.push(name),
            Component::CurDir => {}
            _ => return false,
        }
    }
    files.iter().any(|(_, walked)| *walked == at)
}

/// Fold one `(path, content)` pair into the consent hash: the path bytes, a NUL
/// separator, the content length (or `u64::MAX` when the file is absent), then
/// the content.
fn absorb(hasher: &mut Sha256, path: &[u8], content: Option<&[u8]>) {
    hasher.update(path);
    hasher.update([0u8]);
    let count = content.map_or(u64::MAX, |bytes| bytes.len() as u64);
    hasher.update(count.to_le_bytes());
    if let Some(content) = content {
        hasher.update(content);
    }
}

/// Recursively collect the regular files under `base` (skipping hidden entries
/// and symlinks — a symlinked directory is not descended and a symlinked file is
/// not hashed) as `(path-relative-to-base, absolute-path)` pairs, the relative
/// path as the bytes it is named by, so no two names hash alike.
fn collect_files(base: &Path, current: &Path, out: &mut Vec<(Vec<u8>, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(current) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if is_hidden(&path) {
            continue;
        }
        // symlink_metadata does not follow the link, so a symlink reports as one
        // and is skipped rather than followed.
        let Ok(kind) = std::fs::symlink_metadata(&path).map(|meta| meta.file_type()) else {
            continue;
        };
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            collect_files(base, &path, out);
        } else if kind.is_file()
            && let Ok(relative) = path.strip_prefix(base)
        {
            out.push((relative.as_os_str().as_bytes().to_vec(), path));
        }
    }
}

fn file_name(path: &Path) -> &str {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("")
}

/// Hidden means a leading `.`; a filesystem hidden *flag* on a non-dot name is
/// not consulted (no portable API for it). Filename ordering is likewise byte-wise
/// UTF-8, not locale-collated — both matter only for exotic non-dot / non-ASCII
/// runtime files.
fn is_hidden(path: &Path) -> bool {
    file_name(path).starts_with('.')
}

/// A user runtime catalog: the `runtimes.d` directory plus the built-in ids it
/// must not let a user manifest shadow.
pub struct RuntimeCatalog {
    directory: PathBuf,
    reserved_ids: HashSet<String>,
}

impl RuntimeCatalog {
    /// A catalog over `directory`, reserving `reserved_ids` for the built-ins.
    pub fn new(directory: PathBuf, reserved_ids: HashSet<String>) -> Self {
        Self {
            directory,
            reserved_ids,
        }
    }

    /// The catalog directory, created if it does not yet exist.
    pub fn ensured_directory(&self) -> &Path {
        let _ = std::fs::create_dir_all(&self.directory);
        &self.directory
    }

    /// Load every user manifest, honoring the reserved ids.
    pub fn load(&self) -> StoreLoad {
        UserRuntimeStore::new(self.directory.clone()).load(&self.reserved_ids)
    }

    /// The community-installed runtimes among the loaded manifests.
    pub fn installed_community(&self) -> Vec<RuntimeManifest> {
        self.load()
            .manifests
            .into_iter()
            .filter(|manifest| {
                manifest
                    .provenance
                    .as_ref()
                    .is_some_and(RuntimeProvenance::is_community)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INVOKE_MANIFEST: &str = "id = \"one\"\ncapabilities = [\"chat\"]\nexecution = \"sync\"\ndetect = { extension = \"gguf\" }\n[invoke]\ncommand = \"tool --model {model}\"\n";

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("hedos-runtimes-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn it_loads_a_bare_invoke_manifest_and_hashes_it() {
        let dir = temp_dir("bare");
        std::fs::write(dir.join("one.toml"), INVOKE_MANIFEST).unwrap();
        let load = UserRuntimeStore::new(dir.clone()).load(&HashSet::new());
        assert_eq!(load.manifests.len(), 1);
        assert!(load.issues.is_empty());
        assert!(load.manifests[0].content_hash.is_some());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_listing_passes_the_same_gates_without_hashing_for_consent() {
        let dir = temp_dir("listing");
        std::fs::write(dir.join("one.toml"), INVOKE_MANIFEST).unwrap();
        std::fs::write(
            dir.join("taken.toml"),
            INVOKE_MANIFEST.replace("\"one\"", "\"taken\""),
        )
        .unwrap();
        let reserved = HashSet::from(["taken".to_owned()]);
        let store = UserRuntimeStore::new(dir.clone());
        let listed = store.load_hashing(&reserved, false, Hashing::Skip);
        let ids: Vec<&str> = listed
            .manifests
            .iter()
            .map(|manifest| manifest.id.as_str())
            .collect();
        assert_eq!(ids, ["one"]);
        assert_eq!(listed.manifests[0].content_hash, None);
        let loaded = store.load(&reserved);
        assert_eq!(loaded.manifests.len(), 1);
        assert!(loaded.manifests[0].content_hash.is_some());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_loose_serve_manifest_is_rejected() {
        let dir = temp_dir("loose-serve");
        std::fs::write(
            dir.join("two.toml"),
            "id = \"two\"\ncapabilities = [\"chat\"]\nexecution = \"stream\"\ndetect = { extension = \"gguf\" }\n[serve]\nentrypoint = \"main.py\"\n",
        )
        .unwrap();
        let load = UserRuntimeStore::new(dir.clone()).load(&HashSet::new());
        assert!(load.manifests.is_empty());
        assert!(
            load.issues
                .iter()
                .any(|issue| issue.contains("must live in a directory"))
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_directory_serve_manifest_loads() {
        let dir = temp_dir("dir-serve");
        let runtime = dir.join("my-runtime");
        std::fs::create_dir_all(&runtime).unwrap();
        std::fs::write(
            runtime.join("manifest.toml"),
            "id = \"three\"\ncapabilities = [\"chat\"]\nexecution = \"stream\"\ndetect = { extension = \"gguf\" }\n[serve]\nentrypoint = \"main.py\"\n[env]\nlockfile = \"requirements.lock\"\n",
        )
        .unwrap();
        std::fs::write(runtime.join("main.py"), "print('hi')").unwrap();
        std::fs::write(runtime.join("requirements.lock"), "torch").unwrap();
        let load = UserRuntimeStore::new(dir.clone()).load(&HashSet::new());
        assert_eq!(load.manifests.len(), 1, "issues: {:?}", load.issues);
        assert_eq!(load.manifests[0].id, "three");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_reserved_id_is_rejected() {
        let dir = temp_dir("reserved");
        std::fs::write(dir.join("one.toml"), INVOKE_MANIFEST).unwrap();
        let reserved = HashSet::from(["one".to_owned()]);
        let load = UserRuntimeStore::new(dir.clone()).load(&reserved);
        assert!(load.manifests.is_empty());
        assert!(
            load.issues
                .iter()
                .any(|issue| issue.contains("is reserved"))
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_duplicate_id_keeps_the_first() {
        let dir = temp_dir("dup");
        std::fs::write(dir.join("a-one.toml"), INVOKE_MANIFEST).unwrap();
        std::fs::write(dir.join("b-one.toml"), INVOKE_MANIFEST).unwrap();
        let load = UserRuntimeStore::new(dir.clone()).load(&HashSet::new());
        assert_eq!(load.manifests.len(), 1);
        assert!(
            load.issues
                .iter()
                .any(|issue| issue.contains("duplicate id") && !issue.contains('\u{2014}'))
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_manifest_without_a_detect_rule_loads_but_is_flagged() {
        let dir = temp_dir("no-detect");
        std::fs::write(
            dir.join("nd.toml"),
            "id = \"nd\"\ncapabilities = [\"chat\"]\nexecution = \"sync\"\n[invoke]\ncommand = \"x\"\n",
        )
        .unwrap();
        let load = UserRuntimeStore::new(dir.clone()).load(&HashSet::new());
        assert_eq!(load.manifests.len(), 1);
        assert!(
            load.issues
                .iter()
                .any(|issue| issue.contains("no detect rule"))
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_consent_hash_is_stable_and_content_sensitive() {
        let dir = temp_dir("hash");
        let runtime = dir.join("r");
        std::fs::create_dir_all(&runtime).unwrap();
        let manifest = "id = \"r\"\ncapabilities = [\"chat\"]\nexecution = \"stream\"\ndetect = { extension = \"gguf\" }\n[serve]\nentrypoint = \"main.py\"\n";
        std::fs::write(runtime.join("manifest.toml"), manifest).unwrap();
        std::fs::write(runtime.join("main.py"), "one").unwrap();
        let first = UserRuntimeStore::new(dir.clone())
            .load(&HashSet::new())
            .manifests[0]
            .content_hash
            .clone();
        // A change to a sibling file changes the hash.
        std::fs::write(runtime.join("main.py"), "two").unwrap();
        let second = UserRuntimeStore::new(dir.clone())
            .load(&HashSet::new())
            .manifests[0]
            .content_hash
            .clone();
        assert!(first.is_some());
        assert_ne!(first, second);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The CPU variant a Linux machine without NVIDIA's driver installs in
    /// place of the declared lockfile is hashed as the lockfile is, wherever
    /// it sits.
    #[cfg(unix)]
    #[test]
    fn the_consent_hash_covers_the_lockfile_s_cpu_variant() {
        let dir = temp_dir("variant");
        let runtime = dir.join("r");
        let deps = dir.join("deps");
        std::fs::create_dir_all(&runtime).unwrap();
        std::fs::create_dir_all(&deps).unwrap();
        let manifest = "id = \"r\"\ncapabilities = [\"chat\"]\nexecution = \"stream\"\ndetect = { extension = \"gguf\" }\n[serve]\nentrypoint = \"main.py\"\n[env]\nlockfile = \"deps/requirements.lock\"\n";
        std::fs::write(runtime.join("manifest.toml"), manifest).unwrap();
        std::fs::write(runtime.join("main.py"), "one").unwrap();
        std::fs::write(deps.join("requirements.lock"), "torch==1").unwrap();
        // The lockfile's directory is a link, which the walk over the
        // runtime's files passes over.
        std::os::unix::fs::symlink(&deps, runtime.join("deps")).unwrap();
        let hash = || {
            UserRuntimeStore::new(dir.clone())
                .load(&HashSet::new())
                .manifests
                .iter()
                .find(|manifest| manifest.id == "r")
                .and_then(|manifest| manifest.content_hash.clone())
        };
        let without = hash();
        std::fs::write(deps.join("requirements.cpu.lock"), "torch==1+cpu").unwrap();
        let with = hash();
        std::fs::write(deps.join("requirements.cpu.lock"), "torch==2+cpu").unwrap();
        let changed = hash();
        assert!(without.is_some());
        assert_ne!(without, with);
        assert_ne!(with, changed);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A lockfile of `.` names the runtime's own directory, whose variant, as
    /// the build reads it, sits beside that directory.
    #[test]
    fn the_consent_hash_covers_a_variant_outside_the_runtime_s_directory() {
        let dir = temp_dir("variant-outside");
        let runtime = dir.join("r.lock");
        std::fs::create_dir_all(&runtime).unwrap();
        let manifest = "id = \"r\"\ncapabilities = [\"chat\"]\nexecution = \"stream\"\ndetect = { extension = \"gguf\" }\n[serve]\nentrypoint = \"main.py\"\n[env]\nlockfile = \".\"\n";
        std::fs::write(runtime.join("manifest.toml"), manifest).unwrap();
        std::fs::write(runtime.join("main.py"), "one").unwrap();
        let variant = crate::environment::cpu_variant(&runtime.join(".")).unwrap();
        assert_eq!(variant, dir.join("r.cpu.lock"));
        let hash = || {
            UserRuntimeStore::new(dir.clone())
                .load(&HashSet::new())
                .manifests
                .iter()
                .find(|manifest| manifest.id == "r")
                .and_then(|manifest| manifest.content_hash.clone())
        };
        std::fs::write(&variant, "torch==1").unwrap();
        let first = hash();
        std::fs::write(&variant, "torch==2").unwrap();
        let second = hash();
        assert!(first.is_some());
        assert_ne!(first, second);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_sibling_is_not_hashed() {
        let dir = temp_dir("symlink");
        let runtime = dir.join("r");
        std::fs::create_dir_all(&runtime).unwrap();
        let manifest = "id = \"r\"\ncapabilities = [\"chat\"]\nexecution = \"stream\"\ndetect = { extension = \"gguf\" }\n[serve]\nentrypoint = \"main.py\"\n";
        std::fs::write(runtime.join("manifest.toml"), manifest).unwrap();
        std::fs::write(runtime.join("main.py"), "one").unwrap();
        let before = UserRuntimeStore::new(dir.clone())
            .load(&HashSet::new())
            .manifests[0]
            .content_hash
            .clone();
        std::os::unix::fs::symlink(runtime.join("main.py"), runtime.join("link.py")).unwrap();
        let after = UserRuntimeStore::new(dir.clone())
            .load(&HashSet::new())
            .manifests[0]
            .content_hash
            .clone();
        assert_eq!(before, after, "a symlink must not change the consent hash");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The hashes this runtime had before CPU variants were looked for, with
    /// no variant and with one beside its lockfile (which the walk already
    /// hashed), so an approval given then still holds.
    #[test]
    fn a_runtime_keeps_the_consent_hash_it_was_approved_by() {
        let dir = temp_dir("approved");
        let runtime = dir.join("r");
        std::fs::create_dir_all(&runtime).unwrap();
        let manifest = "id = \"r\"\ncapabilities = [\"chat\"]\nexecution = \"stream\"\ndetect = { extension = \"gguf\" }\n[serve]\nentrypoint = \"main.py\"\n[env]\nlockfile = \"requirements.lock\"\n";
        std::fs::write(runtime.join("manifest.toml"), manifest).unwrap();
        std::fs::write(runtime.join("main.py"), "one").unwrap();
        std::fs::write(runtime.join("requirements.lock"), "x==1").unwrap();
        let hash = || {
            UserRuntimeStore::new(dir.clone())
                .load(&HashSet::new())
                .manifests[0]
                .content_hash
                .clone()
        };
        assert_eq!(
            hash().as_deref(),
            Some("cd51fc2ea4dffc72c420f49dc6aa96dde1e237dd28336894fede34476411707c")
        );
        std::fs::write(runtime.join("requirements.cpu.lock"), "x==1+cpu").unwrap();
        assert_eq!(
            hash().as_deref(),
            Some("105b5e26bdaeaa76e0a9e672d449a6d8ef7caed54d3271092768508b197a3399")
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A manifest that names its lockfile through `..` has the variant hashed
    /// again, beside the walk: consent is asked once more (HEAD's code gave
    /// 066d04eb...), as a path through `..` could lead elsewhere.
    #[test]
    fn a_lockfile_named_through_dot_dot_has_its_variant_hashed_again() {
        let dir = temp_dir("approved-around");
        let runtime = dir.join("r");
        std::fs::create_dir_all(&runtime).unwrap();
        let manifest = "id = \"r\"\ncapabilities = [\"chat\"]\nexecution = \"stream\"\ndetect = { extension = \"gguf\" }\n[serve]\nentrypoint = \"main.py\"\n[env]\nlockfile = \"../r/requirements.lock\"\n";
        std::fs::write(runtime.join("manifest.toml"), manifest).unwrap();
        std::fs::write(runtime.join("main.py"), "one").unwrap();
        std::fs::write(runtime.join("requirements.lock"), "x==1").unwrap();
        std::fs::write(runtime.join("requirements.cpu.lock"), "x==1+cpu").unwrap();
        let hash = UserRuntimeStore::new(dir.clone())
            .load(&HashSet::new())
            .manifests[0]
            .content_hash
            .clone();
        assert_eq!(
            hash.as_deref(),
            Some("222f2accb6bb7f3531d8e5001e9f6855214d971620fa34ce4a92cdcc8eb3156d")
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// On a disk that ignores case, a variant named in another case than the
    /// walked file is hashed under that name too, where before only the walk
    /// hashed it.
    #[test]
    fn the_consent_hash_covers_a_variant_named_in_another_case() {
        let dir = temp_dir("variant-case");
        let runtime = dir.join("r");
        std::fs::create_dir_all(&runtime).unwrap();
        let manifest = "id = \"r\"\ncapabilities = [\"chat\"]\nexecution = \"stream\"\ndetect = { extension = \"gguf\" }\n[serve]\nentrypoint = \"main.py\"\n[env]\nlockfile = \"Requirements.lock\"\n";
        std::fs::write(runtime.join("manifest.toml"), manifest).unwrap();
        std::fs::write(runtime.join("main.py"), "one").unwrap();
        std::fs::write(runtime.join("requirements.lock"), "x==1").unwrap();
        std::fs::write(runtime.join("requirements.cpu.lock"), "x==1+cpu").unwrap();
        let hash = || {
            UserRuntimeStore::new(dir.clone())
                .load(&HashSet::new())
                .manifests[0]
                .content_hash
                .clone()
        };
        if !runtime.join("Requirements.cpu.lock").exists() {
            std::fs::remove_dir_all(&dir).ok();
            return;
        }
        assert_eq!(
            hash().as_deref(),
            Some("9c04dd230f94444947bcad36c983550b88733103895d244e73799cbb5d6cea86")
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A variant reached through a hidden directory is hashed, though it is a
    /// hard link to a walked file: linking it to another walked file asks for
    /// consent again, as does adding it.
    #[test]
    fn the_consent_hash_covers_a_hidden_variant_linked_to_a_walked_file() {
        let dir = temp_dir("variant-hard-link");
        let runtime = dir.join("r");
        std::fs::create_dir_all(runtime.join(".deps")).unwrap();
        let manifest = "id = \"r\"\ncapabilities = [\"chat\"]\nexecution = \"stream\"\ndetect = { extension = \"gguf\" }\n[serve]\nentrypoint = \"main.py\"\n[env]\nlockfile = \".deps/requirements.lock\"\n";
        std::fs::write(runtime.join("manifest.toml"), manifest).unwrap();
        std::fs::write(runtime.join("main.py"), "one").unwrap();
        std::fs::write(runtime.join(".deps/requirements.lock"), "x==1").unwrap();
        std::fs::write(runtime.join("a.lock"), "x==1+cpu").unwrap();
        std::fs::write(runtime.join("b.lock"), "x==2+cpu").unwrap();
        let hash = || {
            UserRuntimeStore::new(dir.clone())
                .load(&HashSet::new())
                .manifests[0]
                .content_hash
                .clone()
        };
        let variant = runtime.join(".deps/requirements.cpu.lock");
        let without = hash();
        std::fs::hard_link(runtime.join("a.lock"), &variant).unwrap();
        let on_a = hash();
        assert_ne!(on_a, without);
        std::fs::remove_file(&variant).unwrap();
        std::fs::hard_link(runtime.join("b.lock"), &variant).unwrap();
        assert_ne!(hash(), on_a);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A variant reached through a symlinked directory is hashed, so pointing
    /// the link at another walked directory asks for consent again.
    #[cfg(unix)]
    #[test]
    fn the_consent_hash_follows_a_symlinked_directory_between_walked_ones() {
        let dir = temp_dir("variant-dir-link");
        let runtime = dir.join("r");
        for (folder, pin) in [("one", "x==1+cpu"), ("two", "x==2+cpu")] {
            std::fs::create_dir_all(runtime.join(folder)).unwrap();
            std::fs::write(runtime.join(folder).join("requirements.lock"), "x==1").unwrap();
            std::fs::write(runtime.join(folder).join("requirements.cpu.lock"), pin).unwrap();
        }
        let manifest = "id = \"r\"\ncapabilities = [\"chat\"]\nexecution = \"stream\"\ndetect = { extension = \"gguf\" }\n[serve]\nentrypoint = \"main.py\"\n[env]\nlockfile = \"deps/requirements.lock\"\n";
        std::fs::write(runtime.join("manifest.toml"), manifest).unwrap();
        std::fs::write(runtime.join("main.py"), "one").unwrap();
        std::os::unix::fs::symlink("one", runtime.join("deps")).unwrap();
        let hash = || {
            UserRuntimeStore::new(dir.clone())
                .load(&HashSet::new())
                .manifests[0]
                .content_hash
                .clone()
        };
        let before = hash();
        std::fs::remove_file(runtime.join("deps")).unwrap();
        std::os::unix::fs::symlink("two", runtime.join("deps")).unwrap();
        assert_ne!(hash(), before);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A `..` after a symlinked directory leads where the link does, not back
    /// beside it, so a variant there is hashed though a walked file has its
    /// name.
    #[cfg(unix)]
    #[test]
    fn the_consent_hash_covers_a_variant_a_symlink_s_parent_leads_to() {
        let dir = temp_dir("variant-up-link");
        let runtime = dir.join("r");
        let outside = dir.join("outside");
        std::fs::create_dir_all(outside.join("inner")).unwrap();
        std::fs::create_dir_all(&runtime).unwrap();
        let manifest = "id = \"r\"\ncapabilities = [\"chat\"]\nexecution = \"stream\"\ndetect = { extension = \"gguf\" }\n[serve]\nentrypoint = \"main.py\"\n[env]\nlockfile = \"deps/../requirements.lock\"\n";
        std::fs::write(runtime.join("manifest.toml"), manifest).unwrap();
        std::fs::write(runtime.join("main.py"), "one").unwrap();
        std::fs::write(runtime.join("requirements.lock"), "x==1").unwrap();
        std::fs::write(runtime.join("requirements.cpu.lock"), "x==1+cpu").unwrap();
        std::fs::write(outside.join("requirements.lock"), "x==1").unwrap();
        std::fs::write(outside.join("requirements.cpu.lock"), "x==1+cpu").unwrap();
        std::os::unix::fs::symlink(outside.join("inner"), runtime.join("deps")).unwrap();
        let hash = || {
            UserRuntimeStore::new(dir.clone())
                .load(&HashSet::new())
                .manifests[0]
                .content_hash
                .clone()
        };
        let before = hash();
        std::fs::write(outside.join("requirements.cpu.lock"), "x==2+cpu").unwrap();
        assert_ne!(hash(), before);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// On a disk that keeps case, a symlinked directory pointed from one
    /// walked directory to another whose name differs only in case asks for
    /// consent again.
    #[cfg(unix)]
    #[test]
    fn the_consent_hash_follows_a_symlinked_directory_between_names_in_two_cases() {
        let dir = temp_dir("variant-dir-case");
        let runtime = dir.join("r");
        std::fs::create_dir_all(runtime.join("Deps")).unwrap();
        if runtime.join("DEPS").exists() {
            std::fs::remove_dir_all(&dir).ok();
            return;
        }
        for (folder, pin) in [("Deps", "x==1+cpu"), ("DEPS", "x==2+cpu")] {
            std::fs::create_dir_all(runtime.join(folder)).unwrap();
            std::fs::write(runtime.join(folder).join("requirements.lock"), "x==1").unwrap();
            std::fs::write(runtime.join(folder).join("requirements.cpu.lock"), pin).unwrap();
        }
        let manifest = "id = \"r\"\ncapabilities = [\"chat\"]\nexecution = \"stream\"\ndetect = { extension = \"gguf\" }\n[serve]\nentrypoint = \"main.py\"\n[env]\nlockfile = \"deps/requirements.lock\"\n";
        std::fs::write(runtime.join("manifest.toml"), manifest).unwrap();
        std::fs::write(runtime.join("main.py"), "one").unwrap();
        std::os::unix::fs::symlink("Deps", runtime.join("deps")).unwrap();
        let hash = || {
            UserRuntimeStore::new(dir.clone())
                .load(&HashSet::new())
                .manifests[0]
                .content_hash
                .clone()
        };
        let before = hash();
        std::fs::remove_file(runtime.join("deps")).unwrap();
        std::os::unix::fs::symlink("DEPS", runtime.join("deps")).unwrap();
        assert_ne!(hash(), before);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A hard link to a walked file, reached where the walk does not go, is
    /// hashed: taking it away, which turns the build back to the plain lock,
    /// asks for consent again.
    #[cfg(unix)]
    #[test]
    fn the_consent_hash_sees_a_linked_variant_go() {
        let dir = temp_dir("variant-link-gone");
        let runtime = dir.join("r");
        std::fs::create_dir_all(runtime.join("a")).unwrap();
        std::fs::create_dir_all(runtime.join(".outside/inner")).unwrap();
        std::fs::create_dir_all(runtime.join(".outside/a")).unwrap();
        let manifest = "id = \"r\"\ncapabilities = [\"chat\"]\nexecution = \"stream\"\ndetect = { extension = \"gguf\" }\n[serve]\nentrypoint = \"main.py\"\n[env]\nlockfile = \"deps/../a/requirements.lock\"\n";
        std::fs::write(runtime.join("manifest.toml"), manifest).unwrap();
        std::fs::write(runtime.join("main.py"), "one").unwrap();
        std::fs::write(runtime.join("a/requirements.cpu.lock"), "x==1+cpu").unwrap();
        std::fs::write(runtime.join(".outside/a/requirements.lock"), "x==1").unwrap();
        std::os::unix::fs::symlink(".outside/inner", runtime.join("deps")).unwrap();
        let variant = runtime.join(".outside/a/requirements.cpu.lock");
        std::fs::hard_link(runtime.join("a/requirements.cpu.lock"), &variant).unwrap();
        let hash = || {
            UserRuntimeStore::new(dir.clone())
                .load(&HashSet::new())
                .manifests[0]
                .content_hash
                .clone()
        };
        let before = hash();
        std::fs::remove_file(&variant).unwrap();
        assert_ne!(hash(), before);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A symlink to a walked file, reached where the walk does not go, is
    /// hashed as one: taking it away asks for consent again.
    #[cfg(unix)]
    #[test]
    fn the_consent_hash_sees_a_symlinked_variant_go() {
        let dir = temp_dir("variant-symlink-gone");
        let runtime = dir.join("r");
        std::fs::create_dir_all(runtime.join("a")).unwrap();
        std::fs::create_dir_all(runtime.join(".outside/inner")).unwrap();
        std::fs::create_dir_all(runtime.join(".outside/a")).unwrap();
        let manifest = "id = \"r\"\ncapabilities = [\"chat\"]\nexecution = \"stream\"\ndetect = { extension = \"gguf\" }\n[serve]\nentrypoint = \"main.py\"\n[env]\nlockfile = \"deps/../a/requirements.lock\"\n";
        std::fs::write(runtime.join("manifest.toml"), manifest).unwrap();
        std::fs::write(runtime.join("main.py"), "one").unwrap();
        std::fs::write(runtime.join("a/requirements.cpu.lock"), "x==1+cpu").unwrap();
        std::fs::write(runtime.join(".outside/a/requirements.lock"), "x==1").unwrap();
        std::os::unix::fs::symlink(".outside/inner", runtime.join("deps")).unwrap();
        let variant = runtime.join(".outside/a/requirements.cpu.lock");
        std::os::unix::fs::symlink(runtime.join("a/requirements.cpu.lock"), &variant).unwrap();
        let hash = || {
            UserRuntimeStore::new(dir.clone())
                .load(&HashSet::new())
                .manifests[0]
                .content_hash
                .clone()
        };
        let before = hash();
        std::fs::remove_file(&variant).unwrap();
        assert_ne!(hash(), before);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A variant reached through a symlinked directory, though that leads
    /// to a walked file, is hashed: pointing the link where there is none
    /// asks for consent again.
    #[cfg(unix)]
    #[test]
    fn the_consent_hash_sees_a_variant_go_behind_a_symlinked_directory() {
        let dir = temp_dir("variant-link-way");
        let runtime = dir.join("r");
        std::fs::create_dir_all(runtime.join("inner")).unwrap();
        std::fs::create_dir_all(runtime.join(".outside/inner")).unwrap();
        let manifest = "id = \"r\"\ncapabilities = [\"chat\"]\nexecution = \"stream\"\ndetect = { extension = \"gguf\" }\n[serve]\nentrypoint = \"main.py\"\n[env]\nlockfile = \"deps/../requirements.lock\"\n";
        std::fs::write(runtime.join("manifest.toml"), manifest).unwrap();
        std::fs::write(runtime.join("main.py"), "one").unwrap();
        std::fs::write(runtime.join("requirements.lock"), "x==1").unwrap();
        std::fs::write(runtime.join("requirements.cpu.lock"), "x==1+cpu").unwrap();
        std::fs::write(runtime.join(".outside/requirements.lock"), "x==1").unwrap();
        std::os::unix::fs::symlink("inner", runtime.join("deps")).unwrap();
        let hash = || {
            UserRuntimeStore::new(dir.clone())
                .load(&HashSet::new())
                .manifests[0]
                .content_hash
                .clone()
        };
        let before = hash();
        std::fs::remove_file(runtime.join("deps")).unwrap();
        std::os::unix::fs::symlink(".outside/inner", runtime.join("deps")).unwrap();
        assert_ne!(hash(), before);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A runtime that is a symlink, naming its lockfile through `..`, climbs
    /// out where the link leads: a variant found there is hashed, though a
    /// walked file has its name, and taking it away asks for consent again.
    #[cfg(unix)]
    #[test]
    fn the_consent_hash_sees_a_variant_go_beside_a_symlinked_runtime() {
        let link_kinds: [fn(&Path, &Path) -> std::io::Result<()>; 2] = [
            |from, to| std::fs::hard_link(from, to),
            |from, to| std::os::unix::fs::symlink(from, to),
        ];
        for (kind, link) in link_kinds.into_iter().enumerate() {
            let dir = temp_dir(&format!("variant-runtime-link-{kind}"));
            let runtimes = dir.join("runtimes.d");
            let real = dir.join("else").join("r2");
            let beside = dir.join("else").join("r");
            std::fs::create_dir_all(&runtimes).unwrap();
            std::fs::create_dir_all(&real).unwrap();
            std::fs::create_dir_all(&beside).unwrap();
            let manifest = "id = \"r\"\ncapabilities = [\"chat\"]\nexecution = \"stream\"\ndetect = { extension = \"gguf\" }\n[serve]\nentrypoint = \"main.py\"\n[env]\nlockfile = \"../r/requirements.lock\"\n";
            std::fs::write(real.join("manifest.toml"), manifest).unwrap();
            std::fs::write(real.join("main.py"), "one").unwrap();
            std::fs::write(real.join("requirements.lock"), "x==1").unwrap();
            std::fs::write(real.join("requirements.cpu.lock"), "x==1+cpu").unwrap();
            std::fs::write(beside.join("requirements.lock"), "x==1").unwrap();
            link(
                &real.join("requirements.cpu.lock"),
                &beside.join("requirements.cpu.lock"),
            )
            .unwrap();
            std::os::unix::fs::symlink(&real, runtimes.join("r")).unwrap();
            let hash = || {
                UserRuntimeStore::new(runtimes.clone())
                    .load(&HashSet::new())
                    .manifests[0]
                    .content_hash
                    .clone()
            };
            let before = hash();
            std::fs::remove_file(beside.join("requirements.cpu.lock")).unwrap();
            assert_ne!(hash(), before, "link kind {kind}");
            std::fs::remove_dir_all(&dir).ok();
        }
    }

    /// A variant reached through `..` past an empty directory is hashed, so
    /// swapping that directory for a symlink to where there is none asks for
    /// consent again.
    #[cfg(unix)]
    #[test]
    fn the_consent_hash_sees_an_empty_directory_become_a_symlink() {
        let dir = temp_dir("variant-empty-dir");
        let runtime = dir.join("r");
        std::fs::create_dir_all(runtime.join("deps")).unwrap();
        std::fs::create_dir_all(runtime.join(".outside/inner")).unwrap();
        let manifest = "id = \"r\"\ncapabilities = [\"chat\"]\nexecution = \"stream\"\ndetect = { extension = \"gguf\" }\n[serve]\nentrypoint = \"main.py\"\n[env]\nlockfile = \"deps/../requirements.lock\"\n";
        std::fs::write(runtime.join("manifest.toml"), manifest).unwrap();
        std::fs::write(runtime.join("main.py"), "one").unwrap();
        std::fs::write(runtime.join("requirements.lock"), "x==1").unwrap();
        std::fs::write(runtime.join("requirements.cpu.lock"), "x==1+cpu").unwrap();
        std::fs::write(runtime.join(".outside/requirements.lock"), "x==1").unwrap();
        let hash = || {
            UserRuntimeStore::new(dir.clone())
                .load(&HashSet::new())
                .manifests[0]
                .content_hash
                .clone()
        };
        let before = hash();
        std::fs::remove_dir(runtime.join("deps")).unwrap();
        std::os::unix::fs::symlink(".outside/inner", runtime.join("deps")).unwrap();
        assert_ne!(hash(), before);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The build takes a variant only as a regular file, and so does the hash:
    /// an empty variant turned into a link to `/dev/null`, which reads as
    /// empty too, asks for consent again.
    #[cfg(unix)]
    #[test]
    fn the_consent_hash_takes_a_variant_only_as_a_regular_file() {
        let dir = temp_dir("variant-not-a-file");
        let runtime = dir.join("r");
        std::fs::create_dir_all(runtime.join(".deps")).unwrap();
        let manifest = "id = \"r\"\ncapabilities = [\"chat\"]\nexecution = \"stream\"\ndetect = { extension = \"gguf\" }\n[serve]\nentrypoint = \"main.py\"\n[env]\nlockfile = \".deps/requirements.lock\"\n";
        std::fs::write(runtime.join("manifest.toml"), manifest).unwrap();
        std::fs::write(runtime.join("main.py"), "one").unwrap();
        std::fs::write(runtime.join(".deps/requirements.lock"), "x==1").unwrap();
        let variant = runtime.join(".deps/requirements.cpu.lock");
        std::fs::write(&variant, "").unwrap();
        let hash = || {
            UserRuntimeStore::new(dir.clone())
                .load(&HashSet::new())
                .manifests[0]
                .content_hash
                .clone()
        };
        let before = hash();
        std::fs::remove_file(&variant).unwrap();
        std::os::unix::fs::symlink("/dev/null", &variant).unwrap();
        assert_ne!(hash(), before);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A name that is not UTF-8 is hashed as its bytes, not as the text it
    /// would print as, so renaming a file to it asks for consent again. A
    /// disk that refuses such names has nothing to check.
    #[test]
    fn the_consent_hash_names_a_file_by_its_bytes() {
        use std::os::unix::ffi::OsStrExt;
        let dir = temp_dir("bytes-name");
        let runtime = dir.join("r");
        std::fs::create_dir_all(&runtime).unwrap();
        std::fs::write(runtime.join("manifest.toml"), INVOKE_MANIFEST).unwrap();
        std::fs::write(runtime.join("a\u{FFFD}.txt"), "same").unwrap();
        let hash = || {
            UserRuntimeStore::new(dir.clone())
                .load(&HashSet::new())
                .manifests[0]
                .content_hash
                .clone()
        };
        let before = hash();
        let bytes = runtime.join(std::ffi::OsStr::from_bytes(b"a\xff.txt"));
        if std::fs::rename(runtime.join("a\u{FFFD}.txt"), &bytes).is_err() {
            std::fs::remove_dir_all(&dir).ok();
            return;
        }
        assert_ne!(hash(), before);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A file named `a\b.py` is not `b.py` in a directory `a`, which the
    /// runtime could import: moving one to the other asks for consent again.
    #[cfg(unix)]
    #[test]
    fn the_consent_hash_names_a_file_with_a_backslash_as_it_is() {
        let dir = temp_dir("backslash-name");
        let runtime = dir.join("r");
        std::fs::create_dir_all(&runtime).unwrap();
        std::fs::write(runtime.join("manifest.toml"), INVOKE_MANIFEST).unwrap();
        std::fs::write(runtime.join("a\\b.py"), "print(1)").unwrap();
        let hash = || {
            UserRuntimeStore::new(dir.clone())
                .load(&HashSet::new())
                .manifests[0]
                .content_hash
                .clone()
        };
        let before = hash();
        std::fs::remove_file(runtime.join("a\\b.py")).unwrap();
        std::fs::create_dir_all(runtime.join("a")).unwrap();
        std::fs::write(runtime.join("a/b.py"), "print(1)").unwrap();
        assert_ne!(hash(), before);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A backslash is part of a name on Unix, so a variant linked first to
    /// `a/b`'s lock and then to `a\b`'s asks for consent again.
    #[cfg(unix)]
    #[test]
    fn the_consent_hash_tells_a_backslash_from_a_directory() {
        let dir = temp_dir("variant-backslash");
        let runtime = dir.join("r");
        std::fs::create_dir_all(runtime.join("a/b")).unwrap();
        std::fs::create_dir_all(runtime.join("a\\b")).unwrap();
        std::fs::create_dir_all(runtime.join(".outside/inner")).unwrap();
        std::fs::create_dir_all(runtime.join(".outside/a/b")).unwrap();
        let manifest = "id = \"r\"\ncapabilities = [\"chat\"]\nexecution = \"stream\"\ndetect = { extension = \"gguf\" }\n[serve]\nentrypoint = \"main.py\"\n[env]\nlockfile = \"deps/../a/b/requirements.lock\"\n";
        std::fs::write(runtime.join("manifest.toml"), manifest).unwrap();
        std::fs::write(runtime.join("main.py"), "one").unwrap();
        std::fs::write(runtime.join("a/b/requirements.cpu.lock"), "x==1+cpu").unwrap();
        std::fs::write(runtime.join("a\\b/requirements.cpu.lock"), "x==2+cpu").unwrap();
        std::fs::write(runtime.join(".outside/a/b/requirements.lock"), "x==1").unwrap();
        std::os::unix::fs::symlink(".outside/inner", runtime.join("deps")).unwrap();
        let variant = runtime.join(".outside/a/b/requirements.cpu.lock");
        std::fs::hard_link(runtime.join("a/b/requirements.cpu.lock"), &variant).unwrap();
        let hash = || {
            UserRuntimeStore::new(dir.clone())
                .load(&HashSet::new())
                .manifests[0]
                .content_hash
                .clone()
        };
        let before = hash();
        std::fs::remove_file(&variant).unwrap();
        std::fs::hard_link(runtime.join("a\\b/requirements.cpu.lock"), &variant).unwrap();
        assert_ne!(hash(), before);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A variant that is a symlink is hashed through it, so pointing it at
    /// another file beside it asks for consent again.
    #[cfg(unix)]
    #[test]
    fn the_consent_hash_follows_a_variant_that_is_a_symlink() {
        let dir = temp_dir("variant-link");
        let runtime = dir.join("r");
        std::fs::create_dir_all(&runtime).unwrap();
        let manifest = "id = \"r\"\ncapabilities = [\"chat\"]\nexecution = \"stream\"\ndetect = { extension = \"gguf\" }\n[serve]\nentrypoint = \"main.py\"\n[env]\nlockfile = \"requirements.lock\"\n";
        std::fs::write(runtime.join("manifest.toml"), manifest).unwrap();
        std::fs::write(runtime.join("main.py"), "one").unwrap();
        std::fs::write(runtime.join("requirements.lock"), "x==1").unwrap();
        std::fs::write(runtime.join("a.lock"), "x==1+cpu").unwrap();
        std::fs::write(runtime.join("b.lock"), "x==2+cpu").unwrap();
        let variant = runtime.join("requirements.cpu.lock");
        std::os::unix::fs::symlink("a.lock", &variant).unwrap();
        let hash = || {
            UserRuntimeStore::new(dir.clone())
                .load(&HashSet::new())
                .manifests[0]
                .content_hash
                .clone()
        };
        let before = hash();
        std::fs::remove_file(&variant).unwrap();
        std::os::unix::fs::symlink("b.lock", &variant).unwrap();
        assert_ne!(hash(), before);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_missing_directory_loads_empty() {
        let dir = std::env::temp_dir().join("hedos-runtimes-absent-does-not-exist");
        let _ = std::fs::remove_dir_all(&dir);
        let load = UserRuntimeStore::new(dir).load(&HashSet::new());
        assert!(load.manifests.is_empty());
        assert!(load.issues.is_empty());
    }

    #[test]
    fn the_catalog_ensures_its_directory_and_loads() {
        let dir = std::env::temp_dir().join(format!("hedos-catalog-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let catalog = RuntimeCatalog::new(dir.clone(), HashSet::new());
        assert!(catalog.ensured_directory().exists());
        std::fs::write(dir.join("one.toml"), INVOKE_MANIFEST).unwrap();
        assert_eq!(catalog.load().manifests.len(), 1);
        // Nothing was installed by the community provider.
        assert!(catalog.installed_community().is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }
}
