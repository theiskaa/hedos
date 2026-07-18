//! Tests for `IdentificationCache`: hit on unchanged bytes, miss on a changed
//! signature, and the uncached cheap kinds.

mod support;

use kernel::records::{Modality, ModelRecord, ModelSource, SourceKind};
use kernel::resolution::{IdentificationCache, ModelFormat};
use support::TempDir;

fn record(kind: SourceKind, path: &str) -> ModelRecord {
    ModelRecord::new(
        "m",
        Modality::text(),
        Vec::new(),
        ModelSource::new(kind, path),
    )
}

#[test]
fn a_second_identify_of_an_unchanged_file_is_a_cache_hit() {
    let dir = TempDir::new();
    let gguf = dir.path().join("model.gguf");
    std::fs::write(&gguf, b"GGUF").unwrap();
    let cache = IdentificationCache::new();
    let rec = record(SourceKind::file(), gguf.to_str().unwrap());

    let first = cache.identify(&rec);
    assert_eq!(first.format, ModelFormat::Gguf);
    assert_eq!(cache.hit_count(), 0);

    let second = cache.identify(&rec);
    assert_eq!(second, first);
    assert_eq!(cache.hit_count(), 1);
}

#[test]
fn a_changed_file_size_invalidates_the_cache() {
    let dir = TempDir::new();
    let gguf = dir.path().join("model.gguf");
    std::fs::write(&gguf, b"GGUF").unwrap();
    let cache = IdentificationCache::new();
    let rec = record(SourceKind::file(), gguf.to_str().unwrap());

    cache.identify(&rec);
    // Rewrite with a different length → the freshness signature changes.
    std::fs::write(&gguf, b"GGUFGGUF").unwrap();
    cache.identify(&rec);
    // The second call re-identified rather than hitting the stale entry.
    assert_eq!(cache.hit_count(), 0);
}

#[test]
fn a_folder_signature_tracks_its_children() {
    let dir = TempDir::new();
    let folder = dir.path().join("bundle");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(
        folder.join("config.json"),
        br#"{"architectures":["LlamaForCausalLM"]}"#,
    )
    .unwrap();
    std::fs::write(folder.join("model.safetensors"), b"weights").unwrap();
    let cache = IdentificationCache::new();
    let rec = record(SourceKind::folder(), folder.to_str().unwrap());

    cache.identify(&rec);
    let hit = cache.identify(&rec);
    assert_eq!(hit.format, ModelFormat::Safetensors);
    assert_eq!(cache.hit_count(), 1);

    // Adding a child shifts the aggregate size → the next identify misses.
    std::fs::write(folder.join("extra.bin"), b"more-bytes").unwrap();
    cache.identify(&rec);
    assert_eq!(cache.hit_count(), 1);
}

#[test]
fn the_cheap_kinds_are_never_cached() {
    let cache = IdentificationCache::new();
    // Ollama identification is a fixed profile — never a cache entry.
    let ollama = record(SourceKind::ollama(), "/store/manifest");
    cache.identify(&ollama);
    cache.identify(&ollama);
    assert_eq!(cache.hit_count(), 0);

    // Builtin likewise.
    let builtin = record(SourceKind::builtin(), "apple");
    cache.identify(&builtin);
    cache.identify(&builtin);
    assert_eq!(cache.hit_count(), 0);

    // Endpoint (the third uncached kind) likewise.
    let endpoint = record(SourceKind::endpoint(), "https://api");
    cache.identify(&endpoint);
    cache.identify(&endpoint);
    assert_eq!(cache.hit_count(), 0);
}

/// Rewrite `path` with `contents` after nudging the clock, so its mtime is
/// distinct from the previous write even at coarse filesystem resolution.
fn rewrite_after_tick(path: &std::path::Path, contents: &[u8]) {
    std::thread::sleep(std::time::Duration::from_millis(10));
    std::fs::write(path, contents).unwrap();
}

