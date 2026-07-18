//! The Hugging Face install provider: searches and plans over [`HFHubAPI`], then
//! downloads a repo's selected files into the hub cache via [`HFCacheWriter`],
//! streaming throttled byte-progress. An interrupted install cleans up after
//! itself so a half-written repo doesn't masquerade as installed.
//!
//! The cache `root` and the auth token are supplied by the caller (the token via
//! the [`HFHubAPI`]); Swift's env/settings/secret-store resolution
//! (`HF_HUB_CACHE`/`HF_HOME`/`HF_TOKEN`) lives in the composition layer, not here.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use kernel::install::file_selection::{self, HFSibling};
use kernel::install::reference::hugging_face_repo;
use kernel::install::{
    InstallAvailability, InstallError, InstallPlan, InstallPlanFile, InstallProgress,
    InstallProviderId, InstallSearchHit, InstallStreamEvent,
};
use kernel::records::SourceKind;
use tokio::sync::mpsc;

use super::hf_cache::{HFCacheLayout, HFCacheWriter};
use super::hf_hub::HFHubAPI;
use super::provider::{InstallEventStream, InstallFuture, InstallProvider};
use super::transport::InstallTransport;

/// Interrupted repos keep their bytes only if at least one file this big landed.
const WEIGHT_KEEP_THRESHOLD: i64 = 10 << 20;

/// Installs models by downloading Hugging Face repos into the hub cache.
pub struct HuggingFaceInstallProvider {
    api: HFHubAPI,
    transport: Arc<dyn InstallTransport>,
    root: PathBuf,
    home: PathBuf,
}

impl HuggingFaceInstallProvider {
    /// A provider fetching over `transport`, writing into hub-cache `root`, with
    /// `home` used only to shorten the displayed destination.
    pub fn new(
        api: HFHubAPI,
        transport: Arc<dyn InstallTransport>,
        root: impl Into<PathBuf>,
        home: impl Into<PathBuf>,
    ) -> Self {
        Self {
            api,
            transport,
            root: root.into(),
            home: home.into(),
        }
    }

    fn display_path(&self) -> String {
        display_path(&self.root, &self.home)
    }
}

impl InstallProvider for HuggingFaceInstallProvider {
    fn id(&self) -> InstallProviderId {
        InstallProviderId::huggingface()
    }

    fn display_name(&self) -> &str {
        "Hugging Face"
    }

    fn source_kind(&self) -> SourceKind {
        SourceKind::huggingface_cache()
    }

    fn supports_search(&self) -> bool {
        true
    }

    fn availability(&self) -> InstallFuture<'_, InstallAvailability> {
        Box::pin(async { InstallAvailability::Ready })
    }

    fn search(
        &self,
        query: &str,
        limit: usize,
    ) -> InstallFuture<'_, Result<Vec<InstallSearchHit>, InstallError>> {
        let query = query.to_owned();
        Box::pin(async move { self.api.search(&query, limit).await })
    }

    fn plan(&self, reference: &str) -> InstallFuture<'_, Result<InstallPlan, InstallError>> {
        let reference = reference.to_owned();
        Box::pin(async move {
            let typed = hugging_face_repo(&reference)
                .ok_or_else(|| InstallError::ReferenceInvalid(reference.clone()))?;
            let info = self.api.model_info(&typed).await?;
            let repo = info.repo.clone();
            let selection = file_selection::select(&info.siblings);
            if !selection.iter().any(HFSibling::is_weight) {
                return Err(InstallError::TransferFailed(format!(
                    "{repo} has no model weights hedos knows how to download."
                )));
            }

            let layout = HFCacheLayout::new(&self.root, &repo);
            let writer = HFCacheWriter::new(layout, Arc::clone(&self.transport));
            let total = summed_bytes(&selection);
            let present = writer.present_bytes(&selection, info.sha.as_deref().unwrap_or(""));
            let remaining = total.map(|total| (total - present).max(0));

            let mut plan = InstallPlan::new(
                InstallProviderId::huggingface(),
                repo.clone(),
                last_segment(&repo),
                self.display_path(),
            );
            plan.revision = info.sha.clone();
            plan.files = selection
                .iter()
                .map(|sibling| InstallPlanFile::new(&sibling.rfilename, sibling.bytes))
                .collect();
            plan.total_bytes = total;
            plan.remaining_bytes = remaining;
            plan.requires_auth = info.gated && !self.api.has_token();
            Ok(plan)
        })
    }

    fn install(&self, plan: InstallPlan) -> InstallEventStream {
        let (tx, rx) = mpsc::channel(32);
        let api = self.api.clone();
        let transport = Arc::clone(&self.transport);
        let root = self.root.clone();
        tokio::spawn(async move {
            let layout = HFCacheLayout::new(&root, &plan.reference);
            let writer = HFCacheWriter::new(layout, transport);
            let repo_existed_before = writer.layout().repo_directory().exists();

            let outcome = tokio::select! {
                biased;
                _ = tx.closed() => {
                    // Cancelled: the receiver is gone, so just clean up.
                    clean_up_after_interruption(&writer, repo_existed_before);
                    return;
                }
                outcome = run_install(&api, &writer, &plan, &tx) => outcome,
            };

            if let Err(error) = outcome {
                clean_up_after_interruption(&writer, repo_existed_before);
                let _ = tx.send(Err(error)).await;
            }
        });
        rx
    }
}

