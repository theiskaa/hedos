//! Keeping hedos itself current: the newest release, read from GitHub, and the
//! line a command ends with when a newer release is out. The look for a newer
//! release runs at most once a day, in a detached `hedos upgrade-check`
//! process, so no command waits on the network for it.

use std::io::IsTerminal;
use std::path::PathBuf;
use std::time::Duration;

use kernel::upgrade::{UpgradeCheck, Version};
use runtime::boot::HedosDirs;
use runtime::settings::SettingsStore;

use crate::support::output::Out;

/// The repository the releases are published from.
pub const REPO_URL: &str = env!("CARGO_PKG_REPOSITORY");

/// The environment variable that turns the daily look and its notice off.
const OPT_OUT_ENV: &str = "HEDOS_NO_UPDATE_CHECK";

/// How long a look for the newest release may take.
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(10);

/// The version of this binary.
pub fn current() -> Option<Version> {
    Version::parse(env!("CARGO_PKG_VERSION"))
}

/// The data dir the last look is remembered in.
pub fn data_dir() -> PathBuf {
    HedosDirs::detect().data
}

/// An HTTP client that names hedos and its version, as GitHub asks of
/// callers. A download follows redirects (release assets are served from
/// elsewhere) and has no overall time limit, only one on connecting.
pub fn client(download: bool) -> Result<reqwest::Client, String> {
    let builder =
        reqwest::Client::builder().user_agent(concat!("hedos/", env!("CARGO_PKG_VERSION")));
    if download {
        builder.connect_timeout(LOOKUP_TIMEOUT)
    } else {
        builder
            .redirect(reqwest::redirect::Policy::none())
            .timeout(LOOKUP_TIMEOUT)
    }
    .build()
    .map_err(|error| error.to_string())
}

/// The newest release's tag, read from where `releases/latest` redirects
/// rather than from the API, which allows a caller only 60 requests an hour.
/// Drafts and pre-releases are never `latest`.
pub async fn latest_tag() -> Result<String, String> {
    let url = format!("{REPO_URL}/releases/latest");
    let response = client(false)?
        .get(&url)
        .send()
        .await
        .map_err(|error| error.to_string())?;
    let status = response.status();
    let location = response
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .filter(|_| status.is_redirection())
        .ok_or_else(|| format!("{url} answered {status} without naming a release"))?;
    tag_of(location).ok_or_else(|| format!("{url} pointed at {location}, which names no release"))
}

/// The release tag at the end of a `.../releases/tag/<tag>` location.
fn tag_of(location: &str) -> Option<String> {
    let (_, tag) = location.rsplit_once("/releases/tag/")?;
    let tag = tag.trim_end_matches('/');
    Version::parse(tag).map(|_| tag.to_owned())
}

/// Look up the newest release and remember it, returning its tag.
pub async fn record_latest() -> Result<String, String> {
    let tag = latest_tag().await?;
    let check = UpgradeCheck {
        checked_at: kernel::time::now_millis(),
        latest: Some(tag.clone()),
    };
    // A look that cannot be remembered still answered; the next command
    // simply looks again.
    let _ = check.save(&data_dir());
    Ok(tag)
}

/// Whether this run looks for a newer release and says when one is out: only
/// for a person at a terminal, never under `--json`, and not when turned off
/// by `HEDOS_NO_UPDATE_CHECK` or `advanced.check_for_updates`.
pub fn notices_on(out: &Out) -> bool {
    !out.is_json()
        && std::io::stderr().is_terminal()
        && !opted_out(std::env::var(OPT_OUT_ENV).ok().as_deref())
        && SettingsStore::discover().load().advanced.check_for_updates
}

/// Whether `value`, the opt-out variable, asks for the look to stay off: any
/// value but empty, `0` or `false` does.
fn opted_out(value: Option<&str>) -> bool {
    value.map(str::trim).is_some_and(|value| {
        !value.is_empty() && value != "0" && !value.eq_ignore_ascii_case("false")
    })
}

/// Start a look for a newer release in the background when the last one is a
/// day old. Call this from inside a tokio runtime.
pub fn refresh_in_background() {
    // Claimed before the look starts, so the commands run while it is under
    // way do not start looks of their own, and a look that fails waits a day
    // like one that answered.
    if UpgradeCheck::claim(&data_dir(), kernel::time::now_millis()) {
        let _ = spawn_check();
    }
}

#[cfg(unix)]
fn spawn_check() -> std::io::Result<()> {
    let mut child = tokio::process::Command::new(std::env::current_exe()?)
        .arg("upgrade-check")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0)
        .kill_on_drop(false)
        .spawn()?;
    // Reaped while this process lives, so a long `serve` collects it; a
    // shorter command leaves it to be reparented.
    tokio::spawn(async move {
        let _ = child.wait().await;
    });
    Ok(())
}

#[cfg(not(unix))]
fn spawn_check() -> std::io::Result<()> {
    Ok(())
}

/// Say, on stderr, that a newer release is out, when the last look found one.
pub fn notice(out: &Out) {
    let (Some(current), Some(check)) = (current(), UpgradeCheck::load(&data_dir())) else {
        return;
    };
    if let Some(latest) = check.newer_than(&current) {
        out.err(&available(&latest, &current));
    }
}

/// The line that says `latest` is out.
pub fn available(latest: &Version, current: &Version) -> String {
    format!("\nhedos {latest} is available (you have {current}). Run `hedos upgrade` to update.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_release_tag_is_read_from_the_redirect() {
        assert_eq!(
            tag_of("https://github.com/theiskaa/hedos/releases/tag/v1.5.2").as_deref(),
            Some("v1.5.2")
        );
        assert_eq!(tag_of("https://github.com/theiskaa/hedos/releases"), None);
        assert_eq!(
            tag_of("https://github.com/theiskaa/hedos/releases/tag/nightly"),
            None
        );
    }

    #[test]
    fn only_a_set_opt_out_turns_the_look_off() {
        assert!(!opted_out(None));
        assert!(!opted_out(Some("")));
        assert!(!opted_out(Some("0")));
        assert!(!opted_out(Some("false")));
        assert!(opted_out(Some("1")));
        assert!(opted_out(Some("yes")));
    }
}
