//! What the UI remembers between runs: where the selection was, and nothing
//! else; a remembered filter surprised more than it helped. State, not
//! settings, so it lives in the data dir.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const FILE: &str = "ui.toml";

/// The remembered UI state.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct UiState {
    /// The id of the model that was selected.
    pub selected_id: Option<String>,
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
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    /// A directory of its own that goes away with the test, passed or not.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            // Tests run in parallel and the clock is not unique enough on
            // its own.
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let dir = std::env::temp_dir().join(format!(
                "hedos-ui-state-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&dir).expect("temp dir");
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn state_round_trips() {
        let dir = TempDir::new();
        let state = UiState {
            selected_id: Some("abc".to_owned()),
        };
        state.save(&dir.0);
        assert_eq!(UiState::load(&dir.0), state);
    }

    #[test]
    fn missing_or_broken_state_is_the_default() {
        let dir = TempDir::new();
        assert_eq!(UiState::load(&dir.0), UiState::default());
        fs::write(UiState::path(&dir.0), "not = [toml").expect("write");
        assert_eq!(UiState::load(&dir.0), UiState::default());
    }
}
