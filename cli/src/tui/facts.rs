//! The machine facts the screen shows, gathered once per refresh: memory,
//! what is loaded and by whom, disk by store, and what the gateway served.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use gateway::audit::GatewayAuditEntry;
use gateway::stats::{LatencyPercentiles, percentiles};
use kernel::discovery;
use kernel::records::ModelRecord;
use kernel::time::now_millis;

use crate::support::machine;
use crate::support::residency::{self, Resident};
use crate::support::session::Session;

const HOUR_MILLIS: i64 = 3_600_000;
const DAY_MILLIS: i64 = 24 * HOUR_MILLIS;
const MINUTE_MILLIS: i64 = 60_000;
/// Hourly buckets in the activity sparkline.
pub const HOURS: usize = 24;
/// Quarter-hour buckets in the header's pulse: a day of them.
pub const PULSE_SLOTS: usize = 96;
const SLOT_MILLIS: i64 = DAY_MILLIS / PULSE_SLOTS as i64;

/// One model's slice of the gateway's recent history. Only served requests
/// count: the gateway records no model on the ones it rejects.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ModelActivity {
    /// Requests served in the last 24 hours.
    pub requests: u64,
    /// Latency percentiles over those.
    pub latency: Option<LatencyPercentiles>,
    /// Requests per hour, oldest first, the last bucket being this hour.
    pub hourly: [u32; HOURS],
    /// When the model was last requested, ever, in Unix milliseconds.
    pub last_seen_millis: i64,
}

/// What the gateway has been doing, from its audit log.
#[derive(Debug, Clone, PartialEq)]
pub struct Activity {
    /// Per model, keyed by the id the gateway resolved the request to.
    pub models: HashMap<String, ModelActivity>,
    /// Requests in the last minute, all models.
    pub requests_last_minute: u64,
    /// Every request the log holds.
    pub total_requests: u64,
    /// When the newest request of any kind came in, in Unix milliseconds.
    pub last_request_millis: i64,
    /// Requests served per hour over the last day, every model together,
    /// oldest first.
    pub hourly: [u32; HOURS],
    /// The same requests per quarter hour over the last day, oldest first,
    /// for the header's pulse. Boxed: the facts ride every refresh event,
    /// and a day of quarter hours would make that the largest of them.
    pub pulse: Box<[u32; PULSE_SLOTS]>,
}

impl Default for Activity {
    fn default() -> Self {
        Self {
            models: HashMap::new(),
            requests_last_minute: 0,
            total_requests: 0,
            last_request_millis: 0,
            hourly: [0; HOURS],
            pulse: Box::new([0; PULSE_SLOTS]),
        }
    }
}

impl Activity {
    /// Fold `entries` at `now` into per-model activity, in one pass.
    pub fn from_entries(entries: &[GatewayAuditEntry], now: i64) -> Self {
        let day_ago = now - DAY_MILLIS;
        let mut models: HashMap<String, ModelActivity> = HashMap::new();
        let mut durations: HashMap<String, Vec<i64>> = HashMap::new();
        let mut pulse = [0; PULSE_SLOTS];
        for entry in entries {
            let Some(model) = &entry.model else {
                continue;
            };
            let activity = models.entry(model.clone()).or_default();
            activity.last_seen_millis = activity.last_seen_millis.max(entry.ts_millis);
            if entry.ts_millis >= day_ago && entry.is_ok() {
                activity.requests += 1;
                let age = (now - entry.ts_millis).max(0) / HOUR_MILLIS;
                activity.hourly[(HOURS - 1).saturating_sub(age as usize)] += 1;
                let slot = (now - entry.ts_millis).max(0) / SLOT_MILLIS;
                pulse[(PULSE_SLOTS - 1).saturating_sub(slot as usize)] += 1;
                durations
                    .entry(model.clone())
                    .or_default()
                    .push(entry.duration_ms);
            }
        }
        for (model, samples) in durations {
            if let Some(activity) = models.get_mut(&model) {
                activity.latency = percentiles(samples);
            }
        }
        let mut hourly = [0; HOURS];
        for activity in models.values() {
            for (total, count) in hourly.iter_mut().zip(activity.hourly) {
                *total += count;
            }
        }
        Self {
            hourly,
            pulse: Box::new(pulse),
            models,
            requests_last_minute: entries
                .iter()
                .filter(|entry| entry.ts_millis >= now - MINUTE_MILLIS)
                .count() as u64,
            total_requests: entries.len() as u64,
            last_request_millis: entries
                .iter()
                .map(|entry| entry.ts_millis)
                .max()
                .unwrap_or(0),
        }
    }

