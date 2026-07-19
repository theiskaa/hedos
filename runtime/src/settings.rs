//! Persistent settings, stored as one human-editable TOML file at
//! `<config-dir>/hedos.toml` (config dir = `XDG_CONFIG_HOME`, else `~/.config`,
//! else `%APPDATA%`). A single file with `[models]`/`[chat]`/`[voice]`/
//! `[gateway]`/`[advanced]` tables — not a per-domain store.
//!
//! Loading is tolerant: a missing file, a malformed file, or a malformed table
//! falls back to defaults for just that scope, so a hand-edit typo never wipes
//! every setting.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::governor::{EvictionPolicy, KeepWarmPolicy};

/// A per-process counter so concurrent `save`s use distinct temp files.
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A settings read/write failure.
#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    /// Reading or writing the file failed.
    #[error("accessing settings at {path}: {source}")]
    Io {
        /// The file involved.
        path: String,
        /// The underlying error.
        source: std::io::Error,
    },
    /// Encoding the settings to TOML failed.
    #[error("encoding settings: {0}")]
    Encode(String),
}

/// Model discovery + residency settings.
#[derive(Debug, Clone, PartialEq, Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ModelsSettings {
    /// Extra folders scanned for local models.
    pub watched_folders: Vec<String>,
    /// Hugging Face hub-cache roots to scan (beyond the default).
    pub hf_cache_roots: Vec<String>,
    /// Runtime ids approved to reach the network.
    pub approved_network_runtimes: Vec<String>,
    /// Approved network runtimes' content hashes, for re-approval on change.
    pub approved_network_runtime_hashes: BTreeMap<String, String>,
    /// Runtime ids approved to run on the host.
    pub approved_host_runtimes: Vec<String>,
    /// Approved host runtimes' content hashes.
    pub approved_host_runtime_hashes: BTreeMap<String, String>,
    /// How long to keep an idle model warm.
    pub keep_warm: KeepWarmPolicy,
    /// How the governor makes room.
    pub eviction: EvictionPolicy,
    /// An explicit RAM budget in MB for the budgeted policy (`0` = unset).
    pub ram_budget_mb: i64,
}

impl Default for ModelsSettings {
    fn default() -> Self {
        Self {
            watched_folders: Vec::new(),
            hf_cache_roots: Vec::new(),
            approved_network_runtimes: Vec::new(),
            approved_network_runtime_hashes: BTreeMap::new(),
            approved_host_runtimes: Vec::new(),
            approved_host_runtime_hashes: BTreeMap::new(),
            keep_warm: KeepWarmPolicy::FiveMinutes,
            eviction: EvictionPolicy::StrictSingle,
            ram_budget_mb: 0,
        }
    }
}

impl ModelsSettings {
    /// The RAM budget as an optional value (`None` when unset / non-positive).
    pub fn ram_budget_mb(&self) -> Option<i64> {
        (self.ram_budget_mb > 0).then_some(self.ram_budget_mb)
    }
}

/// Chat surface settings.
#[derive(Debug, Clone, PartialEq, Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ChatSettings {
    /// The default model id for a new chat (`""` = none).
    pub default_model_id: String,
    /// A default system prompt (`""` = none).
    pub default_system_prompt: String,
    /// Whether to show generation stats.
    pub show_stats: bool,
    /// Whether Enter sends (vs newline).
    pub send_with_enter: bool,
    /// The default bench (model ids kept on hand as tools).
    pub default_bench: Vec<String>,
}

impl Default for ChatSettings {
    fn default() -> Self {
        Self {
            default_model_id: String::new(),
            default_system_prompt: String::new(),
            show_stats: true,
            send_with_enter: true,
            default_bench: Vec::new(),
        }
    }
}

impl ChatSettings {
    /// The default model id, or `None` when unset.
    pub fn default_model_id(&self) -> Option<&str> {
        non_empty(&self.default_model_id)
    }