#[test]
fn a_same_size_mtime_change_on_a_file_invalidates_the_cache() {
    let dir = TempDir::new();
    let gguf = dir.path().join("model.gguf");
    std::fs::write(&gguf, b"GGUF").unwrap();
    let cache = IdentificationCache::new();
    let rec = record(SourceKind::file(), gguf.to_str().unwrap());

    cache.identify(&rec);
    // Same length, later mtime → the signature still changes.
    rewrite_after_tick(&gguf, b"LMGG");
    cache.identify(&rec);
    assert_eq!(cache.hit_count(), 0);
    // And the fresh identification is now cached: a third call hits.
    cache.identify(&rec);
    assert_eq!(cache.hit_count(), 1);
}

#[test]
fn a_child_mtime_bump_invalidates_a_folder_entry() {
    let dir = TempDir::new();
    let folder = dir.path().join("bundle");
    std::fs::create_dir_all(&folder).unwrap();
    let config = folder.join("config.json");
    std::fs::write(&config, br#"{"architectures":["LlamaForCausalLM"]}"#).unwrap();
    std::fs::write(folder.join("model.safetensors"), b"weights").unwrap();
    let cache = IdentificationCache::new();
    let rec = record(SourceKind::folder(), folder.to_str().unwrap());

    cache.identify(&rec);
    cache.identify(&rec); // warm: a hit accrues
    let hits_before = cache.hit_count();
    assert!(hits_before >= 1);
    // Rewrite the child with identical bytes → same size, later mtime. Only the
    // newest-child-mtime fold (not the size fold) can catch this.
    rewrite_after_tick(&config, br#"{"architectures":["LlamaForCausalLM"]}"#);
    cache.identify(&rec);
    // No new hit — the entry was invalidated by the child's mtime alone.
    assert_eq!(cache.hit_count(), hits_before);
}

#[test]
fn records_sharing_a_path_but_differing_by_reference_do_not_collide() {
    let dir = TempDir::new();
    let gguf = dir.path().join("model.gguf");
    std::fs::write(&gguf, b"GGUF").unwrap();
    let cache = IdentificationCache::new();

    let mut a = record(SourceKind::file(), gguf.to_str().unwrap());
    a.source.reference = Some("rev-a".to_owned());
    let mut b = record(SourceKind::file(), gguf.to_str().unwrap());
    b.source.reference = Some("rev-b".to_owned());

    cache.identify(&a);
    // Different reference → a distinct key → a miss, not `a`'s cached entry.
    cache.identify(&b);
    assert_eq!(cache.hit_count(), 0);
    // Each now hits under its own key.
    cache.identify(&a);
    cache.identify(&b);
    assert_eq!(cache.hit_count(), 2);
}

#[test]
fn concurrent_identify_of_the_same_path_is_safe() {
    use std::sync::Arc;
    let dir = TempDir::new();
    let gguf = dir.path().join("model.gguf");
    std::fs::write(&gguf, b"GGUF").unwrap();
    let cache = Arc::new(IdentificationCache::new());
    let path = gguf.to_str().unwrap().to_owned();

    let handles: Vec<_> = (0..8)
        .map(|_| {
            let cache = Arc::clone(&cache);
            let path = path.clone();
            std::thread::spawn(move || {
                let rec = record(SourceKind::file(), &path);
                cache.identify(&rec).format
            })
        })
        .collect();
    for handle in handles {
        assert_eq!(handle.join().unwrap(), ModelFormat::Gguf);
    }
}

#[test]
fn a_missing_path_falls_back_to_a_direct_identify() {
    let cache = IdentificationCache::new();
    // No extension → not GGUF-by-name; a missing path identifies as Unknown.
    let rec = record(SourceKind::file(), "/does/not/exist");
    // No panic, no cache entry — just a direct identification.
    let id = cache.identify(&rec);
    assert_eq!(id.format, ModelFormat::Unknown);
    cache.identify(&rec);
    assert_eq!(cache.hit_count(), 0);
}
