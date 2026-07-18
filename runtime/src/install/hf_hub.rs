//! The Hugging Face hub API: search for models, fetch a repo's metadata + file
//! listing, and build file-download URLs. Ports Swift `Install/HuggingFace/
//! HFHubAPI.swift` (over the injectable [`InstallTransport`], not `URLSession`).

use std::sync::Arc;

use kernel::install::file_selection::HFSibling;
use kernel::install::{InstallError, InstallProviderId, InstallSearchHit};
use serde::Deserialize;

use super::transport::{InstallRequest, InstallTransport};

const DEFAULT_BASE_URL: &str = "https://huggingface.co";

/// A repo's resolved metadata: its canonical id, commit sha, gated flag, and files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HFModelInfo {
    /// The canonical `org/name` repo id.
    pub repo: String,
    /// The resolved commit sha, if reported.
    pub sha: Option<String>,
    /// Whether the model is gated (needs a token).
    pub gated: bool,
    /// The repo's files.
    pub siblings: Vec<HFSibling>,
}

/// A client for the Hugging Face hub, over an injectable transport.
pub struct HFHubAPI {
    base_url: String,
    transport: Arc<dyn InstallTransport>,
    token: Option<String>,
}

impl HFHubAPI {
    /// A client against the public hub, using `transport`.
    pub fn new(transport: Arc<dyn InstallTransport>) -> Self {
        Self {
            base_url: DEFAULT_BASE_URL.to_owned(),
            transport,
            token: None,
        }
    }