    /// The default system prompt, or `None` when unset.
    pub fn default_system_prompt(&self) -> Option<&str> {
        non_empty(&self.default_system_prompt)
    }
}

/// Voice (speech) settings.
#[derive(Debug, Clone, PartialEq, Serialize, serde::Deserialize)]
#[serde(default)]
pub struct VoiceSettings {
    /// The default voice id (`""` = none).
    pub default_voice: String,
    /// Playback speed multiplier.
    pub speed: f64,
    /// Whether to speak replies automatically.
    pub auto_speak: bool,
}

impl Default for VoiceSettings {
    fn default() -> Self {
        Self {
            default_voice: String::new(),
            speed: 1.0,
            auto_speak: false,
        }
    }
}

impl VoiceSettings {
    /// The default voice, or `None` when unset.
    pub fn default_voice(&self) -> Option<&str> {
        non_empty(&self.default_voice)
    }
}

/// Loopback gateway settings.
#[derive(Debug, Clone, PartialEq, Serialize, serde::Deserialize)]
#[serde(default)]
pub struct GatewaySettings {
    /// Whether the gateway is enabled.
    pub enabled: bool,
    /// The loopback address to bind.
    pub host: String,
    /// The port to bind (default avoids colliding with Ollama's 11434).
    pub port: u16,
    /// The maximum concurrent connections.
    pub max_connections: i64,
    /// The maximum concurrent inference jobs.
    pub max_concurrent_inference: i64,
}

impl Default for GatewaySettings {
    fn default() -> Self {
        Self {
            enabled: false,
            host: "127.0.0.1".to_owned(),
            port: 43367,
            max_connections: 128,
            max_concurrent_inference: 4,
        }
    }
}

/// Advanced settings.
#[derive(Debug, Clone, PartialEq, Serialize, serde::Deserialize)]
#[serde(default)]
pub struct AdvancedSettings {
    /// How many completed jobs to retain in history.
    pub job_history_limit: i64,
}

impl Default for AdvancedSettings {
    fn default() -> Self {
        Self {
            job_history_limit: 50,
        }
    }
}

/// The whole settings document — one per `hedos.toml`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Model discovery + residency.
    pub models: ModelsSettings,
    /// Chat surface.
    pub chat: ChatSettings,
    /// Voice.
    pub voice: VoiceSettings,
    /// Loopback gateway.
    pub gateway: GatewaySettings,
    /// Advanced.
    pub advanced: AdvancedSettings,
}

/// Reads and writes the settings file.
pub struct SettingsStore {
    path: PathBuf,
}

impl SettingsStore {
    /// A store backed by an explicit file path.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// A store at the discovered `<config-dir>/hedos.toml`.
    pub fn discover() -> Self {
        Self::new(config_file_path(&process_env()))
    }

    /// A store whose path is resolved from an explicit environment (for tests).
    pub fn with_env(env: &HashMap<String, String>) -> Self {
        Self::new(config_file_path(env))
    }

    /// The settings file path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Load the settings, falling back to defaults for a missing/malformed file
    /// or any table that fails to decode. Note the tradeoff: a typo that makes one
    /// table undecodable defaults that whole table, so a subsequent `save` persists
    /// those defaults, dropping the other keys in that table — but never the others.
    pub fn load(&self) -> Settings {
        let Ok(text) = fs::read_to_string(&self.path) else {
            return Settings::default();
        };
        let Ok(table) = text.parse::<toml::Table>() else {
            return Settings::default();
        };
        Settings {
            models: decode_table(&table, "models"),
            chat: decode_table(&table, "chat"),
            voice: decode_table(&table, "voice"),
            gateway: decode_table(&table, "gateway"),
            advanced: decode_table(&table, "advanced"),
        }
    }

