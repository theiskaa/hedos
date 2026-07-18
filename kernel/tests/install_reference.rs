//! Tests for install-reference parsing and the install error/identity types.

use kernel::install::reference::{
    hugging_face_repo, is_hugging_face_link, is_ollama_link, normalized, normalized_tag,
    ollama_direct_tag, ollama_install_tag, ollama_tag,
};
use kernel::install::{InstallError, InstallProviderId};

#[test]
fn recognizes_hugging_face_and_ollama_links() {
    assert!(is_hugging_face_link("https://huggingface.co/org/model"));
    assert!(is_hugging_face_link("hf.co/org/model"));
    assert!(!is_hugging_face_link("org/model"));
    assert!(is_ollama_link("ollama.com/library/llama3"));
    assert!(is_ollama_link("registry.ollama.ai/llama3"));
    assert!(!is_ollama_link("llama3"));
}

#[test]
fn extracts_a_hugging_face_repo_from_bare_and_link_forms() {
    assert_eq!(
        hugging_face_repo("meta-llama/Llama-3"),
        Some("meta-llama/Llama-3".to_owned())
    );
    assert_eq!(
        hugging_face_repo("hf.co/org/model"),
        Some("org/model".to_owned())
    );
    assert_eq!(
        hugging_face_repo("https://huggingface.co/org/model"),
        Some("org/model".to_owned())
    );
    // A query/fragment and trailing slash are stripped.
    assert_eq!(
        hugging_face_repo("org/model/?tab=files"),
        Some("org/model".to_owned())
    );
    // A recognized deep link keeps only org/name.
    assert_eq!(
        hugging_face_repo("huggingface.co/org/model/tree/main"),
        Some("org/model".to_owned())
    );
}

#[test]
fn rejects_non_hugging_face_repos() {
    // Fewer than two segments.
    assert_eq!(hugging_face_repo("org"), None);
    // A reserved platform root.
    assert_eq!(hugging_face_repo("datasets/foo"), None);
    assert_eq!(hugging_face_repo("spaces/foo"), None);
    // A colon (an Ollama-style tag) is not an HF repo.
    assert_eq!(hugging_face_repo("org/model:tag"), None);
    // A deep path from a NON-hf-host input is rejected.
    assert_eq!(hugging_face_repo("org/model/extra"), None);
    // A deep path whose third segment isn't a known subpath is rejected.
    assert_eq!(hugging_face_repo("huggingface.co/org/model/nonsense"), None);
    // Empty.
    assert_eq!(hugging_face_repo("   "), None);
}

#[test]
fn extracts_ollama_tags_with_namespace_rules() {
    // A single-component reference needs no explicit version.
    assert_eq!(ollama_tag("llama3"), Some("llama3".to_owned()));
    assert_eq!(ollama_tag("llama3:8b"), Some("llama3:8b".to_owned()));
    // A namespaced reference REQUIRES an explicit version for `ollama_tag`.
    assert_eq!(ollama_tag("org/model"), None);
    assert_eq!(
        ollama_tag("org/model:tag"),
        Some("org/model:tag".to_owned())
    );
    // `ollama_install_tag` allows a namespaced reference without a version.
    assert_eq!(
        ollama_install_tag("org/model"),
        Some("org/model".to_owned())
    );
}

#[test]
fn extracts_ollama_tags_from_links() {
    // `library/` is unwrapped to the bare name.
    assert_eq!(
        ollama_tag("ollama.com/library/llama3"),
        Some("llama3".to_owned())
    );
    // A namespaced registry link keeps org/name (no explicit tag required via link).
    assert_eq!(
        ollama_tag("registry.ollama.ai/mistral/7b"),
        Some("mistral/7b".to_owned())
    );
    // A bare `library/` prefix is stripped.
    assert_eq!(ollama_tag("library/llama3"), Some("llama3".to_owned()));
}

