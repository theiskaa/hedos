//! The community runtime library and installer: reading bundled recipe
//! directories, previewing what a runtime would install, and installing or
//! removing a community runtime under `runtimes.d`. Community runtimes must run
//! contained (they declare a `[vm]` section); the contained runner itself is out
//! of scope for this build, but the install machinery is portable.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use kernel::manifests::{ManifestDetect, ManifestVm, RuntimeManifest, RuntimeProvenance};
use kernel::records::{ExecutionMode, ModelRecord};

use super::{ManifestError, detect_matches, slug};

/// A library of community runtime recipes, each a directory with a `manifest.toml`.
pub struct CommunityLibrary {
    directories: Vec<PathBuf>,
}

/// A loaded recipe: its manifest and the directory it lives in.
#[derive(Debug, Clone)]
pub struct Recipe {
    /// The recipe's manifest.
    pub manifest: RuntimeManifest,
    /// The recipe's directory.
    pub directory: PathBuf,
}

impl CommunityLibrary {
    /// A library over the given recipe directories.
    pub fn new(directories: Vec<PathBuf>) -> Self {
        Self { directories }
    }

    /// The recipes whose manifests load successfully.
    pub fn recipes(&self) -> Vec<Recipe> {
        self.directories
            .iter()
            .filter_map(|directory| {
                let (manifest, _) = load_manifest(directory).ok()?;
                Some(Recipe {
                    manifest,
                    directory: directory.clone(),
                })
            })
            .collect()
    }

    /// The recipes whose detect rule matches `record`.
    pub fn matches(&self, record: &ModelRecord) -> Vec<Recipe> {
        self.recipes()
            .into_iter()
            .filter(|recipe| {
                recipe
                    .manifest
                    .detect
                    .as_ref()
                    .is_some_and(|detect| detect_matches(detect, record))
            })
            .collect()
    }
}

/// A human-facing summary of what installing a community runtime entails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeInstallPreview {
    /// The runtime id.
    pub id: String,
    /// The capabilities it serves.
    pub capabilities: Vec<String>,
    /// Its execution mode.
    pub execution: String,
    /// The VM image it runs in.
    pub image: String,
    /// The VM setup commands.
    pub setup: Vec<String>,
    /// The paths it is granted.
    pub paths: Vec<String>,
    /// A plain-language description of what it detects.
    pub detect_summary: Option<String>,
    /// The approximate VM asset download size, if one is needed.
    pub vm_asset_download_mb: Option<i64>,
    /// Where the runtime is being installed from.
    pub source: PathBuf,
}

/// Installs and removes community runtimes under a `runtimes.d` directory.
pub struct ManifestInstaller {
    runtimes_directory: PathBuf,
    reserved_ids: HashSet<String>,
}

impl ManifestInstaller {
    /// An installer targeting `runtimes_directory`, reserving `reserved_ids`.
    pub fn new(runtimes_directory: PathBuf, reserved_ids: HashSet<String>) -> Self {
        Self {
            runtimes_directory,
            reserved_ids,
        }
    }

    /// Preview what installing the runtime at `source` would do.
    /// `vm_asset_download_mb` is the size of any VM image that would be fetched.
    pub fn preview(
        &self,
        source: &Path,
        vm_asset_download_mb: Option<i64>,
    ) -> Result<RuntimeInstallPreview, ManifestError> {
        let (manifest, _) = load_manifest(source)?;
        let vm = self.require_installable(&manifest)?;
        Ok(RuntimeInstallPreview {
            id: manifest.id.clone(),
            capabilities: manifest
                .capabilities
                .iter()
                .map(|capability| capability.as_str().to_owned())
                .collect(),
            execution: execution_str(manifest.execution).to_owned(),
            image: vm.image.clone(),
            setup: vm.setup.clone(),
            paths: manifest.permissions.paths.clone(),
            detect_summary: manifest.detect.as_ref().and_then(detect_summary),
            vm_asset_download_mb,
            source: source.to_path_buf(),
        })
    }