    /// Write the settings via a uniquely-named temp file + rename, creating the
    /// directory. The unique temp name lets concurrent writers (TUI + CLI +
    /// gateway) not clobber each other's in-progress file.
    pub fn save(&self, settings: &Settings) -> Result<(), SettingsError> {
        let toml = toml::to_string_pretty(settings)
            .map_err(|error| SettingsError::Encode(error.to_string()))?;
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|source| self.io_error(source))?;
        }
        let unique = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let temp = self
            .path
            .with_extension(format!("toml.{}.{unique}.tmp", std::process::id()));
        fs::write(&temp, toml).map_err(|source| self.io_error(source))?;
        if let Err(source) = fs::rename(&temp, &self.path) {
            let _ = fs::remove_file(&temp);
            return Err(self.io_error(source));
        }
        Ok(())
    }

    /// Load, mutate, and save — returning the updated settings.
    pub fn update(&self, mutate: impl FnOnce(&mut Settings)) -> Result<Settings, SettingsError> {
        let mut settings = self.load();
        mutate(&mut settings);
        self.save(&settings)?;
        Ok(settings)
    }

    /// Add a watched model folder (tilde-expanded, deduplicated).
    pub fn add_watched_folder(&self, path: &str) -> Result<Settings, SettingsError> {
        let folder = expand_tilde(path);
        self.update(|settings| push_unique(&mut settings.models.watched_folders, folder))
    }

    /// Remove a watched folder (matching the tilde-expanded form).
    pub fn remove_watched_folder(&self, path: &str) -> Result<Settings, SettingsError> {
        let folder = expand_tilde(path);
        self.update(|settings| settings.models.watched_folders.retain(|f| f != &folder))
    }

    /// Add a Hugging Face cache root (tilde-expanded, deduplicated).
    pub fn add_hf_cache_root(&self, path: &str) -> Result<Settings, SettingsError> {
        let root = expand_tilde(path);
        self.update(|settings| push_unique(&mut settings.models.hf_cache_roots, root))
    }

    /// Remove a Hugging Face cache root.
    pub fn remove_hf_cache_root(&self, path: &str) -> Result<Settings, SettingsError> {
        let root = expand_tilde(path);
        self.update(|settings| settings.models.hf_cache_roots.retain(|r| r != &root))
    }

    /// Set (or clear, with `None`) the default chat model id.
    pub fn set_default_chat_model(
        &self,
        model_id: Option<&str>,
    ) -> Result<Settings, SettingsError> {
        let value = model_id.unwrap_or("").to_owned();
        self.update(|settings| settings.chat.default_model_id = value)
    }

    /// Approve a runtime to run on the host, recording its content hash so a
    /// change forces re-approval. `network` sets the network approval definitively:
    /// `false` revokes any prior network approval (a host-only downgrade).
    pub fn approve_runtime(
        &self,
        id: &str,
        content_hash: Option<&str>,
        network: bool,
    ) -> Result<Settings, SettingsError> {
        let id = id.to_owned();
        let hash = content_hash.map(str::to_owned);
        self.update(|settings| {
            let models = &mut settings.models;
            push_unique(&mut models.approved_host_runtimes, id.clone());
            if let Some(hash) = &hash {
                models
                    .approved_host_runtime_hashes
                    .insert(id.clone(), hash.clone());
            }
            if network {
                push_unique(&mut models.approved_network_runtimes, id.clone());
                if let Some(hash) = &hash {
                    models
                        .approved_network_runtime_hashes
                        .insert(id, hash.clone());
                }
            } else {
                // Host-only: drop any prior network approval.
                models.approved_network_runtimes.retain(|r| r != &id);
                models.approved_network_runtime_hashes.remove(&id);
            }
        })
    }

    /// Revoke a runtime's approvals (host + network) and drop its hashes.
    pub fn revoke_runtime(&self, id: &str) -> Result<Settings, SettingsError> {
        self.update(|settings| {
            let models = &mut settings.models;
            models.approved_host_runtimes.retain(|r| r != id);
            models.approved_network_runtimes.retain(|r| r != id);
            models.approved_host_runtime_hashes.remove(id);
            models.approved_network_runtime_hashes.remove(id);
        })
    }

    fn io_error(&self, source: std::io::Error) -> SettingsError {
        SettingsError::Io {
            path: self.path.to_string_lossy().into_owned(),
            source,
        }
    }
}

