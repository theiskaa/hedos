//! The Ollama install provider: pulls a tag through a local Ollama daemon,
//! starting the daemon first if a binary is present but nothing is listening.
//! The `/api/pull` ndjson stream folds into install progress via the kernel's
//! [`Aggregator`](kernel::install::ollama_pull::Aggregator).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use kernel::install::ollama_pull::{Aggregator, Outcome};
use kernel::install::reference::ollama_install_tag;
use kernel::install::{
    InstallAvailability, InstallError, InstallPlan, InstallProviderId, InstallSearchHit,
    InstallStreamEvent,
};
use kernel::records::SourceKind;
use serde::Deserialize;
use tokio::sync::mpsc;

use super::provider::{InstallEventStream, InstallFuture, InstallProvider};

const DEFAULT_BASE_URL: &str = "http://127.0.0.1:11434";
const NOT_INSTALLED_HINT: &str = "Ollama isn't installed. Get it from ollama.com.";
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
/// Cap on connecting to the daemon, so a host that accepts then never answers
/// can't hang `send()` forever.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// A pull can be quiet for a long time while the server assembles a layer, so the
/// idle window is generous — but still bounded, so a wedged server can't hang us.
const PULL_IDLE_TIMEOUT: Duration = Duration::from_secs(60 * 60);
/// How long to wait for a non-200 error body before reporting with what we have.
const ERROR_BODY_TIMEOUT: Duration = Duration::from_secs(30);
/// Ceiling on the ndjson we'll read from one pull, so a misbehaving server can't
/// grow our buffers without bound (4 GiB).
const PULL_RESPONSE_CAP: usize = 4 << 30;
/// Ceiling on a single unterminated line (2 MiB).
const MAX_LINE_BYTES: usize = 2 << 20;
/// How much of a non-200 body to read before giving up and reporting the error.
const ERROR_BODY_CAP: usize = 64 * 1024;

/// Installs models by pulling tags through a local Ollama daemon.
pub struct OllamaInstallProvider {
    base_url: String,
    client: reqwest::Client,
    environment: HashMap<String, String>,
}

impl OllamaInstallProvider {
    /// A provider pointed at the default local Ollama (`127.0.0.1:11434`), reading
    /// the process environment for `PATH`/`OLLAMA_MODELS`/`HOME`.
    pub fn new() -> Self {
        Self::with_config(DEFAULT_BASE_URL, std::env::vars().collect())
    }

    /// A provider pointed at `base_url` with an explicit environment (for tests).
    pub fn with_config(base_url: impl Into<String>, environment: HashMap<String, String>) -> Self {
        let client = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            client,
            environment,
        }
    }

    /// Whether `reference` looks like an Ollama tag (so this provider should claim
    /// a bare, un-prefixed reference the user typed).
    pub fn is_tag_shaped(reference: &str) -> bool {
        kernel::install::reference::is_ollama_tag_shaped(reference)
    }
}

impl Default for OllamaInstallProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl InstallProvider for OllamaInstallProvider {
    fn id(&self) -> InstallProviderId {
        InstallProviderId::ollama()
    }

    fn display_name(&self) -> &str {
        "Ollama"
    }

    fn source_kind(&self) -> SourceKind {
        SourceKind::ollama()
    }

    fn supports_search(&self) -> bool {
        false
    }

    fn availability(&self) -> InstallFuture<'_, InstallAvailability> {
        Box::pin(async move {
            if daemon_binary(&self.environment).is_some()
                || daemon_reachable(&self.client, &self.base_url).await
            {
                InstallAvailability::Ready
            } else {
                InstallAvailability::Unavailable {
                    hint: NOT_INSTALLED_HINT.to_owned(),
                }
            }
        })
    }

    fn search(
        &self,
        _query: &str,
        _limit: usize,
    ) -> InstallFuture<'_, Result<Vec<InstallSearchHit>, InstallError>> {
        Box::pin(async {
            Err(InstallError::ProviderUnavailable(
                "Ollama has no search. Pick from the catalog or enter a tag like gemma3:4b."
                    .to_owned(),
            ))
        })
    }

    fn plan(&self, reference: &str) -> InstallFuture<'_, Result<InstallPlan, InstallError>> {
        let reference = reference.to_owned();
        Box::pin(async move {
            let tag = ollama_install_tag(&reference)
                .ok_or_else(|| InstallError::ReferenceInvalid(reference.clone()))?;
            Ok(InstallPlan::new(
                InstallProviderId::ollama(),
                tag.clone(),
                tag,
                display_models_path(&self.environment),
            ))
        })
    }

    fn install(&self, plan: InstallPlan) -> InstallEventStream {
        let (tx, rx) = mpsc::channel(32);
        let client = self.client.clone();
        let base_url = self.base_url.clone();
        let environment = self.environment.clone();
        tokio::spawn(async move {
            if let Err(error) = run_pull(&client, &base_url, &environment, &plan, &tx).await {
                let _ = tx.send(Err(error)).await;
            }
        });
        rx
    }
}

