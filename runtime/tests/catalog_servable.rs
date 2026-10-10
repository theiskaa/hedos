//! Every model the install catalog recommends is one hedos can serve, judged
//! the way the shelf will judge it once pulled.
//!
//! Nothing is downloaded. Each entry is laid out the way a pull leaves it, from
//! what its registry publishes: a Hugging Face repo from its captured listing
//! (`fixtures/catalog`, the small JSON configs verbatim and every other file
//! the pull selects as a sparse file of its listed size), an Ollama tag as a
//! manifest over a sparse model layer. Then the real scanners find it and the
//! real adapters bid on it, and the test asserts that it resolves to the
//! engine the catalog names, with the sizes the catalog states.

mod support;

use std::collections::BTreeMap;
use std::fs::File;
use std::path::Path;

use kernel::discovery::{HFCacheScanner, OllamaStoreScanner, StoreScanner};
use kernel::install::catalog::{InstallCatalogEntry, entries};
use kernel::install::{HFSibling, InstallProviderId, select};
use kernel::machine::Engine;
use kernel::records::ModelRecord;
use runtime::boot::{HedosDirs, build_kernel};
use runtime::settings::Settings;
use serde::Deserialize;
use support::TempDir;

const TEMPLATE: &str = "{{ .Prompt }}";

#[derive(Deserialize)]
struct Fixture {
    sha: String,
    files: Vec<(String, i64)>,
    texts: BTreeMap<String, String>,
}

fn fixture(repo: &str) -> Fixture {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/catalog")
        .join(format!("{}.json", repo.replace('/', "--")));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    serde_json::from_str(&text).expect("a fixture")
}

/// A file of `bytes` that takes no disk: the scanners read sizes and the
/// headers of what they parse, and the parsed files are written verbatim.
fn sparse(path: &Path, bytes: i64) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    File::create(path)
        .unwrap()
        .set_len(bytes.max(0) as u64)
        .unwrap();
}

/// `repo` as a pull leaves it in the hub cache under `root`: the selected
/// files as blobs, linked from the snapshot `refs/main` names.
fn lay_out_repo(root: &Path, repo: &str) {
    let fixture = fixture(repo);
    let siblings: Vec<HFSibling> = fixture
        .files
        .iter()
        .map(|(path, bytes)| HFSibling::new(path.clone(), Some(*bytes)))
        .collect();
    let dir = root.join(format!("models--{}", repo.replace('/', "--")));
    let snapshot = dir.join("snapshots").join(&fixture.sha);
    std::fs::create_dir_all(dir.join("refs")).unwrap();
    std::fs::write(dir.join("refs/main"), &fixture.sha).unwrap();
    for (index, sibling) in select(&siblings).iter().enumerate() {
        let blob = dir.join("blobs").join(format!("{index:064x}"));
        match fixture.texts.get(&sibling.rfilename) {
            Some(text) => {
                std::fs::create_dir_all(blob.parent().unwrap()).unwrap();
                std::fs::write(&blob, text).unwrap();
            }
            None => sparse(&blob, sibling.bytes.unwrap_or(0)),
        }
        let link = snapshot.join(&sibling.rfilename);
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&blob, &link).unwrap();
    }
}

/// `entry`'s tag as a pull leaves it in the Ollama store under `root`: a
/// manifest over a template layer and a model layer that make up its size.
fn lay_out_tag(root: &Path, entry: &InstallCatalogEntry, index: usize) {
    let (name, tag) = entry.reference.split_once(':').unwrap();
    let model_digest = format!("sha256:{:064x}", 2 * index + 1);
    let template_digest = format!("sha256:{:064x}", 2 * index + 2);
    let model_bytes = entry.download_bytes as i64 - TEMPLATE.len() as i64;
    sparse(
        &root.join("blobs").join(model_digest.replace(':', "-")),
        model_bytes,
    );
    std::fs::write(
        root.join("blobs").join(template_digest.replace(':', "-")),
        TEMPLATE,
    )
    .unwrap();
    let manifest = serde_json::json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.docker.distribution.manifest.v2+json",
        "layers": [
            {
                "mediaType": "application/vnd.ollama.image.model",
                "digest": model_digest,
                "size": model_bytes,
            },
            {
                "mediaType": "application/vnd.ollama.image.template",
                "digest": template_digest,
                "size": TEMPLATE.len(),
            },
        ],
    });
    let path = root
        .join("manifests/registry.ollama.ai/library")
        .join(name)
        .join(tag);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, manifest.to_string()).unwrap();
}

/// The shelf after every catalog entry is laid out and discovered.
async fn shelf_of_the_catalog(dir: &TempDir) -> Vec<ModelRecord> {
    let hub = dir.join("hub");
    let ollama = dir.join("ollama");
    for (index, entry) in entries().iter().enumerate() {
        if entry.provider == InstallProviderId::ollama() {
            lay_out_tag(&ollama, entry, index);
        } else {
            lay_out_repo(&hub, &entry.reference);
        }
    }
    let kernel = build_kernel(
        &HedosDirs {
            data: dir.join("data"),
        },
        &Settings::default(),
    )
    .expect("a kernel");
    let scanners: Vec<Box<dyn StoreScanner>> = vec![
        Box::new(HFCacheScanner::single(hub)),
        Box::new(OllamaStoreScanner::new(ollama)),
    ];
    let summary = kernel.discover(scanners).await.expect("a discovery pass");
    assert!(summary.issues.is_empty(), "{:?}", summary.issues);
    kernel.shelf().await.to_vec()
}

fn record_of<'a>(shelf: &'a [ModelRecord], entry: &InstallCatalogEntry) -> &'a ModelRecord {
    shelf
        .iter()
        .find(|record| record.source.repo.as_deref() == Some(entry.reference.as_str()))
        .unwrap_or_else(|| panic!("{} is not on the shelf", entry.reference))
}

#[tokio::test]
async fn every_catalog_entry_resolves_to_the_engine_it_names() {
    let dir = TempDir::new();
    let shelf = shelf_of_the_catalog(&dir).await;
    let mut failures = Vec::new();
    for entry in entries() {
        let record = record_of(&shelf, &entry);
        let engine = Engine::of_record(record);
        if engine != entry.engine {
            failures.push(format!(
                "{}: resolved to {:?} ({:?}), the catalog says {:?}",
                entry.reference, record.runtime.id, engine, entry.engine
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test]
async fn every_catalog_size_is_what_the_shelf_measures() {
    let dir = TempDir::new();
    let shelf = shelf_of_the_catalog(&dir).await;
    let mut failures = Vec::new();
    for entry in entries() {
        let record = record_of(&shelf, &entry);
        let disk = record.size_on_disk();
        let serving = record.serving_size();
        println!(
            "{:<44} disk {:>14?}  serving {:>14?}",
            entry.reference, disk, serving
        );
        if disk != Some(entry.download_bytes as i64) {
            failures.push(format!(
                "{}: the shelf counts {disk:?} on disk, the catalog says {}",
                entry.reference, entry.download_bytes
            ));
        }
        if serving != Some(entry.serving_bytes as i64) {
            failures.push(format!(
                "{}: the shelf measures {serving:?} to serve, the catalog says {}",
                entry.reference, entry.serving_bytes
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