fn decode_table<T: DeserializeOwned + Default>(table: &toml::Table, key: &str) -> T {
    table
        .get(key)
        .cloned()
        .and_then(|value| value.try_into().ok())
        .unwrap_or_default()
}

fn non_empty(value: &str) -> Option<&str> {
    (!value.is_empty()).then_some(value)
}

fn push_unique(list: &mut Vec<String>, value: String) {
    if !list.contains(&value) {
        list.push(value);
    }
}

/// Expand a leading `~` against `$HOME`; other paths pass through unchanged.
fn expand_tilde(path: &str) -> String {
    match std::env::var("HOME") {
        Ok(home) => kernel::fs::expand_tilde(path, Path::new(&home))
            .to_string_lossy()
            .into_owned(),
        Err(_) => path.to_owned(),
    }
}

/// `<config-dir>/hedos.toml`, where the config dir is `XDG_CONFIG_HOME`, else
/// `$HOME/.config`, else `%APPDATA%`, else the current directory.
fn config_file_path(env: &HashMap<String, String>) -> PathBuf {
    let non_empty = |key: &str| env.get(key).filter(|value| !value.is_empty()).cloned();
    let base = non_empty("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| non_empty("HOME").map(|home| Path::new(&home).join(".config")))
        .or_else(|| non_empty("APPDATA").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("hedos.toml")
}

fn process_env() -> HashMap<String, String> {
    std::env::vars().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path() -> PathBuf {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!(
            "hedos-settings-{stamp}-{:?}/hedos.toml",
            std::thread::current().id()
        ))
    }

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn a_missing_file_loads_defaults() {
        let store = SettingsStore::new("/no/such/hedos.toml");
        assert_eq!(store.load(), Settings::default());
    }

    #[test]
    fn settings_round_trip_through_toml() {
        let path = temp_path();
        let store = SettingsStore::new(&path);
        let mut settings = Settings::default();
        settings.models.watched_folders = vec!["/models".to_owned()];
        settings.models.keep_warm = KeepWarmPolicy::OneHour;
        settings.models.eviction = EvictionPolicy::Budgeted;
        settings.models.ram_budget_mb = 8192;
        settings.chat.default_model_id = "llama3".to_owned();
        settings.voice.speed = 1.25;
        settings.gateway.port = 9000;

        store.save(&settings).expect("save");
        assert_eq!(store.load(), settings);
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn the_policy_enums_serialize_as_snake_case() {
        let path = temp_path();
        let store = SettingsStore::new(&path);
        let mut settings = Settings::default();
        settings.models.keep_warm = KeepWarmPolicy::FifteenMinutes;
        settings.models.eviction = EvictionPolicy::StrictSingle;
        store.save(&settings).expect("save");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains(r#"keep_warm = "fifteen_minutes""#), "{text}");
        assert!(text.contains(r#"eviction = "strict_single""#), "{text}");
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn a_partial_file_keeps_present_fields_and_defaults_the_rest() {
        let path = temp_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "[chat]\ndefault_model_id = \"gemma\"\nshow_stats = false\n",
        )
        .unwrap();
        let settings = SettingsStore::new(&path).load();
        assert_eq!(settings.chat.default_model_id, "gemma");
        assert!(!settings.chat.show_stats);
        // Untouched fields keep their defaults.
        assert!(settings.chat.send_with_enter);
        assert_eq!(settings.voice.speed, 1.0);
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn a_malformed_table_defaults_only_that_table() {
        let path = temp_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        // `speed` is a string, not a number → the whole [voice] table falls back,
        // but [chat] is preserved.
        std::fs::write(
            &path,
            "[chat]\ndefault_model_id = \"keep\"\n\n[voice]\nspeed = \"fast\"\n",
        )
        .unwrap();
        let settings = SettingsStore::new(&path).load();
        assert_eq!(settings.chat.default_model_id, "keep");
        assert_eq!(settings.voice.speed, 1.0); // defaulted
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn watched_folders_dedup_and_remove() {
        let path = temp_path();
        let store = SettingsStore::new(&path);
        store.add_watched_folder("/a").expect("add");
        store.add_watched_folder("/a").expect("add dup");
        let settings = store.add_watched_folder("/b").expect("add b");
        assert_eq!(settings.models.watched_folders, vec!["/a", "/b"]);
        let settings = store.remove_watched_folder("/a").expect("remove");
        assert_eq!(settings.models.watched_folders, vec!["/b"]);
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn approve_and_revoke_a_runtime() {
        let path = temp_path();
        let store = SettingsStore::new(&path);
        let settings = store
            .approve_runtime("python:mlx-lm", Some("abc"), true)
            .expect("approve");
        assert!(
            settings
                .models
                .approved_host_runtimes
                .contains(&"python:mlx-lm".to_owned())
        );
        assert!(
            settings
                .models
                .approved_network_runtimes
                .contains(&"python:mlx-lm".to_owned())
        );
        assert_eq!(
            settings
                .models
                .approved_host_runtime_hashes
                .get("python:mlx-lm"),
            Some(&"abc".to_owned())
        );

        let settings = store.revoke_runtime("python:mlx-lm").expect("revoke");
        assert!(settings.models.approved_host_runtimes.is_empty());
        assert!(settings.models.approved_network_runtimes.is_empty());
        assert!(settings.models.approved_host_runtime_hashes.is_empty());
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn re_approving_host_only_revokes_prior_network_approval() {
        let path = temp_path();
        let store = SettingsStore::new(&path);
        store
            .approve_runtime("python:mlx-lm", Some("h1"), true)
            .expect("approve with network");
        // Re-approve host-only: network approval + its hash must be dropped.
        let settings = store
            .approve_runtime("python:mlx-lm", Some("h1"), false)
            .expect("re-approve host only");
        assert!(
            settings
                .models
                .approved_host_runtimes
                .contains(&"python:mlx-lm".to_owned())
        );
        assert!(settings.models.approved_network_runtimes.is_empty());
        assert!(settings.models.approved_network_runtime_hashes.is_empty());
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn set_and_clear_the_default_chat_model() {
        let path = temp_path();
        let store = SettingsStore::new(&path);
        let settings = store.set_default_chat_model(Some("qwen")).expect("set");
        assert_eq!(settings.chat.default_model_id(), Some("qwen"));
        let settings = store.set_default_chat_model(None).expect("clear");
        assert_eq!(settings.chat.default_model_id(), None);
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn the_config_path_follows_the_xdg_locator() {
        assert_eq!(
            SettingsStore::with_env(&env(&[("XDG_CONFIG_HOME", "/xdg")])).path(),
            Path::new("/xdg/hedos.toml")
        );
        assert_eq!(
            SettingsStore::with_env(&env(&[("HOME", "/home/me")])).path(),
            Path::new("/home/me/.config/hedos.toml")
        );
        // XDG wins over HOME.
        assert_eq!(
            SettingsStore::with_env(&env(&[("XDG_CONFIG_HOME", "/xdg"), ("HOME", "/home/me")]))
                .path(),
            Path::new("/xdg/hedos.toml")
        );
    }

    #[test]
    fn ram_budget_zero_reads_as_unset() {
        let mut models = ModelsSettings::default();
        assert_eq!(models.ram_budget_mb(), None);
        models.ram_budget_mb = 4096;
        assert_eq!(models.ram_budget_mb(), Some(4096));
    }
}