/// Drive one pull to completion, emitting status/progress on `tx`. A send failure
/// means the consumer dropped the receiver (cancellation) — we stop quietly.
async fn run_pull(
    client: &reqwest::Client,
    base_url: &str,
    environment: &HashMap<String, String>,
    plan: &InstallPlan,
    tx: &mpsc::Sender<Result<InstallStreamEvent, InstallError>>,
) -> Result<(), InstallError> {
    if !daemon_reachable(client, base_url).await {
        if daemon_binary(environment).is_none() {
            return Err(InstallError::ProviderUnavailable(
                "Ollama stopped running. Start it again and retry.".to_owned(),
            ));
        }
        if tx
            .send(Ok(InstallStreamEvent::Status(
                "Starting Ollama…".to_owned(),
            )))
            .await
            .is_err()
        {
            return Ok(());
        }
        start_daemon(client, base_url, environment).await?;
    }

    let url = format!("{base_url}/api/pull");
    let body = serde_json::to_vec(&serde_json::json!({
        "model": plan.reference,
        "stream": true,
    }))
    .map_err(|error| InstallError::TransferFailed(error.to_string()))?;
    let mut response = match client
        .post(&url)
        .header("content-type", "application/json")
        .body(body)
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) if error.is_connect() => {
            return Err(InstallError::ProviderUnavailable(
                "Ollama isn't running. Start it with `ollama serve`.".to_owned(),
            ));
        }
        Err(error) => {
            return Err(InstallError::TransferFailed(format!(
                "pulling {}: {error}",
                plan.reference
            )));
        }
    };

    let status = response.status().as_u16();
    if status != 200 {
        let mut collected = Vec::new();
        while collected.len() < ERROR_BODY_CAP {
            // A stalled error body shouldn't hang us — take what arrives, then stop
            // on EOF, error, or timeout.
            match tokio::time::timeout(ERROR_BODY_TIMEOUT, response.chunk()).await {
                Ok(Ok(Some(bytes))) => collected.extend_from_slice(&bytes),
                _ => break,
            }
        }
        return Err(InstallError::TransferFailed(error_message(
            &collected,
            i64::from(status),
        )));
    }

    let mut aggregator = Aggregator::new();
    let mut buffer: Vec<u8> = Vec::new();
    let mut total_read = 0usize;
    loop {
        // Notice a dropped receiver (cancellation) even while parked on a read,
        // rather than only at the next send.
        let chunk = tokio::select! {
            biased;
            _ = tx.closed() => return Ok(()),
            result = tokio::time::timeout(PULL_IDLE_TIMEOUT, response.chunk()) => result,
        }
        .map_err(|_| {
            InstallError::TransferFailed(
                "ollama pull stalled: no data within the idle timeout".to_owned(),
            )
        })?;
        let bytes = match chunk {
            Ok(Some(bytes)) => bytes,
            Ok(None) => {
                // Fold a final line the server left unterminated (the trailing
                // buffer is yielded at EOF) — it may be the `success` line.
                if !buffer.is_empty() {
                    let text = String::from_utf8_lossy(&buffer);
                    match fold_line(&mut aggregator, text.trim_end_matches('\r'), tx).await? {
                        LineOutcome::Continue => {}
                        LineOutcome::Done | LineOutcome::Cancelled => return Ok(()),
                    }
                }
                break;
            }
            Err(error) => {
                return Err(InstallError::TransferFailed(format!(
                    "reading ollama pull: {error}"
                )));
            }
        };
        total_read = total_read.saturating_add(bytes.len());
        if total_read > PULL_RESPONSE_CAP {
            return Err(InstallError::TransferFailed(format!(
                "ollama sent a response larger than {PULL_RESPONSE_CAP} bytes"
            )));
        }
        buffer.extend_from_slice(&bytes);
        while let Some(newline) = buffer.iter().position(|&byte| byte == b'\n') {
            let line: Vec<u8> = buffer.drain(..=newline).collect();
            let text = String::from_utf8_lossy(&line[..line.len() - 1]);
            match fold_line(&mut aggregator, text.trim_end_matches('\r'), tx).await? {
                LineOutcome::Continue => {}
                LineOutcome::Done | LineOutcome::Cancelled => return Ok(()),
            }
        }
        if buffer.len() > MAX_LINE_BYTES {
            return Err(InstallError::TransferFailed(format!(
                "ollama sent a line larger than {MAX_LINE_BYTES} bytes"
            )));
        }
    }

    Err(InstallError::TransferFailed(
        "ollama ended the pull without reporting success".to_owned(),
    ))
}

