//! Parsing a user-typed model reference — a bare id, a `hf.co/...` or
//! `ollama.com/...` link — into a Hugging Face repo (`org/name`) or an Ollama tag
//! (`name:version`). Pure string work; the ambiguity is resolved the same way the
//! Swift original does so the CLI/gateway accept identical inputs.

use crate::install::provider::InstallProviderId;

const HUGGING_FACE_HOSTS: [&str; 3] = ["huggingface.co/", "www.huggingface.co/", "hf.co/"];
const OLLAMA_HOSTS: [&str; 3] = ["ollama.com/", "www.ollama.com/", "registry.ollama.ai/"];
const HUGGING_FACE_SUBPATHS: [&str; 8] = [
    "tree",
    "blob",
    "resolve",
    "commit",
    "commits",
    "discussions",
    "blame",
    "raw",
];
const HUGGING_FACE_RESERVED_ROOTS: [&str; 14] = [
    "datasets",
    "spaces",
    "collections",
    "models",
    "blog",
    "docs",
    "papers",
    "tasks",
    "posts",
    "pricing",
    "settings",
    "organizations",
    "learn",
    "chat",
];

/// Whether `raw` is a Hugging Face URL.
pub fn is_hugging_face_link(raw: &str) -> bool {
    matches_host(raw, &HUGGING_FACE_HOSTS)
}

/// Whether `raw` is an Ollama URL.
pub fn is_ollama_link(raw: &str) -> bool {
    matches_host(raw, &OLLAMA_HOSTS)
}

fn matches_host(raw: &str, hosts: &[&str]) -> bool {
    cleaned(raw).is_some_and(|text| {
        let lower = text.to_lowercase();
        hosts.iter().any(|host| lower.starts_with(host))
    })
}

/// The `org/name` repo a Hugging Face reference points at, or `None` if `raw` is
/// not a plausible HF repo. A multi-segment path is only accepted from an explicit
/// `hf.co`/`huggingface.co` link whose third segment is empty or a known HF subpath.
pub fn hugging_face_repo(raw: &str) -> Option<String> {
    let text = cleaned(raw)?;
    let text = stripped(&text, &HUGGING_FACE_HOSTS);
    if text.contains("://") || text.contains(':') {
        return None;
    }
    let components: Vec<&str> = text.split('/').collect();
    if components.len() < 2 {
        return None;
    }
    let org = components[0];
    let name = components[1];
    if org.is_empty()
        || name.is_empty()
        || HUGGING_FACE_RESERVED_ROOTS.contains(&org.to_lowercase().as_str())
    {
        return None;
    }
    if components.len() > 2 {
        let raw_lower = raw.to_lowercase();
        let from_hf_host = raw_lower.contains("hf.co") || raw_lower.contains("huggingface.co");
        let next = components[2];
        let next_ok =
            next.is_empty() || HUGGING_FACE_SUBPATHS.contains(&next.to_lowercase().as_str());
        if !(from_hf_host && next_ok) {
            return None;
        }
    }
    Some(format!("{org}/{name}"))
}

/// The Ollama tag a reference points at, requiring an explicit `:version` for a
/// namespaced (`org/name`) reference.
pub fn ollama_tag(raw: &str) -> Option<String> {
    tag(raw, true)
}

/// The Ollama tag for an install, allowing a namespaced reference without an
/// explicit version (it defaults to `:latest` downstream).
pub fn ollama_install_tag(raw: &str) -> Option<String> {
    tag(raw, false)
}

/// The Ollama tag for a search query — only when it isn't an HF repo and the input
/// carries an explicit `:` or is an Ollama link (so a bare word doesn't resolve to
/// Ollama by default).
pub fn ollama_direct_tag(query: &str) -> Option<String> {
    if hugging_face_repo(query).is_some() {
        return None;
    }
    let tag = ollama_tag(query)?;
    if query.contains(':') || is_ollama_link(query) {
        Some(tag)
    } else {
        None
    }
}