#[test]
fn ollama_direct_tag_only_for_explicit_or_linked_inputs() {
    // Explicit version → yes.
    assert_eq!(ollama_direct_tag("llama3:8b"), Some("llama3:8b".to_owned()));
    // A bare word is NOT resolved to Ollama by default.
    assert_eq!(ollama_direct_tag("llama3"), None);
    // A link → yes.
    assert_eq!(
        ollama_direct_tag("ollama.com/library/llama3"),
        Some("llama3".to_owned())
    );
    // An `org/model` shape is an HF repo → not an Ollama direct tag.
    assert_eq!(ollama_direct_tag("org/model"), None);
}

#[test]
fn normalizes_tags_and_by_provider() {
    assert_eq!(normalized_tag("llama3"), "llama3:latest");
    assert_eq!(normalized_tag("Llama3:8B"), "llama3:8b");
    assert_eq!(
        normalized(&InstallProviderId::ollama(), "Llama3"),
        "llama3:latest"
    );
    assert_eq!(
        normalized(&InstallProviderId::huggingface(), "Org/Model"),
        "org/model"
    );
}

#[test]
fn hugging_face_repo_edge_cases() {
    // A `#fragment` is cut like a `?query`.
    assert_eq!(
        hugging_face_repo("org/model#readme"),
        Some("org/model".to_owned())
    );
    // A double slash leaves an empty name → rejected.
    assert_eq!(hugging_face_repo("org//model"), None);
    // `www.` hosts and uppercase hosts are recognized (case-insensitive strip).
    assert_eq!(
        hugging_face_repo("www.huggingface.co/org/model"),
        Some("org/model".to_owned())
    );
    assert_eq!(
        hugging_face_repo("HF.CO/org/model"),
        Some("org/model".to_owned())
    );
    // A deep path via a `www.huggingface.co` host still resolves to org/name.
    assert_eq!(
        hugging_face_repo("www.huggingface.co/org/model/tree/main"),
        Some("org/model".to_owned())
    );
    // Interior whitespace is rejected at the entry point.
    assert_eq!(hugging_face_repo("a b/c"), None);
}

#[test]
fn ollama_tag_link_and_shape_edge_cases() {
    // A `library/` link discards trailing segments (keeps only the name).
    assert_eq!(
        ollama_tag("ollama.com/library/llama3/blobs"),
        Some("llama3".to_owned())
    );
    // `www.ollama.com` host recognized.
    assert_eq!(
        ollama_tag("www.ollama.com/library/mistral"),
        Some("mistral".to_owned())
    );
    // A trailing colon (empty version) is not a valid tag.
    assert_eq!(ollama_tag("model:"), None);
    // Interior whitespace rejected.
    assert_eq!(ollama_tag("a b"), None);
    // `ollama_install_tag` on a link behaves like `ollama_tag` (links don't require
    // an explicit version).
    assert_eq!(
        ollama_install_tag("registry.ollama.ai/mistral/7b"),
        Some("mistral/7b".to_owned())
    );
    // A namespaced explicit tag is a direct tag (HF rejects on the colon first).
    assert_eq!(
        ollama_direct_tag("org/model:tag"),
        Some("org/model:tag".to_owned())
    );
}

#[test]
fn install_errors_render_the_expected_messages() {
    assert_eq!(
        InstallError::ProviderUnknown(InstallProviderId::huggingface()).to_string(),
        "No install provider is registered as huggingface."
    );
    assert_eq!(
        InstallError::ReferenceInvalid("weird".to_owned()).to_string(),
        "weird is not a reference this provider understands."
    );
    let disk = InstallError::InsufficientDisk {
        required_bytes: 8_000_000_000,
        available_bytes: 1_000_000_000,
    }
    .to_string();
    assert!(disk.starts_with("Not enough free disk space:"));
    assert!(disk.contains("is available."));
}

#[test]
fn provider_id_round_trips() {
    assert_eq!(InstallProviderId::ollama().as_str(), "ollama");
    assert_eq!(InstallProviderId::huggingface().as_str(), "huggingface");
    assert_eq!(InstallProviderId::ollama().to_string(), "ollama");
}