    /// Install the community runtime at `source` under `runtimes.d`, returning its
    /// id. Fails if it is not contained, its id is reserved, or one is already
    /// installed under that name.
    pub fn install(&self, source: &Path) -> Result<String, ManifestError> {
        let (manifest, manifest_path) = load_manifest(source)?;
        self.require_installable(&manifest)?;

        let destination = self.runtimes_directory.join(slug(&manifest.id));
        if destination.exists() {
            return Err(ManifestError::Failed(format!(
                "a runtime named {} is already installed",
                manifest.id
            )));
        }
        std::fs::create_dir_all(&self.runtimes_directory)
            .map_err(|error| ManifestError::Failed(error.to_string()))?;
        if manifest.directory.is_some() {
            copy_dir_recursive(source, &destination)
                .map_err(|error| ManifestError::Failed(error.to_string()))?;
        } else {
            std::fs::create_dir_all(&destination)
                .map_err(|error| ManifestError::Failed(error.to_string()))?;
            std::fs::copy(&manifest_path, destination.join("manifest.toml"))
                .map_err(|error| ManifestError::Failed(error.to_string()))?;
        }
        RuntimeProvenance::community()
            .write(&destination)
            .map_err(|error| ManifestError::Failed(error.to_string()))?;
        Ok(manifest.id)
    }

    /// Remove the community runtime `id`. Refuses to remove a runtime Hedos did not
    /// install (one without community provenance).
    pub fn uninstall(&self, id: &str) -> Result<(), ManifestError> {
        let destination = self.runtimes_directory.join(slug(id));
        if !destination.exists() {
            return Err(ManifestError::Failed(format!(
                "no installed runtime named {id}"
            )));
        }
        if !RuntimeProvenance::read(&destination)
            .is_some_and(|provenance| provenance.is_community())
        {
            return Err(ManifestError::Failed(format!(
                "{id} was not installed by Hedos — remove it by hand from runtimes.d"
            )));
        }
        std::fs::remove_dir_all(&destination)
            .map_err(|error| ManifestError::Failed(error.to_string()))
    }

    /// A contained, non-reserved manifest is installable; return its `[vm]` section.
    fn require_installable<'a>(
        &self,
        manifest: &'a RuntimeManifest,
    ) -> Result<&'a ManifestVm, ManifestError> {
        let Some(vm) = &manifest.vm else {
            return Err(ManifestError::Failed(format!(
                "community runtimes run contained — {} needs a [vm] section",
                manifest.id
            )));
        };
        if self.reserved_ids.contains(&manifest.id) {
            return Err(ManifestError::Failed(format!(
                "id \"{}\" is reserved",
                manifest.id
            )));
        }
        Ok(vm)
    }
}

/// Load the manifest at `source` (a directory's `manifest.toml`, or a bare file),
/// returning it and the manifest file's path.
fn load_manifest(source: &Path) -> Result<(RuntimeManifest, PathBuf), ManifestError> {
    let is_directory = source.is_dir();
    let manifest_path = if is_directory {
        source.join("manifest.toml")
    } else {
        source.to_path_buf()
    };
    if !manifest_path.is_file() {
        return Err(ManifestError::Failed(format!(
            "no manifest.toml at {}",
            source.display()
        )));
    }
    let text = std::fs::read_to_string(&manifest_path)
        .map_err(|error| ManifestError::Failed(error.to_string()))?;
    let directory = is_directory.then(|| source.to_path_buf());
    let manifest = RuntimeManifest::parse(&text, directory)
        .map_err(|error| ManifestError::Failed(error.to_string()))?;
    Ok((manifest, manifest_path))
}

/// A plain-language description of a detect rule.
fn detect_summary(detect: &ManifestDetect) -> Option<String> {
    if let Some(file) = &detect.file {
        return Some(match &detect.contains {
            Some(contains) => format!("models whose {file} mentions {contains}"),
            None => format!("models carrying {file}"),
        });
    }
    detect
        .file_extension
        .as_ref()
        .map(|extension| format!(".{extension} files"))
}

fn execution_str(execution: ExecutionMode) -> &'static str {
    match execution {
        ExecutionMode::Stream => "stream",
        ExecutionMode::Job => "job",
        ExecutionMode::Sync => "sync",
    }
}