/// What handling one pull line meant for the loop.
enum LineOutcome {
    /// Keep reading.
    Continue,
    /// The pull reported success.
    Done,
    /// The consumer dropped the receiver — stop quietly.
    Cancelled,
}

/// Fold one ndjson line and emit any resulting status/progress on `tx`.
async fn fold_line(
    aggregator: &mut Aggregator,
    line: &str,
    tx: &mpsc::Sender<Result<InstallStreamEvent, InstallError>>,
) -> Result<LineOutcome, InstallError> {
    let event = match aggregator.fold(line)? {
        Outcome::Ignored => return Ok(LineOutcome::Continue),
        Outcome::Success => return Ok(LineOutcome::Done),
        Outcome::Status(message) => InstallStreamEvent::Status(message),
        Outcome::Progress(progress) => InstallStreamEvent::Progress(progress),
    };
    if tx.send(Ok(event)).await.is_err() {
        return Ok(LineOutcome::Cancelled);
    }
    Ok(LineOutcome::Continue)
}

pub(crate) async fn daemon_reachable(client: &reqwest::Client, base_url: &str) -> bool {
    let url = format!("{base_url}/api/tags");
    match tokio::time::timeout(PROBE_TIMEOUT, client.get(&url).send()).await {
        Ok(Ok(response)) => response.status().as_u16() == 200,
        _ => false,
    }
}

/// Spawn `ollama serve` and wait (up to ~50s: 20 polls, each a 500 ms sleep plus a
/// reachability probe capped at [`PROBE_TIMEOUT`]) for it to answer.
pub(crate) async fn start_daemon(
    client: &reqwest::Client,
    base_url: &str,
    environment: &HashMap<String, String>,
) -> Result<(), InstallError> {
    let binary = daemon_binary(environment)
        .ok_or_else(|| InstallError::ProviderUnavailable(NOT_INSTALLED_HINT.to_owned()))?;
    let mut child = std::process::Command::new(&binary)
        .arg("serve")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|error| {
            InstallError::ProviderUnavailable(format!("couldn't start Ollama: {error}"))
        })?;

    let mut exited = false;
    for _ in 0..20 {
        tokio::time::sleep(Duration::from_millis(500)).await;
        if daemon_reachable(client, base_url).await {
            // Leave the daemon running — a `std::process::Child` drop doesn't kill it.
            return Ok(());
        }
        if matches!(child.try_wait(), Ok(Some(_))) {
            exited = true;
        }
    }
    // Reap the process we started but couldn't reach, so it doesn't linger as a
    // zombie under the long-lived parent (`try_wait` already reaped it if it exited).
    if !exited {
        let _ = child.kill();
    }
    let _ = child.wait();
    Err(InstallError::ProviderUnavailable(
        if exited {
            "Ollama quit as soon as it started. Try `ollama serve` in a terminal."
        } else {
            "Started Ollama but it never became reachable."
        }
        .to_owned(),
    ))
}

/// Find an `ollama` executable in the well-known locations, then on `PATH`.
pub(crate) fn daemon_binary(environment: &HashMap<String, String>) -> Option<PathBuf> {
    let home = home_dir(environment);
    let mut candidates = vec![
        PathBuf::from("/usr/local/bin/ollama"),
        PathBuf::from("/opt/homebrew/bin/ollama"),
        home.join(".local/bin/ollama"),
    ];
    if let Some(path) = environment.get("PATH") {
        candidates.extend(
            path.split(':')
                .filter(|entry| !entry.is_empty())
                .map(|dir| Path::new(dir).join("ollama")),
        );
    }
    candidates.into_iter().find(|path| is_executable_file(path))
}