fn tag(raw: &str, require_explicit_tag_for_namespaced: bool) -> Option<String> {
    let text = cleaned(raw)?;
    let lower = text.to_lowercase();
    let is_link = OLLAMA_HOSTS.iter().any(|host| lower.starts_with(host));
    let mut text = stripped(&text, &OLLAMA_HOSTS);
    if text.contains("://") {
        return None;
    }
    if is_link {
        let components: Vec<&str> = text.split('/').collect();
        let selected: Vec<&str> = if components
            .first()
            .is_some_and(|first| first.eq_ignore_ascii_case("library"))
        {
            components.iter().skip(1).take(1).copied().collect()
        } else {
            components.iter().take(2).copied().collect()
        };
        if selected.is_empty() {
            return None;
        }
        let joined = selected.join("/");
        return shaped(&joined, false).then_some(joined);
    }
    if lower.starts_with("library/") {
        text = text["library/".len()..].to_string();
    }
    shaped(&text, require_explicit_tag_for_namespaced).then_some(text)
}

/// A tag with `:latest` added when no version is present, lowercased.
pub fn normalized_tag(reference: &str) -> String {
    let with_tag = if reference.contains(':') {
        reference.to_owned()
    } else {
        format!("{reference}:latest")
    };
    with_tag.to_lowercase()
}

/// A reference normalized for `provider`: Ollama tags gain `:latest` and lowercase;
/// other providers lowercase.
pub fn normalized(provider: &InstallProviderId, reference: &str) -> String {
    if provider.as_str() == "ollama" {
        normalized_tag(reference)
    } else {
        reference.to_lowercase()
    }
}

/// Whether `reference` is a well-formed Ollama tag (an explicit version is required
/// for a namespaced reference). Part of the parsing API the gateway/cli use to
/// validate a typed tag, alongside the other `reference` entry points.
pub fn is_ollama_tag_shaped(reference: &str) -> bool {
    shaped(reference, true)
}

fn shaped(reference: &str, require_explicit_tag_for_namespaced: bool) -> bool {
    if reference.is_empty()
        || reference.chars().any(char::is_whitespace)
        || reference.contains("://")
    {
        return false;
    }
    let components: Vec<&str> = reference.split('/').collect();
    if components.len() > 2 || components.iter().any(|component| component.is_empty()) {
        return false;
    }
    let Some(name) = components.last() else {
        return false;
    };
    let name_parts: Vec<&str> = name.split(':').collect();
    if name_parts.len() > 2 || name_parts.iter().any(|part| part.is_empty()) {
        return false;
    }
    if components.len() == 2 {
        return !require_explicit_tag_for_namespaced || name.contains(':');
    }
    true
}

/// Trim, reject interior whitespace, drop an `http(s)://` scheme and any `?`/`#`
/// query/fragment, and strip trailing slashes. `None` for an empty or whitespace-
/// bearing input.
fn cleaned(raw: &str) -> Option<String> {
    let mut text = raw.trim().to_owned();
    if text.is_empty() || text.chars().any(char::is_whitespace) {
        return None;
    }
    // No `break`: Swift re-tests the remaining text against each scheme, so a
    // stacked `https://http://…` prefix is fully stripped.
    for scheme in ["https://", "http://"] {
        if text.to_lowercase().starts_with(scheme) {
            text = text[scheme.len()..].to_owned();
        }
    }
    if let Some(stop) = text.find(['?', '#']) {
        text.truncate(stop);
    }
    while text.ends_with('/') {
        text.pop();
    }
    (!text.is_empty()).then_some(text)
}

/// Drop a leading known host prefix from `text` (case-insensitive), if present.
fn stripped(text: &str, hosts: &[&str]) -> String {
    let lower = text.to_lowercase();
    for host in hosts {
        if lower.starts_with(host) {
            return text[host.len()..].to_owned();
        }
    }
    text.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_shape_requires_a_version_when_namespaced() {
        assert!(is_ollama_tag_shaped("llama3"));
        assert!(is_ollama_tag_shaped("llama3:8b"));
        assert!(is_ollama_tag_shaped("org/model:tag"));
        // Namespaced without a version → not shaped.
        assert!(!is_ollama_tag_shaped("org/model"));
        // Whitespace / too many segments / empty name-parts → not shaped.
        assert!(!is_ollama_tag_shaped("a b"));
        assert!(!is_ollama_tag_shaped("a/b/c"));
        assert!(!is_ollama_tag_shaped("a:b:c"));
        assert!(!is_ollama_tag_shaped("model:"));
        assert!(!is_ollama_tag_shaped(""));
    }

    #[test]
    fn cleaned_strips_stacked_schemes() {
        // No `break` in the scheme loop: a stacked prefix is fully removed.
        assert_eq!(cleaned("https://http://foo").as_deref(), Some("foo"));
    }
}