/// Resolve, order, and download the repo's files, emitting progress on `tx`.
async fn run_install(
    api: &HFHubAPI,
    writer: &HFCacheWriter,
    plan: &InstallPlan,
    tx: &mpsc::Sender<Result<InstallStreamEvent, InstallError>>,
) -> Result<(), InstallError> {
    let _ = tx
        .send(Ok(InstallStreamEvent::Status(format!(
            "Resolving {}",
            plan.reference
        ))))
        .await;

    let info = api.model_info(&plan.reference).await?;
    let revision = info
        .sha
        .clone()
        .or_else(|| plan.revision.clone())
        .ok_or_else(|| {
            InstallError::TransferFailed(format!("{} has no resolvable revision.", plan.reference))
        })?;
    let selection = file_selection::select(&info.siblings);
    if !selection.iter().any(HFSibling::is_weight) {
        return Err(InstallError::TransferFailed(format!(
            "{} has no model weights hedos knows how to download.",
            plan.reference
        )));
    }

    let ordered = download_order(selection);
    let first_weight = ordered.iter().find(|sibling| sibling.is_weight());
    writer.prepare_skeleton(
        &revision,
        first_weight
            .map(|sibling| HFCacheWriter::pending_blob_name(sibling, &revision))
            .as_deref(),
    )?;
    let keeping: HashSet<String> = ordered
        .iter()
        .map(|sibling| HFCacheWriter::pending_blob_name(sibling, &revision))
        .collect();
    writer.remove_stray_incompletes(&keeping)?;

    let meter = Arc::new(InstallProgressMeter::new(summed_bytes(&ordered)));
    for sibling in &ordered {
        let _ = tx
            .send(Ok(InstallStreamEvent::Progress(
                meter.begin(&sibling.rfilename),
            )))
            .await;
        let request = api.resolve_request(&plan.reference, &revision, &sibling.rfilename);
        let meter = Arc::clone(&meter);
        // A sender clone for the sync progress callback. `closed()` tracks the
        // receiver, not the sender count, so this clone doesn't defeat the outer
        // cancel arm; it's dropped when the closure drops at the iteration's end.
        let tx = tx.clone();
        let mut on_bytes = move |delta: i64| {
            if let Some(progress) = meter.add(delta) {
                // Progress is throttled and lossy-tolerant; a full channel just
                // means the consumer will catch up on the next emit.
                let _ = tx.try_send(Ok(InstallStreamEvent::Progress(progress)));
            }
        };
        writer
            .download(sibling, &revision, request, &mut on_bytes)
            .await?;
    }

    writer.commit_ref(&revision)?;
    writer.remove_stray_incompletes(&HashSet::new())?;
    let _ = tx
        .send(Ok(InstallStreamEvent::Progress(meter.finish())))
        .await;
    Ok(())
}

/// Order files so the small support files land before the big weights, then by
/// size and name — matching the Swift download order.
fn download_order(mut selection: Vec<HFSibling>) -> Vec<HFSibling> {
    selection.sort_by(|first, second| {
        // `false` (non-weight) sorts before `true` (weight).
        first
            .is_weight()
            .cmp(&second.is_weight())
            .then_with(|| first.bytes.unwrap_or(0).cmp(&second.bytes.unwrap_or(0)))
            .then_with(|| first.rfilename.cmp(&second.rfilename))
    });
    selection
}

/// If the repo wasn't there before, drop a half-finished install: remove it
/// entirely unless a substantial blob landed, in which case keep the blobs but
/// drop the ref/snapshots until a blob actually completes.
fn clean_up_after_interruption(writer: &HFCacheWriter, repo_existed_before: bool) {
    if repo_existed_before {
        return;
    }
    if !writer.has_substantial_progress(WEIGHT_KEEP_THRESHOLD) {
        writer.remove_repo();
        return;
    }
    if !writer.has_completed_blob() {
        writer.retreat_to_blobs_only();
    }
}

fn summed_bytes(selection: &[HFSibling]) -> Option<i64> {
    let sizes: Vec<i64> = selection
        .iter()
        .filter_map(|sibling| sibling.bytes)
        .collect();
    if sizes.is_empty() {
        None
    } else {
        Some(sizes.into_iter().fold(0i64, i64::saturating_add))
    }
}

