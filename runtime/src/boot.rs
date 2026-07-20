//! The composition root: assemble a production [`Kernel`] — registry, artifacts,
//! governor, job history, and the full built-in adapter set — plus the default
//! install service and the discovery-settings bridge, so a front end (the CLI,
//! and later the TUI) stays a thin shell. State lives under a per-user data dir;
//! configuration comes from the shared `hedos.toml`.

use std::path::PathBuf;
use std::sync::Arc;

use kernel::artifacts::ArtifactStore;
use kernel::jobs::JobHistoryStore;
use kernel::registry::{Registry, RegistryError};

use crate::adapters::{
    A1111Adapter, ComfyUiAdapter, DaemonLiveness, DiffusersAdapter, EmbeddingsAdapter,
    LlamaServerAdapter, LlamaServerPool, LlamaServerSpawner, MfluxAdapter, MissingWhisperBackend,
    MlxAudioAdapter, MlxLmAdapter, MlxVlmAdapter, OllamaAdapter, OpenAiEndpointAdapter,
    WhisperCppAdapter, WhisperEngine,
};
use crate::environment::EnvironmentManager;
use crate::facade::{Kernel, RegisteredAdapter};
use crate::governor::{GovernorConfig, MemoryGovernor, ResidencyPolicy};
use crate::install::{
    HFHubAPI, HuggingFaceInstallProvider, InstallProvider, InstallService, InstallTransport,
    OllamaInstallProvider, ReqwestTransport,
};
use crate::settings::Settings;
use crate::sidecar::SidecarSupervisor;

/// The per-user directories Hedos reads and writes.
pub struct HedosDirs {
    /// State root: registry, artifacts, job history, sidecar workdirs/envs, and
    /// the community `runtimes.d`.
    pub data: PathBuf,
}

impl HedosDirs {
    /// Detect the data dir: `$XDG_DATA_HOME/hedos`, else `~/.local/share/hedos`,
    /// else a `hedos-data` dir in the current directory.
    pub fn detect() -> Self {
        let base = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share"))
            });
        let data = base.map_or_else(|| PathBuf::from("hedos-data"), |base| base.join("hedos"));
        Self { data }
    }

    /// A named subdirectory of the data dir.
    pub fn sub(&self, name: &str) -> PathBuf {
        self.data.join(name)
    }
}

