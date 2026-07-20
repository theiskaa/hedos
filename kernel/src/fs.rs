//! Small filesystem-path helpers shared across the workspace.

use std::path::{Path, PathBuf};

/// Expand a leading `~` or `~/` in `path` against `home`. A path that does not
/// begin with a tilde segment is returned unchanged.
pub fn expand_tilde(path: &str, home: &Path) -> PathBuf {
    if path == "~" {
        home.to_path_buf()
    } else if let Some(rest) = path.strip_prefix("~/") {
        home.join(rest)
    } else {
        PathBuf::from(path)
    }
}

/// Expand a leading `~`/`~/` in `path` against `$HOME`, read from the process
/// environment. A path without a tilde segment — or when `$HOME` is unset —
/// passes through unchanged.
pub fn expand_tilde_env(path: &str) -> PathBuf {
    match std::env::var("HOME") {
        Ok(home) => expand_tilde(path, Path::new(&home)),
        Err(_) => PathBuf::from(path),
    }
}

/// Whether `path` is an existing regular file the process may execute (on
/// non-Unix, merely an existing file — there is no mode bit to check).
pub fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

/// Find `binary` on the process `PATH`, returning the first executable match.
pub fn find_on_path(binary: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(binary))
        .find(|candidate| is_executable(candidate))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tilde_expands_against_home() {
        let home = Path::new("/home/koala");
        assert_eq!(expand_tilde("~", home), PathBuf::from("/home/koala"));
        assert_eq!(
            expand_tilde("~/models/a.gguf", home),
            PathBuf::from("/home/koala/models/a.gguf")
        );
    }

    #[test]
    fn a_trailing_slash_on_home_is_normalized() {
        assert_eq!(
            expand_tilde("~/x", Path::new("/home/koala/")),
            PathBuf::from("/home/koala/x")
        );
    }

    #[test]
    fn a_non_tilde_path_is_unchanged() {
        let home = Path::new("/home/koala");
        assert_eq!(expand_tilde("/abs/path", home), PathBuf::from("/abs/path"));
        assert_eq!(expand_tilde("relative", home), PathBuf::from("relative"));
        // A tilde not at the start is not a home reference.
        assert_eq!(expand_tilde("a/~/b", home), PathBuf::from("a/~/b"));
    }
}