    /// The activity for `record`. Handlers log the resolved id, so the id is
    /// the key; the names are tried for logs older than that.
    pub fn for_record(&self, record: &ModelRecord) -> Option<&ModelActivity> {
        [record.id.as_str(), record.name.as_str()]
            .into_iter()
            .chain(record.alias.as_deref())
            .find_map(|key| self.models.get(key))
    }
}

/// Everything about the machine the screen shows.
#[derive(Debug, Clone, Default)]
pub struct Facts {
    /// The machine's total memory in bytes.
    pub memory_bytes: u64,
    /// The models in memory: local, then the gateway's, then the Ollama daemon's.
    pub residents: Vec<Resident>,
    /// The port a running gateway answered on, if any.
    pub gateway_port: Option<u16>,
    /// Bytes on disk per store kind, largest first, each file counted once
    /// as `hedos scan` counts it. `None` until the first count finishes.
    pub disk_by_store: Option<Vec<(String, i64)>>,
    /// What the gateway has served, from its audit log.
    pub activity: Activity,
    /// When these facts were read, in Unix milliseconds; ages are measured
    /// from here so the screen never needs a clock.
    pub collected_at_millis: i64,
}

impl Facts {
    /// Gather the facts for `records` from `session`, the gateway probe, the
    /// audit `entries`, and the disk figures `disk` last counted.
    pub async fn collect(
        session: &Session,
        records: &[ModelRecord],
        entries: &[GatewayAuditEntry],
        disk: &DiskCount,
    ) -> Self {
        let now = now_millis();
        let loaded = residency::loaded(session, records).await;
        let disk = disk.figures(records).await;
        Self {
            memory_bytes: machine::memory_budget_bytes(),
            residents: loaded.residents,
            gateway_port: loaded.gateway_port,
            disk_by_store: disk,
            activity: Activity::from_entries(entries, now),
            collected_at_millis: now,
        }
    }

    /// Bytes held in memory, all holders together.
    pub fn resident_bytes(&self) -> i64 {
        self.residents.iter().map(|resident| resident.bytes).sum()
    }

    /// The resident entry for `id`, if it is loaded.
    pub fn resident(&self, id: &str) -> Option<&Resident> {
        self.residents.iter().find(|resident| resident.id == id)
    }

    /// Whether `id` is loaded, by any holder.
    pub fn is_warm(&self, id: &str) -> bool {
        self.resident(id).is_some()
    }

    /// Bytes not held by any resident.
    pub fn free_bytes(&self) -> i64 {
        self.memory_bytes as i64 - self.resident_bytes()
    }

    /// Bytes on disk across every store, once a count has finished.
    pub fn disk_bytes(&self) -> Option<i64> {
        self.disk_by_store
            .as_ref()
            .map(|stores| stores.iter().map(|(_, bytes)| bytes).sum())
    }
}

/// How long a refresh waits for the disk count it started before it shows
/// the last figures instead.
const DISK_WAIT: Duration = Duration::from_millis(500);

/// Counts the shelf's bytes on disk for the machine pane, one count at a
/// time. Counting stats every model file, which a stalled mount can hold up
/// for as long as it likes, so a refresh never waits on it past
/// [`DISK_WAIT`]: it shows the last figures a count finished with, and starts
/// no new count while one is still running, so a stall holds up one count
/// however many refreshes pass. Each count runs on a thread of its own that
/// leaving the shelf never waits for, and that thread has a `hedos
/// disk-count` process do the stats: a process with a thread stuck in a stat
/// on such a mount cannot finish exiting, so the child is the one left
/// behind, not the shelf.
pub struct DiskCount {
    count: Counter,
    wait: Duration,
    state: Arc<Mutex<DiskState>>,
}

/// Bytes on disk per store kind, largest first.
type Figures = Vec<(String, i64)>;

/// What counts the bytes on disk per store of a shelf, or `None` when the
/// count failed.
type Counter = Arc<dyn Fn(&[ModelRecord]) -> Option<Figures> + Send + Sync>;

#[derive(Default)]
struct DiskState {
    last: Option<Figures>,
    running: bool,
}

impl Default for DiskCount {
    fn default() -> Self {
        Self::new(Arc::new(count_apart), DISK_WAIT)
    }
}

