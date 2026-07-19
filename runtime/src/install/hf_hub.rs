//! The Hugging Face hub API: search for models, fetch a repo's metadata + file
//! listing, and build file-download URLs. Runs over the injectable
//! [`InstallTransport`].

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
#[derive(Clone)]
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
                // The last non-empty path segment (empty segments are dropped, so a
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
                    updated_at: hit
                        .last_modified
                        .as_deref()
                        .and_then(kernel::time::millis_from_iso8601),
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
                // The listed size, else the LFS pointer's size; the LFS oid is the
                // content SHA-256 the download path keys blobs on.
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
    /// percent-encoded, so filenames with spaces or unicode resolve correctly. This
    /// returns a bare URL, not an authorized request — the download provider
    /// attaches the bearer token.
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
/// `"false"`). Any other shape decodes to `Other` and reads as not-gated (rather
/// than failing the whole response).
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