    /// Point the client at a different base URL (e.g. a mock server in tests).
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into().trim_end_matches('/').to_owned();
        self
    }

    /// Attach a bearer token for gated/authenticated requests.
    pub fn with_token(mut self, token: Option<String>) -> Self {
        self.token = token.filter(|token| !token.is_empty());
        self
    }

    /// Search the hub for models matching `query`, most-downloaded first.
    pub async fn search(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<InstallSearchHit>, InstallError> {
        let mut url = self.parse_url(&format!("{}/api/models", self.base_url))?;
        url.query_pairs_mut()
            .append_pair("search", query)
            .append_pair("limit", &limit.to_string())
            .append_pair("sort", "downloads");
        let response = self
            .transport
            .fetch(self.authorized(InstallRequest::get(url.as_str())))
            .await?;
        if response.status != 200 {
            return Err(InstallError::TransferFailed(format!(
                "hugging face search returned HTTP {}",
                response.status
            )));
        }
        let hits: Vec<RawHit> = serde_json::from_slice(&response.body)
            .map_err(|error| InstallError::TransferFailed(format!("parsing search: {error}")))?;
        Ok(hits
            .into_iter()
            .map(|hit| {
                // The last non-empty path segment (Swift `split` drops empties, so a
                // trailing-slash id falls back to the segment before it).
                let name = hit
                    .id
                    .split('/')
                    .rfind(|part| !part.is_empty())
                    .unwrap_or(&hit.id)
                    .to_owned();
                InstallSearchHit {
                    provider: InstallProviderId::huggingface(),
                    reference: hit.id,
                    name,
                    downloads: hit.downloads,
                    likes: hit.likes,
                    updated_at: hit.last_modified.as_deref().and_then(parse_iso8601_millis),
                }
            })
            .collect())
    }

    /// Fetch `repo`'s metadata and file listing.
    pub async fn model_info(&self, repo: &str) -> Result<HFModelInfo, InstallError> {
        let mut url = self.parse_url(&format!("{}/api/models/{repo}", self.base_url))?;
        url.query_pairs_mut().append_pair("blobs", "true");
        let response = self
            .transport
            .fetch(self.authorized(InstallRequest::get(url.as_str())))
            .await?;
        match response.status {
            200 => {}
            401 | 403 => return Err(InstallError::AuthRequired(repo.to_owned())),
            404 => return Err(InstallError::ReferenceNotFound(repo.to_owned())),
            status => {
                return Err(InstallError::TransferFailed(format!(
                    "hugging face returned HTTP {status}"
                )));
            }
        }
        let info: RawInfo = serde_json::from_slice(&response.body).map_err(|error| {
            InstallError::TransferFailed(format!("parsing model info: {error}"))
        })?;
        let canonical = match info.id {
            Some(id) if id.contains('/') => id,
            _ => repo.to_owned(),
        };
        let siblings = info
            .siblings
            .unwrap_or_default()
            .into_iter()
            .map(|sibling| {
                // The listed size, else the LFS pointer's size (Swift `size ?? lfs?.size`);
                // the LFS oid is the content SHA-256 the download path keys blobs on.
                let (size, oid) = sibling
                    .lfs
                    .map(|lfs| (lfs.size, lfs.oid))
                    .unwrap_or((None, None));
                let bytes = sibling.size.or(size);
                HFSibling::new(sibling.rfilename, bytes).with_sha256(oid)
            })
            .collect();
        Ok(HFModelInfo {
            repo: canonical,
            sha: info.sha,
            gated: info.gated.map(|gated| gated.is_gated()).unwrap_or(false),
            siblings,
        })
    }

    /// The URL to download `path` from `repo` at `revision`. Each path segment is
    /// percent-encoded (Swift `appendPathComponent`), so filenames with spaces or
    /// unicode resolve correctly. Unlike the Swift original this returns a bare URL,
    /// not an authorized request — the download provider attaches the bearer token.
    pub fn resolve_url(&self, repo: &str, revision: &str, path: &str) -> String {
        let Ok(mut url) = reqwest::Url::parse(&self.base_url) else {
            return format!("{}/{repo}/resolve/{revision}/{path}", self.base_url);
        };
        if let Ok(mut segments) = url.path_segments_mut() {
            for part in repo.split('/').filter(|part| !part.is_empty()) {
                segments.push(part);
            }
            segments.push("resolve");
            segments.push(revision);
            for part in path.split('/').filter(|part| !part.is_empty()) {
                segments.push(part);
            }
        }
        url.to_string()
    }

    /// The authorized download request for `path` from `repo` at `revision` — the
    /// resolve URL plus the bearer token, when set. The download provider streams it.
    pub fn resolve_request(&self, repo: &str, revision: &str, path: &str) -> InstallRequest {
        self.authorized(InstallRequest::get(self.resolve_url(repo, revision, path)))
    }

    /// Whether a token is attached (a gated repo needs one).
    pub fn has_token(&self) -> bool {
        self.token.is_some()
    }

    fn parse_url(&self, raw: &str) -> Result<reqwest::Url, InstallError> {
        reqwest::Url::parse(raw)
            .map_err(|error| InstallError::TransferFailed(format!("bad url {raw}: {error}")))
    }

    fn authorized(&self, request: InstallRequest) -> InstallRequest {
        match &self.token {
            Some(token) => request.header("authorization", format!("Bearer {token}")),
            None => request,
        }
    }
}

#[derive(Deserialize)]
struct RawHit {
    id: String,
    downloads: Option<i64>,
    likes: Option<i64>,
    #[serde(rename = "lastModified")]
    last_modified: Option<String>,
}

#[derive(Deserialize)]
struct RawInfo {
    id: Option<String>,
    sha: Option<String>,
    gated: Option<RawGated>,
    siblings: Option<Vec<RawSibling>>,
}

#[derive(Deserialize)]
struct RawSibling {
    rfilename: String,
    size: Option<i64>,
    lfs: Option<RawLfs>,
}

#[derive(Deserialize)]
struct RawLfs {
    size: Option<i64>,
    oid: Option<String>,
}