/// A failure assembling the kernel.
#[derive(Debug, thiserror::Error)]
pub enum BootError {
    /// Creating a required directory failed.
    #[error("creating {path}: {source}")]
    Io {
        /// The directory that could not be created.
        path: String,
        /// The underlying error.
        source: std::io::Error,
    },
    /// Opening the model registry failed.
    #[error(transparent)]
    Registry(#[from] RegistryError),
}

/// Assemble a production [`Kernel`] rooted at `dirs` and configured by `settings`
/// — the registry/artifacts/history stores, a memory governor detecting the
/// machine's RAM, and the full built-in adapter set.
pub fn build_kernel(dirs: &HedosDirs, settings: &Settings) -> Result<Kernel, BootError> {
    let registry = Registry::open(&make_dir(dirs.sub("registry"))?)?;
    let artifacts = ArtifactStore::new(&make_dir(dirs.sub("artifacts"))?);
    let history = JobHistoryStore::new(
        &make_dir(dirs.sub("history"))?,
        settings.advanced.job_history_limit.max(0) as usize,
    );

    let governor = MemoryGovernor::new(GovernorConfig::detect());
    governor.apply(&ResidencyPolicy {
        keep_warm: settings.models.keep_warm,
        eviction: settings.models.eviction,
        ram_budget_mb: settings.models.ram_budget_mb(),
    });

    // Extract the shipped Python runtime bundles into the data dir so the sidecar
    // adapters can find them. Best-effort: a failure just leaves a bundle absent,
    // which the runtime reports on use, and never blocks the non-Python commands.
    let _ = crate::sidecar::provision_bundles(&dirs.sub("bundles"));

    let adapters = default_adapters(&governor, dirs);
    let kernel = Kernel::new(registry, artifacts, Arc::new(governor), history, adapters);
    kernel.set_default_system_prompt(settings.chat.default_system_prompt().map(str::to_owned));
    Ok(kernel)
}

/// The default install service: the Ollama and Hugging Face providers.
pub fn default_install_service() -> InstallService {
    let home = home_dir();
    let transport: Arc<dyn InstallTransport> = Arc::new(ReqwestTransport::new());
    let api = HFHubAPI::new(Arc::clone(&transport));
    let hugging_face = HuggingFaceInstallProvider::new(api, transport, hf_cache_root(&home), home);
    let providers: Vec<Arc<dyn InstallProvider>> = vec![
        Arc::new(OllamaInstallProvider::new()),
        Arc::new(hugging_face),
    ];
    InstallService::new(providers)
}

/// Translate the runtime settings' discovery inputs into the kernel's discovery
/// `ModelsSettings` (the bridge the two crates need — the structs differ).
pub fn discovery_settings(settings: &Settings) -> kernel::discovery::ModelsSettings {
    kernel::discovery::ModelsSettings {
        watched_folders: settings.models.watched_folders.clone(),
        hf_cache_roots: settings.models.hf_cache_roots.clone(),
    }
}

/// The built-in adapter set — the port of the Swift `defaultAdapters`. Each
/// adapter is present regardless of whether its backend (a `llama-server`
/// binary, a Python sidecar, the Ollama daemon, an API key) is installed; a
/// capability only actually serves when its backend is available. The two
/// framework-bound adapters (MLX-Swift, Apple Foundation) are intentionally out.
fn default_adapters(governor: &MemoryGovernor, dirs: &HedosDirs) -> Vec<RegisteredAdapter> {
    let bundles = vec![dirs.sub("bundles")];
    let workdirs = dirs.sub("workdirs");
    let env_root = dirs.sub("env");
    // The five arguments every Python-sidecar adapter's `new` takes.
    let args = |name: &str| {
        (
            governor.clone(),
            SidecarSupervisor::new(),
            EnvironmentManager::new(env_root.join(name)),
            bundles.clone(),
            workdirs.clone(),
        )
    };

    let mut adapters: Vec<RegisteredAdapter> = Vec::new();

    // Streaming adapters.
    adapters.push(RegisteredAdapter::streaming(Arc::new(
        LlamaServerAdapter::new(Arc::new(LlamaServerPool::new(Arc::new(
            LlamaServerSpawner::new("llama-server"),
        )))),
    )));
    adapters.push(RegisteredAdapter::streaming(Arc::new(
        WhisperCppAdapter::new(
            governor.clone(),
            WhisperEngine::new(Arc::new(MissingWhisperBackend)),
        ),
    )));
    adapters.push(RegisteredAdapter::streaming(Arc::new(OllamaAdapter::new())));
    let (g, s, e, b, w) = args("mlx-audio");
    adapters.push(RegisteredAdapter::streaming(Arc::new(
        MlxAudioAdapter::new(g, s, e, b, w),
    )));
    let (g, s, e, b, w) = args("mlx-lm");
    adapters.push(RegisteredAdapter::streaming(Arc::new(MlxLmAdapter::new(
        g, s, e, b, w,
    ))));
    let (g, s, e, b, w) = args("mlx-vlm");
    adapters.push(RegisteredAdapter::streaming(Arc::new(MlxVlmAdapter::new(
        g, s, e, b, w,
    ))));
    let (g, s, e, b, w) = args("embeddings");
    adapters.push(RegisteredAdapter::streaming(Arc::new(
        EmbeddingsAdapter::new(g, s, e, b, w),
    )));
    adapters.push(RegisteredAdapter::streaming(Arc::new(
        OpenAiEndpointAdapter::new(),
    )));

    // Image generation runs through the job path.
    let (g, s, e, b, w) = args("mflux");
    let mflux = Arc::new(MfluxAdapter::new(g, s, e, b, w));
    adapters.push(RegisteredAdapter::with_jobs(mflux.clone(), mflux));
    let (g, s, e, b, w) = args("diffusers");
    let diffusers = Arc::new(DiffusersAdapter::new(g, s, e, b, w));
    adapters.push(RegisteredAdapter::with_jobs(diffusers.clone(), diffusers));
    let comfy = Arc::new(ComfyUiAdapter::new(DaemonLiveness::with_defaults()));
    adapters.push(RegisteredAdapter::with_jobs(comfy.clone(), comfy));
    let a1111 = Arc::new(A1111Adapter::new(DaemonLiveness::with_defaults()));
    adapters.push(RegisteredAdapter::with_jobs(a1111.clone(), a1111));

    // Community manifest runtimes (`runtimes.d`) are loaded in a follow-up; the
    // default set is empty, so the built-in adapters are the whole shelf.
    adapters
}

/// Create `path` (and parents), mapping failure to [`BootError::Io`].
fn make_dir(path: PathBuf) -> Result<PathBuf, BootError> {
    std::fs::create_dir_all(&path).map_err(|source| BootError::Io {
        path: path.display().to_string(),
        source,
    })?;
    Ok(path)
}

/// The user's home directory, or the current directory if `$HOME` is unset.
fn home_dir() -> PathBuf {
    std::env::var_os("HOME").map_or_else(|| PathBuf::from("."), PathBuf::from)
}

/// The Hugging Face hub cache root: `$HF_HUB_CACHE`, else `$HF_HOME/hub`, else
/// `~/.cache/huggingface/hub`.
fn hf_cache_root(home: &std::path::Path) -> PathBuf {
    if let Some(cache) = std::env::var_os("HF_HUB_CACHE").filter(|value| !value.is_empty()) {
        return PathBuf::from(cache);
    }
    if let Some(hf_home) = std::env::var_os("HF_HOME").filter(|value| !value.is_empty()) {
        return PathBuf::from(hf_home).join("hub");
    }
    home.join(".cache/huggingface/hub")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dirs() -> HedosDirs {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0);
        HedosDirs {
            data: std::env::temp_dir().join(format!("hedos-boot-{}-{unique}", std::process::id())),
        }
    }

    #[tokio::test]
    async fn build_kernel_wires_the_builtin_adapters_and_serves_an_empty_shelf() {
        let dirs = temp_dirs();
        let kernel = build_kernel(&dirs, &Settings::default()).expect("build kernel");
        // A fresh data dir has no models, but the kernel is fully wired.
        assert!(kernel.shelf().await.is_empty());
        // An unknown model resolves to not-found (proof the facade is live).
        assert!(kernel.voices("nope").await.is_err());
        let _ = std::fs::remove_dir_all(&dirs.data);
    }

    #[test]
    fn the_discovery_bridge_carries_the_discovery_inputs() {
        let mut settings = Settings::default();
        settings.models.watched_folders = vec!["/models".to_owned()];
        settings.models.hf_cache_roots = vec!["~/hf".to_owned()];
        let bridged = discovery_settings(&settings);
        assert_eq!(bridged.watched_folders, ["/models"]);
        assert_eq!(bridged.hf_cache_roots, ["~/hf"]);
    }

    #[test]
    fn the_default_install_service_offers_both_providers() {
        let service = default_install_service();
        // Constructed without panicking; both providers are present.
        let _ = service;
    }
}
