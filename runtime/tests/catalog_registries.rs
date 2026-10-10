//! The install catalog against the registries it names: every tag and repo
//! still exists, and each download size is within 5 % of what a pull would
//! download today. It reads only manifests and file listings, never a weight file.
//!
//! Ignored by default, since it needs the network. Run it by hand before a
//! release:
//!
//! ```text
//! cargo test -p hedos-runtime --test catalog_registries -- --ignored --nocapture
//! ```

use std::sync::Arc;

use kernel::install::catalog::{InstallCatalogEntry, entries};
use kernel::install::{InstallProviderId, select};
use runtime::install::hf_hub::HFHubAPI;
use runtime::install::transport::ReqwestTransport;
use serde::Deserialize;

const DRIFT: f64 = 0.05;

#[derive(Deserialize)]
struct Manifest {
    layers: Vec<Layer>,
}

#[derive(Deserialize)]
struct Layer {
    size: u64,
}

/// What pulling an Ollama tag downloads: the sum of its manifest's layers.
async fn ollama_bytes(client: &reqwest::Client, reference: &str) -> Result<u64, String> {
    let (name, tag) = reference.split_once(':').unwrap_or((reference, "latest"));
    let url = format!("https://registry.ollama.ai/v2/library/{name}/manifests/{tag}");
    let response = client
        .get(&url)
        .header(
            "accept",
            "application/vnd.docker.distribution.manifest.v2+json",
        )
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status()));
    }
    let manifest: Manifest = response.json().await.map_err(|error| error.to_string())?;
    Ok(manifest.layers.iter().map(|layer| layer.size).sum())
}

/// What pulling a Hugging Face repo downloads: the files the pull selects.
async fn hugging_face_bytes(api: &HFHubAPI, repo: &str) -> Result<u64, String> {
    let info = api
        .model_info(repo)
        .await
        .map_err(|error| error.to_string())?;
    let bytes: i64 = select(&info.siblings)
        .iter()
        .filter_map(|sibling| sibling.bytes)
        .sum();
    u64::try_from(bytes).map_err(|_| "negative size".to_owned())
}

async fn listed_bytes(
    client: &reqwest::Client,
    api: &HFHubAPI,
    entry: &InstallCatalogEntry,
) -> Result<u64, String> {
    if entry.provider == InstallProviderId::ollama() {
        ollama_bytes(client, &entry.reference).await
    } else {
        hugging_face_bytes(api, &entry.reference).await
    }
}

#[tokio::test]
#[ignore = "reads the Ollama and Hugging Face registries"]
async fn catalog_sizes_match_the_registries() {
    let client = reqwest::Client::new();
    let api = HFHubAPI::new(Arc::new(ReqwestTransport::new()));
    let mut failures = Vec::new();
    for entry in entries() {
        match listed_bytes(&client, &api, &entry).await {
            Ok(bytes) => {
                let drift =
                    (bytes as f64 - entry.download_bytes as f64).abs() / bytes.max(1) as f64;
                println!(
                    "{:<44} listed {bytes:>14}  catalog {:>14}  drift {:.1} %",
                    entry.reference,
                    entry.download_bytes,
                    drift * 100.0
                );
                if drift > DRIFT {
                    failures.push(format!(
                        "{}: catalog {} vs listed {bytes}",
                        entry.reference, entry.download_bytes
                    ));
                }
            }
            Err(error) => failures.push(format!("{}: {error}", entry.reference)),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