/// The hub reports `gated` as either a bool or a mode string (`"auto"`/`"manual"`/
/// `"false"`). Any other shape decodes to `Other` and reads as not-gated, matching
/// the Swift decoder's fallback (rather than failing the whole response).
#[derive(Deserialize)]
#[serde(untagged)]
enum RawGated {
    Bool(bool),
    Mode(String),
    Other(serde::de::IgnoredAny),
}

impl RawGated {
    fn is_gated(&self) -> bool {
        match self {
            RawGated::Bool(flag) => *flag,
            RawGated::Mode(mode) => !mode.eq_ignore_ascii_case("false"),
            RawGated::Other(_) => false,
        }
    }
}

/// Parse an ISO-8601 UTC timestamp (`2024-01-15T10:30:00[.fff]Z`) to epoch
/// milliseconds, or `None` if it doesn't parse.
fn parse_iso8601_millis(value: &str) -> Option<i64> {
    let value = value.trim().trim_end_matches('Z');
    let (date, time) = value.split_once('T')?;
    let mut date_parts = date.split('-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: i64 = date_parts.next()?.parse().ok()?;
    let day: i64 = date_parts.next()?.parse().ok()?;
    let (hms, fraction) = match time.split_once('.') {
        Some((hms, fraction)) => (hms, Some(fraction)),
        None => (time, None),
    };
    let mut time_parts = hms.split(':');
    let hour: i64 = time_parts.next()?.parse().ok()?;
    let minute: i64 = time_parts.next()?.parse().ok()?;
    let second: i64 = time_parts.next()?.parse().ok()?;
    let millis: i64 = fraction
        .map(|fraction| {
            // Take the leading ASCII digits (never byte-slice — the fraction is
            // untrusted network input and could carry a multibyte char).
            let digits: String = fraction
                .chars()
                .take_while(char::is_ascii_digit)
                .take(3)
                .collect();
            format!("{digits:0<3}").parse().unwrap_or(0)
        })
        .unwrap_or(0);
    let days = days_from_civil(year, month, day);
    Some((days * 86_400 + hour * 3_600 + minute * 60 + second) * 1_000 + millis)
}

/// Days from the Unix epoch for a civil date (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = (if year >= 0 { year } else { year - 399 }) / 400;
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::{days_from_civil, parse_iso8601_millis};

    #[test]
    fn civil_date_anchors_at_the_epoch() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(1970, 1, 2), 1);
        assert_eq!(days_from_civil(2024, 1, 15), 19737);
        // Leap day and the day-after-Feb boundary.
        assert_eq!(days_from_civil(2000, 3, 1), 11017);
        assert_eq!(days_from_civil(2024, 2, 29), 19782);
    }

    #[test]
    fn iso8601_parses_fractions_and_never_panics() {
        assert_eq!(parse_iso8601_millis("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_iso8601_millis("2024-01-15T10:30:00.000Z"),
            Some(1_705_314_600_000)
        );
        assert_eq!(
            parse_iso8601_millis("2024-01-15T10:30:00Z"),
            Some(1_705_314_600_000)
        );
        // Fractions of varying length pad/truncate to milliseconds.
        assert_eq!(
            parse_iso8601_millis("2024-01-15T10:30:00.5Z"),
            Some(1_705_314_600_500)
        );
        assert_eq!(
            parse_iso8601_millis("2024-01-15T10:30:00.12Z"),
            Some(1_705_314_600_120)
        );
        assert_eq!(
            parse_iso8601_millis("2024-01-15T10:30:00.123456Z"),
            Some(1_705_314_600_123)
        );
        // A multibyte char in the fraction must NOT panic (untrusted input).
        assert_eq!(
            parse_iso8601_millis("2024-01-15T10:30:00.12éZ"),
            Some(1_705_314_600_120)
        );
        // Garbage → None, not a panic.
        assert_eq!(parse_iso8601_millis("not a date"), None);
        assert_eq!(parse_iso8601_millis("2024-01-15T10:30:00+02:00"), None);
        assert_eq!(parse_iso8601_millis(""), None);
    }
}