/// Recursively copy the directory `src` to `dst`.
fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let target = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const COMMUNITY_IMAGE: &str = "ghcr.io/hedos/ubuntu@sha256:0123456789abcdef";
    const COMMUNITY_MANIFEST: &str = "id = \"cool-runtime\"\ncapabilities = [\"chat\"]\nexecution = \"sync\"\ndetect = { extension = \"gguf\" }\n[invoke]\ncommand = \"run --model {model}\"\n[vm]\nimage = \"ghcr.io/hedos/ubuntu@sha256:0123456789abcdef\"\nsetup = [\"apt install foo\"]\n";

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("hedos-community-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn it_installs_a_community_runtime_and_stamps_provenance() {
        let root = temp_dir("install");
        let source = root.join("recipe.toml");
        std::fs::write(&source, COMMUNITY_MANIFEST).unwrap();
        let runtimes = root.join("runtimes.d");
        let installer = ManifestInstaller::new(runtimes.clone(), HashSet::new());

        let id = installer.install(&source).unwrap();
        assert_eq!(id, "cool-runtime");
        let destination = runtimes.join("cool-runtime");
        assert!(destination.join("manifest.toml").is_file());
        assert!(
            RuntimeProvenance::read(&destination)
                .unwrap()
                .is_community()
        );

        // A second install under the same id is refused.
        assert!(installer.install(&source).is_err());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn it_rejects_a_non_contained_or_reserved_runtime() {
        let root = temp_dir("reject");
        let no_vm = root.join("no-vm.toml");
        std::fs::write(
            &no_vm,
            "id = \"x\"\ncapabilities = [\"chat\"]\nexecution = \"sync\"\ndetect = { extension = \"gguf\" }\n[invoke]\ncommand = \"y\"\n",
        )
        .unwrap();
        let installer = ManifestInstaller::new(root.join("runtimes.d"), HashSet::new());
        assert!(
            installer
                .install(&no_vm)
                .unwrap_err()
                .to_string()
                .contains("run contained")
        );

        let reserved = ManifestInstaller::new(
            root.join("runtimes.d"),
            HashSet::from(["cool-runtime".to_owned()]),
        );
        let source = root.join("recipe.toml");
        std::fs::write(&source, COMMUNITY_MANIFEST).unwrap();
        assert!(
            reserved
                .install(&source)
                .unwrap_err()
                .to_string()
                .contains("reserved")
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn it_uninstalls_only_community_installed_runtimes() {
        let root = temp_dir("uninstall");
        let source = root.join("recipe.toml");
        std::fs::write(&source, COMMUNITY_MANIFEST).unwrap();
        let runtimes = root.join("runtimes.d");
        let installer = ManifestInstaller::new(runtimes.clone(), HashSet::new());
        installer.install(&source).unwrap();
        assert!(installer.uninstall("cool-runtime").is_ok());
        assert!(!runtimes.join("cool-runtime").exists());

        // A hand-placed runtime (no community provenance) is refused.
        let manual = runtimes.join("manual");
        std::fs::create_dir_all(&manual).unwrap();
        std::fs::write(manual.join("manifest.toml"), COMMUNITY_MANIFEST).unwrap();
        assert!(
            installer
                .uninstall("manual")
                .unwrap_err()
                .to_string()
                .contains("not installed by Hedos")
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn preview_summarizes_the_runtime() {
        let root = temp_dir("preview");
        let source = root.join("recipe.toml");
        std::fs::write(&source, COMMUNITY_MANIFEST).unwrap();
        let installer = ManifestInstaller::new(root.join("runtimes.d"), HashSet::new());
        let preview = installer.preview(&source, Some(2048)).unwrap();
        assert_eq!(preview.id, "cool-runtime");
        assert_eq!(preview.image, COMMUNITY_IMAGE);
        assert_eq!(preview.capabilities, vec!["chat".to_owned()]);
        assert_eq!(preview.execution, "sync");
        assert_eq!(preview.detect_summary.as_deref(), Some(".gguf files"));
        assert_eq!(preview.vm_asset_download_mb, Some(2048));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_library_loads_and_matches_recipes() {
        let root = temp_dir("library");
        let recipe = root.join("cool");
        std::fs::create_dir_all(&recipe).unwrap();
        std::fs::write(recipe.join("manifest.toml"), COMMUNITY_MANIFEST).unwrap();
        let library = CommunityLibrary::new(vec![recipe.clone()]);
        assert_eq!(library.recipes().len(), 1);

        use kernel::records::{Modality, ModelSource, SourceKind};
        let mut record = ModelRecord::new(
            "m",
            Modality::text(),
            Vec::new(),
            ModelSource::new(SourceKind::folder(), "/models/m.gguf"),
        );
        record.primary_weight_path = Some("/models/m.gguf".to_owned());
        assert_eq!(library.matches(&record).len(), 1);
        std::fs::remove_dir_all(&root).ok();
    }
}
