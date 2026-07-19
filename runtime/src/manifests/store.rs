//! Loading user-installed runtime manifests from a `runtimes.d` directory, and
//! the catalog that wraps it. Each entry is either a bare `*.toml` (invoke-only
//! runtimes) or a directory holding `manifest.toml` beside its files; loading
//! validates ids, provenance, and structure, computing a consent hash over each
//! runtime's files so a later change is detectable.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use kernel::manifests::{RuntimeManifest, RuntimeProvenance};
use sha2::{Digest, Sha256};

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
            let Some((manifest_path, manifest_dir, label)) = entry_target(&entry) else {
                continue;
            };
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
            if let Some(directory) = &manifest_dir {
                manifest.provenance = RuntimeProvenance::read(directory);
            }

            let is_community = manifest
                .provenance
                .as_ref()
                .is_some_and(RuntimeProvenance::is_community);
            if is_community && manifest.vm.is_none() {
                load.issues.push(format!(
                    "{label}: community runtimes run contained — \"{}\" has no [vm] section",
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
                    "{label}: duplicate id \"{}\" — keeping the first",
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
            manifest.content_hash = Some(consent_hash(&text, manifest_dir.as_deref(), &manifest));
            seen.insert(manifest.id.clone());
            load.manifests.push(manifest);
        }
        load
    }
}

/// Resolve a `runtimes.d` entry to the manifest file to read, its directory (if
/// the entry is a runtime folder), and a display label — or `None` if the entry
/// is neither a `manifest.toml` folder nor a bare `*.toml`.
fn entry_target(entry: &Path) -> Option<(PathBuf, Option<PathBuf>, String)> {
    let name = file_name(entry);
    if entry.is_dir() {
        let manifest_path = entry.join("manifest.toml");
        if !manifest_path.is_file() {
            return None;
        }
        let label = format!("runtimes.d/{name}/manifest.toml");
        Some((manifest_path, Some(entry.to_path_buf()), label))
    } else {
        let is_toml = entry
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("toml"));
        if !is_toml {
            return None;
        }
        Some((entry.to_path_buf(), None, format!("runtimes.d/{name}")))
    }
}

/// A hash over everything a runtime consists of, so a later edit invalidates any
/// prior trust: the manifest text, the declared entrypoint/lockfile, and every
/// other file beside it. Without a directory it is just the manifest text hash.
fn consent_hash(text: &str, directory: Option<&Path>, manifest: &RuntimeManifest) -> String {
    let Some(directory) = directory else {
        let mut hasher = Sha256::new();
        hasher.update(text.as_bytes());
        return hex::encode(hasher.finalize());
    };

    let mut hasher = Sha256::new();
    absorb(&mut hasher, "manifest.toml", Some(text.as_bytes()));
    if let Some(serve) = &manifest.serve {
        let content = std::fs::read(directory.join(&serve.entrypoint)).ok();
        absorb(&mut hasher, &serve.entrypoint, content.as_deref());
    }
    if let Some(env) = &manifest.env {
        let content = std::fs::read(directory.join(&env.lockfile)).ok();
        absorb(&mut hasher, &env.lockfile, content.as_deref());
    }

    let mut files: Vec<(String, PathBuf)> = Vec::new();
    collect_files(directory, directory, &mut files);
    files.retain(|(relative, _)| relative != "manifest.toml");
    files.sort_by(|a, b| a.0.cmp(&b.0));
    for (relative, path) in files {
        let content = std::fs::read(&path).ok();
        absorb(&mut hasher, &relative, content.as_deref());
    }
    hex::encode(hasher.finalize())
}

/// Fold one `(path, content)` pair into the consent hash: the path bytes, a NUL
/// separator, the content length (or `u64::MAX` when the file is absent), then
/// the content.
fn absorb(hasher: &mut Sha256, path: &str, content: Option<&[u8]>) {
    hasher.update(path.as_bytes());
    hasher.update([0u8]);
    let count = content.map_or(u64::MAX, |bytes| bytes.len() as u64);
    hasher.update(count.to_le_bytes());
    if let Some(content) = content {
        hasher.update(content);
    }
}

/// Recursively collect the regular files under `base` (skipping hidden entries
/// and symlinks — a symlinked directory is not descended and a symlinked file is
/// not hashed) as `(path-relative-to-base, absolute-path)` pairs.
fn collect_files(base: &Path, current: &Path, out: &mut Vec<(String, PathBuf)>) {
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
            out.push((relative.to_string_lossy().replace('\\', "/"), path));
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
                .any(|issue| issue.contains("duplicate id"))
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