fn is_executable_file(path: &Path) -> bool {
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

fn home_dir(environment: &HashMap<String, String>) -> PathBuf {
    environment
        .get("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .unwrap_or_default()
}

fn models_root(environment: &HashMap<String, String>) -> PathBuf {
    if let Some(custom) = environment
        .get("OLLAMA_MODELS")
        .filter(|value| !value.is_empty())
    {
        return expand_tilde(custom, &home_dir(environment));
    }
    home_dir(environment).join(".ollama/models")
}

fn expand_tilde(raw: &str, home: &Path) -> PathBuf {
    if let Some(rest) = raw.strip_prefix("~/") {
        home.join(rest)
    } else if raw == "~" {
        home.to_path_buf()
    } else {
        PathBuf::from(raw)
    }
}

/// The models directory to show the user, with the home prefix collapsed to `~`.
fn display_models_path(environment: &HashMap<String, String>) -> String {
    let home = home_dir(environment);
    let path = models_root(environment).to_string_lossy().into_owned();
    let home = home.to_string_lossy();
    if !home.is_empty() && (path == *home || path.starts_with(&format!("{home}/"))) {
        return format!("~{}", &path[home.len()..]);
    }
    path
}

/// Turn an error body/status into a user-facing message.
pub(crate) fn error_message(body: &[u8], code: i64) -> String {
    #[derive(Deserialize)]
    struct ErrorBody {
        error: Option<String>,
    }
    if let Ok(ErrorBody {
        error: Some(message),
    }) = serde_json::from_slice::<ErrorBody>(body)
        && !message.is_empty()
    {
        return format!("ollama: {message}");
    }
    format!("ollama returned HTTP {code}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    }

    #[test]
    fn models_root_prefers_the_ollama_models_override() {
        let environment = env(&[("HOME", "/home/me"), ("OLLAMA_MODELS", "/data/models")]);
        assert_eq!(models_root(&environment), PathBuf::from("/data/models"));
    }

    #[test]
    fn models_root_falls_back_to_the_home_dotfile() {
        let environment = env(&[("HOME", "/home/me")]);
        assert_eq!(
            models_root(&environment),
            PathBuf::from("/home/me/.ollama/models")
        );
    }

    #[test]
    fn a_tilde_override_expands_against_home() {
        let environment = env(&[("HOME", "/home/me"), ("OLLAMA_MODELS", "~/models")]);
        assert_eq!(models_root(&environment), PathBuf::from("/home/me/models"));
    }

    #[test]
    fn display_path_collapses_the_home_prefix() {
        let environment = env(&[("HOME", "/home/me")]);
        assert_eq!(display_models_path(&environment), "~/.ollama/models");
    }

    #[test]
    fn display_path_keeps_a_path_outside_home() {
        let environment = env(&[("HOME", "/home/me"), ("OLLAMA_MODELS", "/data/models")]);
        assert_eq!(display_models_path(&environment), "/data/models");
    }

    #[test]
    fn error_message_prefers_the_json_error_field() {
        assert_eq!(
            error_message(br#"{"error":"pull access denied"}"#, 403),
            "ollama: pull access denied"
        );
    }

    #[test]
    fn error_message_falls_back_to_the_status_code() {
        assert_eq!(error_message(b"not json", 500), "ollama returned HTTP 500");
        assert_eq!(
            error_message(br#"{"error":""}"#, 500),
            "ollama returned HTTP 500"
        );
    }

    #[test]
    fn daemon_binary_finds_an_executable_on_path() {
        let dir = std::env::temp_dir().join("hedos-ollama-bin-test");
        let _ = std::fs::create_dir_all(&dir);
        let binary = dir.join("ollama");
        std::fs::write(&binary, b"#!/bin/sh\n").expect("write");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755))
                .expect("chmod");
        }
        let environment = env(&[("PATH", dir.to_str().expect("path"))]);
        // On a machine without ollama in the well-known dirs this is our temp copy;
        // on one with a real ollama, `find` still returns *a* valid executable.
        assert!(daemon_binary(&environment).is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
