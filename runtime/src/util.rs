//! Small crate-internal helpers.

use std::path::Path;

/// The size of the file at `path` in whole mebibytes, or `None` if it can't be
/// stat'd.
pub(crate) fn weights_mb(path: &Path) -> Option<i64> {
    std::fs::metadata(path)
        .ok()
        .map(|meta| (meta.len() / (1 << 20)) as i64)
}