impl DiskCount {
    fn new(count: Counter, wait: Duration) -> Self {
        Self {
            count,
            wait,
            state: Arc::new(Mutex::new(DiskState::default())),
        }
    }

    /// The bytes on disk per store of `records`: a count started now when
    /// none is running and it finishes in time, else the last one that
    /// finished, else `None` while the first is still counting.
    pub async fn figures(&self, records: &[ModelRecord]) -> Option<Figures> {
        if let Some(done) = self.start(records)
            && let Ok(Ok(Some(figures))) = tokio::time::timeout(self.wait, done).await
        {
            return Some(figures);
        }
        self.lock().last.clone()
    }

    /// Start a count of `records` on a thread of its own, unless one is
    /// running: the receiver hears what it found.
    fn start(
        &self,
        records: &[ModelRecord],
    ) -> Option<tokio::sync::oneshot::Receiver<Option<Figures>>> {
        {
            let mut state = self.lock();
            if state.running {
                return None;
            }
            state.running = true;
        }
        let (found, done) = tokio::sync::oneshot::channel();
        let count = Arc::clone(&self.count);
        let running = Running {
            state: Arc::clone(&self.state),
            found: Some(found),
        };
        let shelf = records.to_vec();
        // A thread that cannot be started drops `running`, which marks the
        // count finished again; the lock is not held here for that reason.
        std::thread::Builder::new()
            .name("disk-count".to_owned())
            .spawn(move || running.finish(count(&shelf)))
            .ok()
            .map(|_| done)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, DiskState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A running count's hold on the state and on the refresh waiting for it.
/// Dropped, as when the count panics, it marks the count finished before the
/// refresh hears of it, so the next refresh is free to start another.
struct Running {
    state: Arc<Mutex<DiskState>>,
    found: Option<tokio::sync::oneshot::Sender<Option<Figures>>>,
}

impl Running {
    /// Keep what the count found, when it found anything, and hand it to
    /// the refresh waiting for it.
    fn finish(mut self, figures: Option<Figures>) {
        {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(figures) = &figures {
                state.last = Some(figures.clone());
            }
            state.running = false;
        }
        if let Some(found) = self.found.take() {
            let _ = found.send(figures);
        }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        // After `finish` the flag may already belong to the next count.
        if self.found.is_some() {
            self.state
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .running = false;
        }
    }
}

/// Count the bytes on disk per store of `records` in a `hedos disk-count`
/// process, handing it the records on stdin. `None` when the process cannot
/// be started or says nothing that reads as figures.
fn count_apart(records: &[ModelRecord]) -> Option<Figures> {
    let input = serde_json::to_vec(records).ok()?;
    let mut child = std::process::Command::new(std::env::current_exe().ok()?)
        .arg("disk-count")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let written = child
        .stdin
        .take()
        .map(|mut stdin| std::io::Write::write_all(&mut stdin, &input));
    let output = child.wait_with_output().ok()?;
    if !matches!(written, Some(Ok(()))) || !output.status.success() {
        return None;
    }
    serde_json::from_slice(&output.stdout).ok()
}

/// Bytes on disk per store kind, largest first, ties by name: each file
/// counted once by device and inode, so two Ollama tags over one blob or a
/// hard link add nothing after the first. A record whose weights are gone
/// holds no disk. `hedos disk-count` prints these.
pub(crate) fn disk_by_store(records: &[ModelRecord]) -> Figures {
    largest_first(
        discovery::disk_by_store(records)
            .into_iter()
            .map(|(kind, bytes)| (kind.as_str().to_owned(), bytes))
            .collect(),
    )
}

fn largest_first(mut stores: Figures) -> Figures {
    stores.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    stores
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::support::residency::Holder;
    use kernel::records::{Capability, Modality, ModelSource, ModelState, SourceKind};

    fn record(name: &str, kind: SourceKind, footprint_bytes: Option<i64>) -> ModelRecord {
        let mut record = ModelRecord::new(
            name,
            Modality::text(),
            vec![Capability::chat()],
            ModelSource::new(kind, name),
        );
        record.footprint_bytes = footprint_bytes;
        record
    }

    #[test]
    fn disk_is_summed_per_store_largest_first() {
        let records = [
            record("a", SourceKind::ollama(), Some(1)),
            record("b", SourceKind::huggingface_cache(), Some(5)),
            record("c", SourceKind::ollama(), Some(2)),
            record("d", SourceKind::file(), None),
        ];
        let mut gone = record("e", SourceKind::ollama(), Some(100));
        gone.state = ModelState::Missing;
        let records = [
            records[0].clone(),
            records[1].clone(),
            records[2].clone(),
            records[3].clone(),
            gone,
        ];
        let stores = disk_by_store(&records);
        assert_eq!(stores[0].0, "huggingface-cache");
        assert_eq!(stores[0].1, 5);
        assert_eq!(stores[1], ("ollama".to_owned(), 3));
        assert_eq!(stores[2], ("file".to_owned(), 0));
    }

    /// A count that stalls, as a stat on a mount that stopped answering
    /// does, until `release` is sent, counting each start in `starts`.
    fn stalled(
        figures: Vec<(String, i64)>,
    ) -> (
        Counter,
        std::sync::mpsc::Sender<()>,
        Arc<std::sync::atomic::AtomicUsize>,
    ) {
        let (release, stall) = std::sync::mpsc::channel::<()>();
        let stall = Mutex::new(stall);
        let starts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let started = Arc::clone(&starts);
        let count: Counter = Arc::new(move |_: &[ModelRecord]| {
            started.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let _ = stall.lock().unwrap_or_else(PoisonError::into_inner).recv();
            Some(figures.clone())
        });
        (count, release, starts)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_stalled_disk_count_never_holds_a_refresh_or_runs_twice() {
        let (count, release, starts) = stalled(vec![("ollama".to_owned(), 7)]);
        let disk = DiskCount::new(count, Duration::from_millis(50));
        let records = [record("a", SourceKind::ollama(), Some(3))];
        let before = std::time::Instant::now();
        assert_eq!(disk.figures(&records).await, None, "still counting");
        assert_eq!(disk.figures(&records).await, None);
        assert!(before.elapsed() < Duration::from_secs(2));
        assert_eq!(starts.load(std::sync::atomic::Ordering::SeqCst), 1);

        release.send(()).unwrap();
        let mut figures = None;
        for _ in 0..200 {
            figures = disk.figures(&records).await;
            if figures.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(
            figures,
            Some(vec![("ollama".to_owned(), 7)]),
            "the last count is kept"
        );
    }

    #[test]
    fn leaving_never_waits_for_a_stalled_disk_count() {
        let (count, release, starts) = stalled(vec![("file".to_owned(), 1)]);
        let disk = DiskCount::new(count, Duration::from_millis(20));
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        assert_eq!(runtime.block_on(disk.figures(&[])), None);
        let before = std::time::Instant::now();
        drop(runtime);
        assert!(
            before.elapsed() < Duration::from_secs(1),
            "dropping the runtime waited on the count"
        );
        assert_eq!(starts.load(std::sync::atomic::Ordering::SeqCst), 1);
        release.send(()).unwrap();
    }

    #[tokio::test]
    async fn a_disk_count_that_finishes_in_time_is_shown_at_once() {
        let disk = DiskCount::new(
            Arc::new(|_: &[ModelRecord]| Some(vec![("file".to_owned(), 9)])),
            Duration::from_secs(5),
        );
        assert_eq!(disk.figures(&[]).await, Some(vec![("file".to_owned(), 9)]));
    }

    #[tokio::test]
    async fn a_failed_or_panicking_count_leaves_the_next_one_free_to_run() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let called = Arc::clone(&calls);
        let disk = DiskCount::new(
            Arc::new(move |_: &[ModelRecord]| {
                match called.fetch_add(1, std::sync::atomic::Ordering::SeqCst) {
                    0 => Some(vec![("file".to_owned(), 4)]),
                    1 => None,
                    2 => panic!("a count that panics"),
                    _ => Some(vec![("file".to_owned(), 5)]),
                }
            }),
            Duration::from_secs(5),
        );
        assert_eq!(disk.figures(&[]).await, Some(vec![("file".to_owned(), 4)]));
        assert_eq!(
            disk.figures(&[]).await,
            Some(vec![("file".to_owned(), 4)]),
            "a failed count keeps the last figures"
        );
        assert_eq!(disk.figures(&[]).await, Some(vec![("file".to_owned(), 4)]));
        assert_eq!(disk.figures(&[]).await, Some(vec![("file".to_owned(), 5)]));
    }

    #[cfg(unix)]
    #[test]
    fn disk_counts_a_file_two_records_reach_once() {
        let dir = std::env::temp_dir().join(format!(
            "hedos-facts-disk-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.gguf");
        let linked = dir.join("b.gguf");
        let own = dir.join("c.gguf");
        std::fs::write(&file, [1u8; 700]).unwrap();
        std::fs::write(&own, [2u8; 200]).unwrap();
        let _ = std::fs::remove_file(&linked);
        std::fs::hard_link(&file, &linked).unwrap();
        let at = |path: &std::path::Path| {
            let mut record = record("m", SourceKind::file(), Some(700));
            record.source = ModelSource::new(SourceKind::file(), &path.to_string_lossy());
            record
        };
        let mut studio = at(&linked);
        studio.source.kind = SourceKind::lm_studio();
        let stores = disk_by_store(&[at(&file), studio, at(&own)]);
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(
            stores,
            vec![("lm-studio".to_owned(), 700), ("file".to_owned(), 200)],
            "the hard link counts under the first store, lm-studio"
        );
    }

    #[test]
    fn the_disk_total_still_counts_every_quant() {
        let mut multi = record("multi", SourceKind::huggingface_cache(), Some(300));
        multi.serving_bytes = Some(100);
        assert_eq!(
            disk_by_store(&[multi]),
            vec![("huggingface-cache".to_owned(), 300)]
        );
    }

    #[test]
    fn totals_add_up() {
        let facts = Facts {
            memory_bytes: 0,
            residents: vec![
                Resident {
                    id: "a".into(),
                    name: "a".into(),
                    bytes: 10,
                    holder: Holder::Local,
                    expires_at_millis: None,
                },
                Resident {
                    id: "b".into(),
                    name: "b".into(),
                    bytes: 5,
                    holder: Holder::Gateway,
                    expires_at_millis: None,
                },
            ],
            gateway_port: None,
            disk_by_store: Some(vec![("ollama".into(), 7), ("file".into(), 3)]),
            activity: Activity::default(),
            collected_at_millis: 0,
        };
        assert_eq!(facts.resident_bytes(), 15);
        assert_eq!(facts.disk_bytes(), Some(10));
        assert_eq!(Facts::default().disk_bytes(), None);
        assert_eq!(facts.resident("b").map(|r| r.holder), Some(Holder::Gateway));
    }

    fn entry(model: &str, outcome: &str, ts_millis: i64) -> GatewayAuditEntry {
        GatewayAuditEntry {
            ts_millis,
            client: None,
            client_name: None,
            method: "POST".to_owned(),
            route: "/api/chat".to_owned(),
            model: Some(model.to_owned()),
            capability: Some("chat".to_owned()),
            outcome: outcome.to_owned(),
            status: if outcome == "ok" { 200 } else { 500 },
            duration_ms: 10,
            detail: None,
        }
    }

    #[test]
    fn a_day_of_requests_lands_in_hourly_buckets() {
        let now = 100 * DAY_MILLIS;
        let entries = [
            entry("m", "ok", now - 10),
            entry("m", "ok", now - 2 * HOUR_MILLIS - 1),
            entry("m", "error", now - 3 * HOUR_MILLIS),
            entry("m", "ok", now - 2 * DAY_MILLIS),
        ];
        let activity = Activity::from_entries(&entries, now);
        let model = &activity.models["m"];
        assert_eq!(model.requests, 2);
        assert_eq!(model.hourly[HOURS - 1], 1);
        assert_eq!(model.hourly[HOURS - 3], 1);
        assert_eq!(model.hourly.iter().sum::<u32>(), 2);
        assert_eq!(activity.pulse[PULSE_SLOTS - 1], 1);
        assert_eq!(activity.pulse[PULSE_SLOTS - 9], 1, "two hours back");
        assert_eq!(activity.pulse.iter().sum::<u32>(), 2);
        assert_eq!(model.last_seen_millis, now - 10);
        assert_eq!(activity.requests_last_minute, 1);
    }

    #[test]
    fn an_old_model_keeps_its_last_seen_but_no_recent_counts() {
        let now = 100 * DAY_MILLIS;
        let activity = Activity::from_entries(&[entry("m", "ok", now - 3 * DAY_MILLIS)], now);
        let model = &activity.models["m"];
        assert_eq!(model.requests, 0);
        assert_eq!(model.last_seen_millis, now - 3 * DAY_MILLIS);
    }
}