fn last_segment(repo: &str) -> String {
    repo.rsplit('/')
        .find(|part| !part.is_empty())
        .unwrap_or(repo)
        .to_owned()
}

/// The destination path to show, with the home prefix collapsed to `~`.
fn display_path(root: &Path, home: &Path) -> String {
    let path = root.to_string_lossy().into_owned();
    let home = home.to_string_lossy();
    if !home.is_empty() && (path == *home || path.starts_with(&format!("{home}/"))) {
        return format!("~{}", &path[home.len()..]);
    }
    path
}

/// Throttles download progress so the UI isn't flooded: emits at most every
/// 8 MiB or 300 ms.
struct InstallProgressMeter {
    total_bytes: Option<i64>,
    state: Mutex<MeterState>,
}

struct MeterState {
    downloaded: i64,
    current_file: Option<String>,
    last_emitted_bytes: i64,
    last_emitted_at: Instant,
}

impl InstallProgressMeter {
    const EMIT_BYTES: i64 = 8 << 20;
    const EMIT_INTERVAL: Duration = Duration::from_millis(300);

    fn new(total_bytes: Option<i64>) -> Self {
        Self {
            total_bytes,
            state: Mutex::new(MeterState {
                downloaded: 0,
                current_file: None,
                last_emitted_bytes: 0,
                last_emitted_at: Instant::now(),
            }),
        }
    }

    fn begin(&self, file: &str) -> InstallProgress {
        let mut state = self.lock();
        state.current_file = Some(file.to_owned());
        state.last_emitted_bytes = state.downloaded;
        state.last_emitted_at = Instant::now();
        self.snapshot(&state)
    }

    fn add(&self, delta: i64) -> Option<InstallProgress> {
        let mut state = self.lock();
        state.downloaded += delta;
        let now = Instant::now();
        if state.downloaded - state.last_emitted_bytes < Self::EMIT_BYTES
            && now.duration_since(state.last_emitted_at) < Self::EMIT_INTERVAL
        {
            return None;
        }
        state.last_emitted_bytes = state.downloaded;
        state.last_emitted_at = now;
        Some(self.snapshot(&state))
    }

    fn finish(&self) -> InstallProgress {
        let mut state = self.lock();
        state.current_file = None;
        self.snapshot(&state)
    }

    fn snapshot(&self, state: &MeterState) -> InstallProgress {
        InstallProgress {
            bytes_downloaded: state.downloaded.max(0),
            total_bytes: self.total_bytes,
            total_is_partial: false,
            current_file: state.current_file.clone(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, MeterState> {
        // The lock is never held across an await; poisoning can't happen here.
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn weight(name: &str, bytes: i64) -> HFSibling {
        HFSibling::new(name, Some(bytes))
    }

    #[test]
    fn download_order_puts_support_files_before_weights_then_by_size_and_name() {
        let ordered = download_order(vec![
            weight("model.safetensors", 5000),
            weight("config.json", 20),
            weight("tokenizer.json", 300),
            weight("model2.safetensors", 100),
        ]);
        let names: Vec<&str> = ordered.iter().map(|s| s.rfilename.as_str()).collect();
        // Non-weights first (config, tokenizer by size), then weights by size.
        assert_eq!(
            names,
            [
                "config.json",
                "tokenizer.json",
                "model2.safetensors",
                "model.safetensors"
            ]
        );
    }

    #[test]
    fn the_meter_throttles_small_frequent_updates() {
        let meter = InstallProgressMeter::new(Some(100 << 20));
        meter.begin("w.bin");
        // A tiny delta right after begin: under both the 8 MiB and 300 ms thresholds.
        assert!(meter.add(16).is_none());
        // A delta over the byte threshold emits.
        let emitted = meter.add(9 << 20).expect("a large delta emits");
        assert_eq!(emitted.bytes_downloaded, (9 << 20) + 16);
        assert_eq!(emitted.total_bytes, Some(100 << 20));
        assert!(!emitted.total_is_partial); // HF totals are exact → a fraction shows
    }

    #[test]
    fn display_path_collapses_the_home_prefix() {
        assert_eq!(
            display_path(Path::new("/home/me/.cache/hf"), Path::new("/home/me")),
            "~/.cache/hf"
        );
        assert_eq!(
            display_path(Path::new("/data/models"), Path::new("/home/me")),
            "/data/models"
        );
    }

    #[test]
    fn last_segment_is_the_repo_name() {
        assert_eq!(last_segment("org/Model"), "Model");
        assert_eq!(last_segment("bare"), "bare");
        assert_eq!(last_segment("org/"), "org");
    }
}
