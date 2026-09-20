//! Host-execution consent for manifest runtimes. A host-side manifest runs code
//! on this machine, so it stays inert until its id is approved in the settings
//! together with the hash of its files as they were when the user looked at
//! them. Any later edit changes the hash and the approval stops counting.

use kernel::manifests::RuntimeManifest;
use kernel::records::ModelRecord;

use super::detect_matches;
use crate::settings::ModelsSettings;

/// Where a manifest stands against the approvals recorded in the settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostConsent {
    /// Approved, and its files are unchanged since.
    Approved,
    /// Approved once, but its files have changed since and need another look.
    Changed,
    /// Never approved.
    Unapproved,
}

impl HostConsent {
    /// Whether the runtime may bid and run.
    pub fn is_approved(self) -> bool {
        self == Self::Approved
    }
}

/// Where `manifest` stands against the host approvals in `models`. A manifest
/// that was never hashed cannot be matched to an approval and reads as changed
/// or unapproved, never approved.
pub fn host_consent(manifest: &RuntimeManifest, models: &ModelsSettings) -> HostConsent {
    if !models.approved_host_runtimes.contains(&manifest.id) {
        return HostConsent::Unapproved;
    }
    let approved_hash = models.approved_host_runtime_hashes.get(&manifest.id);
    match (approved_hash, &manifest.content_hash) {
        (Some(approved), Some(current)) if approved == current => HostConsent::Approved,
        _ => HostConsent::Changed,
    }
}

/// The models on `shelf` that approving `manifest` would put to work, or that
/// it already serves: the ones its detect rule recognizes that no other runtime
/// holds. A manifest bids last, so a match a built-in already serves stays
/// where it is. The runtime check comes first because it is free, and the
/// detect rule reads the disk.
pub fn servable_models<'a>(
    manifest: &RuntimeManifest,
    shelf: &'a [ModelRecord],
) -> Vec<&'a ModelRecord> {
    let Some(detect) = &manifest.detect else {
        return Vec::new();
    };
    shelf
        .iter()
        .filter(|record| {
            record
                .runtime
                .id
                .as_ref()
                .is_none_or(|id| id.as_str() == manifest.id)
        })
        .filter(|record| detect_matches(detect, record))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(hash: Option<&str>) -> RuntimeManifest {
        let text = "id = \"one\"\ncapabilities = [\"chat\"]\nexecution = \"sync\"\ndetect = { extension = \"gguf\" }\n[invoke]\ncommand = \"tool --model {model}\"\n";
        let mut manifest = RuntimeManifest::parse(text, None).unwrap();
        manifest.content_hash = hash.map(str::to_owned);
        manifest
    }

    fn approved(id: &str, hash: Option<&str>) -> ModelsSettings {
        let mut models = ModelsSettings::default();
        models.approved_host_runtimes.push(id.to_owned());
        if let Some(hash) = hash {
            models
                .approved_host_runtime_hashes
                .insert(id.to_owned(), hash.to_owned());
        }
        models
    }

    #[test]
    fn an_unlisted_runtime_is_unapproved() {
        let consent = host_consent(&manifest(Some("a")), &ModelsSettings::default());
        assert_eq!(consent, HostConsent::Unapproved);
    }

    #[test]
    fn a_matching_hash_is_approved() {
        let consent = host_consent(&manifest(Some("a")), &approved("one", Some("a")));
        assert!(consent.is_approved());
    }

    #[test]
    fn an_edit_after_approval_reads_as_changed() {
        let consent = host_consent(&manifest(Some("b")), &approved("one", Some("a")));
        assert_eq!(consent, HostConsent::Changed);
    }

    #[test]
    fn an_approval_without_a_hash_never_counts() {
        assert_eq!(
            host_consent(&manifest(Some("a")), &approved("one", None)),
            HostConsent::Changed
        );
        assert_eq!(
            host_consent(&manifest(None), &approved("one", Some("a"))),
            HostConsent::Changed
        );
    }
}
