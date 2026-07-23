//! [`ModelHabitat`]: where models live on this machine. It computes the store
//! roots (from the environment, the home directory, and user settings) and
//! assembles the [`StoreScanner`]s that sweep them. This is the single place the
//! four scanners are wired together and pointed at their default locations.
//!
//! The Apple-Foundation (builtin) scanner is not assembled here: its
//! availability probe lives in the runtime crate's backend bridge, so callers
//! append it to this list (see the runtime's `apple_foundation_scanner`).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::discovery::hf_scanner::HFCacheScanner;
use crate::discovery::lm_studio_scanner::LMStudioScanner;
use crate::discovery::loose_file_scanner::LooseFileScanner;
use crate::discovery::ollama_scanner::OllamaStoreScanner;
use crate::discovery::scanner::StoreScanner;
use crate::fs::expand_tilde;
use crate::records::SourceKind;

/// The discovery-relevant model settings: user-added folders to watch and extra
/// Hugging Face cache roots. (The full settings domain is not yet ported.)
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelsSettings {
    /// Extra directories the user asked to scan for loose files.
    pub watched_folders: Vec<String>,
    /// Extra Hugging Face cache roots the user configured.
    pub hf_cache_roots: Vec<String>,
}

/// The machine's model locations, resolved from `home` and `environment`.
#[derive(Debug, Clone)]
pub struct ModelHabitat {
    home: PathBuf,
    environment: HashMap<String, String>,
}

impl ModelHabitat {
    /// A habitat rooted at `home` with the given `environment`.
    pub fn new(home: impl Into<PathBuf>, environment: HashMap<String, String>) -> Self {
        Self {
            home: home.into(),
            environment,
        }
    }

    /// A habitat detected from the process: `$HOME` and the current environment.
    pub fn detect() -> Self {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        let environment = std::env::vars().collect();
        Self { home, environment }
    }

    /// Every `(kind, root)` this habitat would scan, in order.
    pub fn roots(&self, settings: &ModelsSettings) -> Vec<(SourceKind, PathBuf)> {
        let mut roots = vec![(SourceKind::ollama(), self.ollama_root())];
        for url in self.hf_default_roots(&settings.hf_cache_roots) {
            roots.push((SourceKind::huggingface_cache(), url));
        }
        for url in lm_studio_roots(&self.home) {
            roots.push((SourceKind::lm_studio(), url));
        }
        for url in loose_directories(&self.home) {
            roots.push((SourceKind::file(), url));
        }
        for path in &settings.watched_folders {
            roots.push((SourceKind::file(), PathBuf::from(path)));
        }
        roots
    }

    /// The scanners to run. If `kinds` is given, only scanners producing at least
    /// one of those kinds are included; `None` includes all.
    pub fn scanners(
        &self,
        kinds: Option<&[SourceKind]>,
        settings: &ModelsSettings,
    ) -> Vec<Box<dyn StoreScanner>> {
        let wanted = |produced: &[SourceKind]| match kinds {
            None => true,
            Some(kinds) => produced.iter().any(|kind| kinds.contains(kind)),
        };
        let mut scanners: Vec<Box<dyn StoreScanner>> = Vec::new();
        if wanted(&[SourceKind::ollama()]) {
            scanners.push(Box::new(OllamaStoreScanner::new(self.ollama_root())));
        }
        if wanted(&[SourceKind::huggingface_cache()]) {
            // The user roots go ONLY through `user_roots` (scanned as required);
            // the default roots must exclude them (empty user list) so the scanner
            // doesn't sweep a user root twice. `roots()` above intentionally does
            // include them, since it enumerates the full set.
            scanners.push(Box::new(HFCacheScanner::with_user_roots(
                self.hf_default_roots(&[]),
                self.hf_user_roots(&settings.hf_cache_roots),
            )));
        }
        if wanted(&[SourceKind::lm_studio()]) {
            scanners.push(Box::new(LMStudioScanner::new(lm_studio_roots(&self.home))));
        }
        if wanted(&[SourceKind::file(), SourceKind::folder()]) {
            let watched = settings.watched_folders.iter().map(PathBuf::from).collect();
            scanners.push(Box::new(LooseFileScanner::with_user_directories(
                loose_directories(&self.home),
                watched,
            )));
        }
        scanners
    }

    fn ollama_root(&self) -> PathBuf {
        match self.environment.get("OLLAMA_MODELS") {
            Some(custom) if !custom.is_empty() => expand_tilde(custom, &self.home),
            _ => self.home.join(".ollama/models"),
        }
    }

    /// The standard Hugging Face roots (env + the default cache) plus the user's
    /// configured roots, de-duplicated in order.
    fn hf_default_roots(&self, user: &[String]) -> Vec<PathBuf> {
        let mut candidates = Vec::new();
        if let Some(cache) = self.environment.get("HF_HUB_CACHE")
            && !cache.is_empty()
        {
            candidates.push(expand_tilde(cache, &self.home));
        }
        if let Some(hf_home) = self.environment.get("HF_HOME")
            && !hf_home.is_empty()
        {
            candidates.push(expand_tilde(hf_home, &self.home).join("hub"));
        }
        candidates.push(self.home.join(".cache/huggingface/hub"));
        candidates.extend(self.hf_user_roots(user));
        dedup(candidates)
    }

    /// For each user path, the hub subdirectories that exist (`hub`,
    /// `huggingface/hub`, or the path itself), falling back to the bare path.
    fn hf_user_roots(&self, paths: &[String]) -> Vec<PathBuf> {
        let mut roots = Vec::new();
        for path in paths {
            let base = expand_tilde(path, &self.home);
            let candidates = [base.join("hub"), base.join("huggingface/hub"), base.clone()];
            let existing: Vec<PathBuf> = candidates
                .into_iter()
                .filter(|url| is_hub_directory(url))
                .collect();
            if existing.is_empty() {
                roots.push(base);
            } else {
                roots.extend(existing);
            }
        }
        dedup(roots)
    }
}

fn lm_studio_roots(home: &Path) -> Vec<PathBuf> {
    vec![
        home.join(".lmstudio/models"),
        home.join(".cache/lm-studio/models"),
    ]
}

fn loose_directories(home: &Path) -> Vec<PathBuf> {
    vec![home.join("Downloads"), home.join("Models")]
}

fn is_hub_directory(url: &Path) -> bool {
    url.is_dir()
}

/// De-duplicate paths, preserving first-seen order.
fn dedup(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    paths
        .into_iter()
        .filter(|path| seen.insert(path.clone()))
        .collect()
}
