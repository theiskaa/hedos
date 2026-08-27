//! Where a model is loaded, across every place one can be: this process's
//! governor, a running gateway, and the Ollama daemon. Both the commands and
//! the UI ask the same questions here, so they never disagree.

use std::collections::{BTreeMap, HashSet};
use std::time::{Duration, Instant};

use kernel::records::byte_format::BYTES_PER_MIB;
use kernel::records::{Capability, JsonValue, ModelRecord};
use kernel::time::now_millis;

use crate::error::CliError;
use crate::support::ollama;
use crate::support::payload;
use crate::support::session::Session;

/// How long to give the Ollama daemon to let a model go after it agrees to;
/// it unloads after answering, so an immediate `/api/ps` still lists it.
const DAEMON_UNLOAD_GRACE: Duration = Duration::from_secs(5);
const DAEMON_POLL: Duration = Duration::from_millis(250);

/// Who holds a resident model in memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Holder {
    /// This process's own kernel.
    Local,
    /// A `hedos serve` on the configured port.
    Gateway,
    /// The Ollama daemon, which loads the models it serves itself.
    Daemon,
}

/// One model held in memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Resident {
    /// The record id.
    pub id: String,
    /// The display name.
    pub name: String,
    /// The footprint in bytes.
    pub bytes: i64,
    /// Who holds it.
    pub holder: Holder,
    /// When its idle unload fires, in Unix milliseconds, if a timer is armed.
    pub expires_at_millis: Option<i64>,
}

impl Resident {
    /// Seconds until the idle unload fires, if one is armed and still ahead.
    /// A deadline the clock has passed is a snapshot gone stale, not a countdown.
    pub fn expires_in_seconds(&self) -> Option<i64> {
        self.expires_at_millis
            .map(|deadline| (deadline - now_millis()) / 1000)
            .filter(|seconds| *seconds > 0)
    }
}

/// What is loaded right now and where the gateway answered, if it did.
pub(crate) struct Loaded {
    /// Residents in order of holder: local, the gateway's, the daemon's; a
    /// model counted once, by the first holder that reports it.
    pub residents: Vec<Resident>,
    /// The port a running gateway answered on.
    pub gateway_port: Option<u16>,
}

impl Loaded {
    /// The ids of every resident.
    pub fn ids(&self) -> HashSet<String> {
        self.residents
            .iter()
            .map(|resident| resident.id.clone())
            .collect()
    }
}

/// Everything loaded of `records`, from the governor, a running gateway, and
/// the Ollama daemon.
pub(crate) async fn loaded(session: &Session, records: &[ModelRecord]) -> Loaded {
    let by_id: BTreeMap<&str, &ModelRecord> = records
        .iter()
        .map(|record| (record.id.as_str(), record))
        .collect();
    let mut residents: Vec<Resident> = session
        .kernel
        .resident_models()
        .into_iter()
        .filter_map(|entry| {
            let id = entry.model_id?;
            Some(Resident {
                name: by_id
                    .get(id.as_str())
                    .map_or(entry.name, |record| record.display_name().to_owned()),
                id,
                bytes: entry.footprint_mb * BYTES_PER_MIB,
                holder: Holder::Local,
                expires_at_millis: entry.expires_at_millis,
            })
        })
        .collect();
    let mut seen: HashSet<String> = residents.iter().map(|r| r.id.clone()).collect();

    let live = session.live_gateway().await;
    if let Some(live) = &live {
        for resident in &live.residents {
            let Some(record) = by_id.get(resident.id.as_str()) else {
                continue;
            };
            if seen.insert(resident.id.clone()) {
                residents.push(Resident {
                    id: resident.id.clone(),
                    name: record.display_name().to_owned(),
                    bytes: resident.size,
                    holder: Holder::Gateway,
                    expires_at_millis: resident.expires_at_millis(),
                });
            }
        }
    }
    if let Some(daemon) = ollama::residents().await {
        for record in records {
            let Some(held) = ollama::held(&daemon, record) else {
                continue;
            };
            if seen.insert(record.id.clone()) {
                residents.push(Resident {
                    id: record.id.clone(),
                    name: record.display_name().to_owned(),
                    bytes: held.size,
                    holder: Holder::Daemon,
                    expires_at_millis: held.expires_at_millis(),
                });
            }
        }
    }
    Loaded {
        residents,
        gateway_port: live.map(|live| live.port),
    }
}

/// Whether `record` is loaded after a warm: tracked by this process's
/// governor, or held by the Ollama daemon, which loads its models itself.
pub(crate) async fn is_resident(session: &Session, record: &ModelRecord) -> Result<bool, String> {
    if session.kernel.governor().is_resident(&record.id) {
        return Ok(true);
    }
    ollama::holds_now(record).await
}

/// The word for a finished load, by whether residency is tracked for it.
pub(crate) fn residency_outcome(resident: bool) -> &'static str {
    if resident {
        "warm"
    } else {
        "loaded (residency not tracked for this runtime)"
    }
}

/// The smallest request that loads `record`: a one-token chat/complete, or a
/// dot of speech. `None` if the model serves none of those.
pub(crate) fn warm_request(record: &ModelRecord) -> Option<(Capability, JsonValue)> {
    let has = |cap: &Capability| record.capabilities.contains(cap);
    if has(&Capability::chat()) {
        let mut payload = BTreeMap::new();
        payload.insert(
            "messages".to_owned(),
            JsonValue::Array(vec![payload::message("user", "hi")]),
        );
        payload.insert("max_tokens".to_owned(), JsonValue::Int(1));
        Some((Capability::chat(), JsonValue::Object(payload)))
    } else if has(&Capability::complete()) {
        let mut payload = BTreeMap::new();
        payload.insert("prompt".to_owned(), JsonValue::String("hi".to_owned()));
        payload.insert("max_tokens".to_owned(), JsonValue::Int(1));
        Some((Capability::complete(), JsonValue::Object(payload)))
    } else if has(&Capability::speak()) {
        let mut payload = BTreeMap::new();
        payload.insert("text".to_owned(), JsonValue::String(".".to_owned()));
        Some((Capability::speak(), JsonValue::Object(payload)))
    } else {
        None
    }
}

/// Evict `record` from wherever it is loaded: this process's governor, or the
/// Ollama daemon when the daemon holds it. Whether it is still resident after.
pub(crate) async fn unload_anywhere(
    session: &Session,
    record: &ModelRecord,
) -> Result<bool, CliError> {
    session
        .kernel
        .governor()
        .residency()
        .unload_now(&record.id)
        .await;
    let governor_holds = session.kernel.governor().is_resident(&record.id);
    let Some(tag) = ollama::tag_of(record) else {
        return Ok(governor_holds);
    };
    if !ollama::holds_now(record).await.map_err(CliError::new)? {
        return Ok(governor_holds);
    }
    ollama::unload(tag).await.map_err(CliError::new)?;
    let deadline = Instant::now() + DAEMON_UNLOAD_GRACE;
    while ollama::holds_now(record).await.map_err(CliError::new)? {
        if Instant::now() >= deadline {
            return Ok(true);
        }
        tokio::time::sleep(DAEMON_POLL).await;
    }
    Ok(governor_holds)
}
