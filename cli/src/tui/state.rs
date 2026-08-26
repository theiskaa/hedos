//! What the UI remembers between runs: where the selection was, the sort,
//! and the filter. State, not settings, so it lives in the data dir.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::order::Sort;

const FILE: &str = "ui.toml";

/// The remembered UI state.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct UiState {
    /// The id of the model that was selected.
    pub selected_id: Option<String>,
    /// The sort in effect.
    pub sort: Sort,
    /// The filter in effect.
    pub filter: String,
}

impl UiState {
    fn path(directory: &Path) -> PathBuf {
        directory.join(FILE)
    }

    /// The state saved in `directory`, or the default when there is none or
    /// it does not parse.
    pub fn load(directory: &Path) -> Self {
        fs::read_to_string(Self::path(directory))
            .ok()
            .and_then(|text| toml::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Save into `directory`, creating it. A failure is not worth stopping
    /// for: the next run simply starts at the top.
    pub fn save(&self, directory: &Path) {
        let Ok(text) = toml::to_string(self) else {
            return;
        };
        let _ = fs::create_dir_all(directory);
        let _ = fs::write(Self::path(directory), text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "hedos-ui-state-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn state_round_trips() {
        let dir = temp_dir();
        let state = UiState {
            selected_id: Some("abc".to_owned()),
            sort: Sort::LastUsed,
            filter: "qw".to_owned(),
        };
        state.save(&dir);
        assert_eq!(UiState::load(&dir), state);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_or_broken_state_is_the_default() {
        let dir = temp_dir();
        assert_eq!(UiState::load(&dir), UiState::default());
        fs::write(UiState::path(&dir), "not = [toml").expect("write");
        assert_eq!(UiState::load(&dir), UiState::default());
        let _ = fs::remove_dir_all(dir);
    }
}
