//! Where a model is loaded, across every place one can be: this process's
//! governor, a running gateway, and the Ollama daemon. Both the commands and
//! the UI ask the same questions here, so they never disagree.

use std::collections::{BTreeMap, HashSet};
use std::time::{Duration, Instant};

use kernel::records::byte_format::BYTES_PER_MIB;
use kernel::records::{Capability, JsonValue, ModelRecord};

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
    /// Seconds until the idle unload fires, measured from `now` in Unix
    /// milliseconds, if one is armed and still ahead. A deadline `now` has
    /// passed is a snapshot gone stale, not a countdown.
    pub fn expires_in_seconds_at(&self, now: i64) -> Option<i64> {
        self.expires_at_millis
            .map(|deadline| (deadline - now) / 1000)
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

/// The question a judge is warmed with: the smallest one that is still a
/// question, so the load is paid for and nothing is asked of the answer.
const WARM_QUESTION: &str =
    r#"{"state":"","questions":{"warm":{"type":"noul","instructions":"ready"}}}"#;

/// How long a gateway warm may take: a cold model behind a sidecar can spend
/// minutes loading, and the caller asked for exactly that.
const GATEWAY_WARM_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// The body that loads `record` through a gateway's Ollama chat endpoint, or
/// `None` for a model that answers something other than a conversation. The
/// content is [`warm_request`]'s, so a judge is asked a question here too.
pub(crate) fn gateway_warm_body(record: &ModelRecord) -> Option<serde_json::Value> {
    let (_, payload) = warm_request(record)?;
    let content = payload
        .as_object()
        .and_then(|fields| fields.get("messages"))
        .and_then(JsonValue::as_array)
        .and_then(|messages| messages.first())
        .and_then(JsonValue::as_object)
        .and_then(|message| message.get("content"))
        .and_then(JsonValue::as_str)?;
    let mut body = serde_json::json!({
        "model": record.id,
        "messages": [{ "role": "user", "content": content }],
        "stream": false,
    });
    // A reply cap only where the local probe set one. A judge answers in a
    // single forward pass, and a runtime that honors no sampling parameters
    // refuses the request outright rather than ignoring the cap.
    if payload
        .as_object()
        .is_some_and(|fields| fields.contains_key("max_tokens"))
    {
        body["options"] = serde_json::json!({ "num_predict": 1 });
    }
    Some(body)
}

/// Load `record` on the gateway at `port`, which is where it is served while one
/// is running. Warming this process instead would load the model here, report
/// success, and leave the gateway as cold as it was.
pub(crate) async fn warm_via_gateway(record: &ModelRecord, port: u16) -> Result<String, String> {
    let body = gateway_warm_body(record)
        .ok_or_else(|| format!("{} can't be warmed", record.display_name()))?;
    let response = reqwest::Client::builder()
        .timeout(GATEWAY_WARM_TIMEOUT)
        .build()
        .map_err(|error| error.to_string())?
        .post(format!("http://127.0.0.1:{port}/api/chat"))
        .json(&body)
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if response.status().is_success() {
        return Ok(format!("warm on the gateway :{port}"));
    }
    let status = response.status();
    let reason = response.text().await.unwrap_or_default();
    Err(if reason.is_empty() {
        format!("gateway answered {status}")
    } else {
        reason
    })
}

/// The smallest request that loads `record`: a trivial question for a judge, a
/// one-token chat/complete, or a dot of speech. `None` if the model serves none
/// of those.
pub(crate) fn warm_request(record: &ModelRecord) -> Option<(Capability, JsonValue)> {
    let has = |cap: &Capability| record.capabilities.contains(cap);
    // A judge refuses prose, so its probe is the smallest well-formed question
    // rather than "hi". Checked before chat because a judge reached over the
    // chat wire declares both, and "hi" is exactly what it would reject.
    if has(&Capability::judge()) {
        let mut payload = BTreeMap::new();
        payload.insert(
            "messages".to_owned(),
            JsonValue::Array(vec![payload::message("user", WARM_QUESTION)]),
        );
        Some((Capability::judge(), JsonValue::Object(payload)))
    } else if has(&Capability::chat()) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use kernel::records::{Modality, ModelSource, SourceKind};

    fn model(capabilities: Vec<Capability>) -> ModelRecord {
        ModelRecord::new(
            "m",
            Modality::text(),
            capabilities,
            ModelSource::new(SourceKind::folder(), "/models/m"),
        )
    }

    /// The probe text a warm request carries, whatever shape it took.
    fn probe(record: &ModelRecord) -> (Capability, String) {
        let (capability, payload) = warm_request(record).expect("a warmable model");
        let fields = payload.as_object().expect("an object payload").clone();
        let text = match fields.get("messages") {
            Some(JsonValue::Array(messages)) => messages
                .first()
                .and_then(JsonValue::as_object)
                .and_then(|message| message.get("content"))
                .and_then(JsonValue::as_str)
                .unwrap_or_default()
                .to_owned(),
            _ => fields
                .get("prompt")
                .or_else(|| fields.get("text"))
                .and_then(JsonValue::as_str)
                .unwrap_or_default()
                .to_owned(),
        };
        (capability, text)
    }

    #[test]
    fn a_judge_is_warmed_with_a_question_rather_than_a_greeting() {
        // Both capabilities, which is what a judge served over the chat wire
        // declares: the greeting is exactly what such a model refuses.
        let (capability, text) = probe(&model(vec![Capability::chat(), Capability::judge()]));
        assert_eq!(capability, Capability::judge());
        assert!(text.contains("\"questions\""), "{text}");
        assert!(text.contains("\"noul\""), "{text}");
    }

    #[test]
    fn a_chat_model_is_still_warmed_with_a_greeting() {
        let (capability, text) = probe(&model(vec![Capability::chat()]));
        assert_eq!(capability, Capability::chat());
        assert_eq!(text, "hi");
    }

    #[test]
    fn the_gateway_warm_body_carries_the_same_probe_as_a_local_one() {
        // The gateway posts a conversation whatever the model is, so a judge's
        // question has to travel as the message content or it warms nothing.
        let judge = model(vec![Capability::chat(), Capability::judge()]);
        let body = gateway_warm_body(&judge).expect("a warmable model");
        let content = body["messages"][0]["content"].as_str().expect("content");
        assert!(content.contains("\"questions\""), "{content}");
        assert_eq!(body["model"], judge.id);

        assert!(body.get("options").is_none(), "a judge has no reply budget");

        let chatter = model(vec![Capability::chat()]);
        let chat_body = gateway_warm_body(&chatter).expect("body");
        assert_eq!(chat_body["messages"][0]["content"], "hi");
        assert_eq!(chat_body["options"]["num_predict"], 1);
    }

    #[test]
    fn a_model_that_serves_none_of_them_cannot_be_warmed() {
        assert!(warm_request(&model(vec![Capability::image()])).is_none());
    }
}
