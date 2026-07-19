//! The per-route request handlers, plus the streaming helpers they share.

pub mod stream;

/// A fresh, opaque completion id with the given prefix (e.g. `chatcmpl-`). Uses
/// process entropy rather than a UUID dependency; clients treat it as opaque.
pub fn completion_id(prefix: &str) -> String {
    use std::hash::{BuildHasher, Hasher};
    let high = std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish();
    let low = std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish();
    format!("{prefix}{high:016x}{low:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_completion_id_carries_its_prefix_and_is_unique() {
        let first = completion_id("chatcmpl-");
        let second = completion_id("chatcmpl-");
        assert!(first.starts_with("chatcmpl-"));
        assert_eq!(first.len(), "chatcmpl-".len() + 32);
        assert_ne!(first, second);
    }
}
