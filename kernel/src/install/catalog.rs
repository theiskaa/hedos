//! A curated catalog of models worth installing, grouped by task and filtered by
//! how well they fit the machine's memory.

use std::collections::HashSet;

use crate::install::provider::InstallProviderId;
use crate::profiles::{FitAssessment, FitVerdict};

/// The task a catalog entry is meant for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InstallCategory {
    /// General chat / assistants.
    Chat,
    /// Coding help.
    Code,
    /// Speech (text-to-speech / transcription).
    Voice,
    /// Image generation.
    Image,
}

impl InstallCategory {
    /// The stable string form.
    pub fn as_str(&self) -> &'static str {
        match self {
            InstallCategory::Chat => "chat",
            InstallCategory::Code => "code",
            InstallCategory::Voice => "voice",
            InstallCategory::Image => "image",
        }
    }
}

/// One recommendable model: where it comes from, how big it is, and what it's for.
#[derive(Debug, Clone, PartialEq)]
pub struct InstallCatalogEntry {
    /// The provider that installs it.
    pub provider: InstallProviderId,
    /// The reference to install (tag or repo).
    pub reference: String,
    /// The name to show.
    pub name: String,
    /// A one-line description.
    pub blurb: String,
    /// The approximate download/footprint size in gigabytes.
    pub size_gb: f64,
    /// The task it's meant for.
    pub category: InstallCategory,
}

impl InstallCatalogEntry {
    fn new(
        provider: InstallProviderId,
        reference: &str,
        name: &str,
        blurb: &str,
        size_gb: f64,
        category: InstallCategory,
    ) -> Self {
        Self {
            provider,
            reference: reference.to_owned(),
            name: name.to_owned(),
            blurb: blurb.to_owned(),
            size_gb,
            category,
        }
    }

    /// A stable id: `provider|reference`.
    pub fn id(&self) -> String {
        format!("{}|{}", self.provider.as_str(), self.reference)
    }

    /// How well this model fits in `total_memory_bytes` (footprint = `size_gb`
    /// gigabytes, passed to the assessor as MiB), or `None` if the size is unusable.
    pub fn fit(&self, total_memory_bytes: u64) -> Option<FitAssessment> {
        FitVerdict::assess(Some((self.size_gb * 1024.0) as i64), total_memory_bytes)
    }
}

/// The full curated catalog.
pub fn entries() -> Vec<InstallCatalogEntry> {
    let ollama = InstallProviderId::ollama();
    let hf = InstallProviderId::huggingface();
    vec![
        InstallCatalogEntry::new(
            ollama.clone(),
            "gemma3:1b",
            "gemma3:1b",
            "Tiny and instant. Always fits.",
            0.8,
            InstallCategory::Chat,
        ),
        InstallCatalogEntry::new(
            ollama.clone(),
            "llama3.2:3b",
            "llama3.2:3b",
            "Fast general chat on modest memory.",
            2.0,
            InstallCategory::Chat,
        ),
        InstallCatalogEntry::new(
            ollama.clone(),
            "gemma3:4b",
            "gemma3:4b",
            "Fast everyday chat. Runs comfortably on any Mac.",
            3.3,
            InstallCategory::Chat,
        ),
        InstallCatalogEntry::new(
            ollama.clone(),
            "gemma3:12b",
            "gemma3:12b",
            "Stronger reasoning, still nimble.",
            8.1,
            InstallCategory::Chat,
        ),
        InstallCatalogEntry::new(
            ollama.clone(),
            "gemma3:27b",
            "gemma3:27b",
            "Flagship reasoning with room to spare on a big Mac.",
            17.0,
            InstallCategory::Chat,
        ),
        InstallCatalogEntry::new(
            ollama.clone(),
            "llama3.3:70b",
            "llama3.3:70b",
            "The big one, at Q4. Leaves a little headroom, not much.",
            40.0,
            InstallCategory::Chat,
        ),
        InstallCatalogEntry::new(
            ollama.clone(),
            "qwen2.5-coder:7b",
            "qwen2.5-coder:7b",
            "Everyday coding help that fits most Macs.",
            4.7,
            InstallCategory::Code,
        ),
        InstallCatalogEntry::new(
            ollama.clone(),
            "qwen2.5-coder:14b",
            "qwen2.5-coder:14b",
            "Strong local coding model with a large context.",
            9.0,
            InstallCategory::Code,
        ),
        InstallCatalogEntry::new(
            ollama,
            "deepseek-coder-v2:16b",
            "deepseek-coder-v2:16b",
            "Sharp on repository-scale edits and refactors.",
            9.4,
            InstallCategory::Code,
        ),
        InstallCatalogEntry::new(
            hf.clone(),
            "hexgrad/Kokoro-82M",
            "kokoro-82m",
            "Tiny, warm text-to-speech. Instant on any Mac.",
            0.3,
            InstallCategory::Voice,
        ),
        InstallCatalogEntry::new(
            hf.clone(),
            "openai/whisper-large-v3",
            "whisper-large-v3",
            "Best-in-class speech-to-text for dictation.",
            1.5,
            InstallCategory::Voice,
        ),
        InstallCatalogEntry::new(
            hf.clone(),
            "black-forest-labs/FLUX.1-schnell",
            "flux.1-schnell",
            "Quick, striking image generation in a few steps.",
            24.0,
            InstallCategory::Image,
        ),
        InstallCatalogEntry::new(
            hf,
            "stabilityai/stable-diffusion-xl-base-1.0",
            "sdxl",
            "Dependable, well-supported image workhorse.",
            7.0,
            InstallCategory::Image,
        ),
    ]
}

/// Up to three recommended entries for `total_memory_bytes`, optionally scoped to
/// a `category` and/or a set of `providers`. Entries that run well are preferred
/// (the largest three that do); if none fit, the single smallest scoped entry is
/// returned so there's always a suggestion.
pub fn recommended(
    category: Option<InstallCategory>,
    total_memory_bytes: u64,
    providers: Option<&HashSet<InstallProviderId>>,
) -> Vec<InstallCatalogEntry> {
    let scoped: Vec<InstallCatalogEntry> = entries()
        .into_iter()
        .filter(|entry| category.is_none_or(|category| entry.category == category))
        .filter(|entry| providers.is_none_or(|providers| providers.contains(&entry.provider)))
        .collect();

    let mut fitting: Vec<InstallCatalogEntry> = scoped
        .iter()
        .filter(|entry| {
            entry
                .fit(total_memory_bytes)
                .is_some_and(|assessment| assessment.verdict == FitVerdict::RunsWell)
        })
        .cloned()
        .collect();
    fitting.sort_by(by_size_then_reference);

    if fitting.is_empty() {
        return scoped
            .into_iter()
            .min_by(by_size_then_reference)
            .map(|entry| vec![entry])
            .unwrap_or_default();
    }
    // The largest three that still run well.
    let start = fitting.len().saturating_sub(3);
    fitting.split_off(start)
}

/// Recommend for a machine with `ram_gb` gigabytes (at least 1).
pub fn recommended_for_ram(
    category: Option<InstallCategory>,
    ram_gb: u64,
    providers: Option<&HashSet<InstallProviderId>>,
) -> Vec<InstallCatalogEntry> {
    recommended(category, ram_gb.max(1) << 30, providers)
}

fn by_size_then_reference(a: &InstallCatalogEntry, b: &InstallCatalogEntry) -> std::cmp::Ordering {
    a.size_gb
        .total_cmp(&b.size_gb)
        .then_with(|| a.reference.cmp(&b.reference))
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1 << 30;

    #[test]
    fn ids_are_provider_and_reference() {
        let entry = &entries()[0];
        assert_eq!(entry.id(), format!("ollama|{}", entry.reference));
    }

    #[test]
    fn a_category_filter_scopes_the_recommendations() {
        let code = recommended(Some(InstallCategory::Code), 64 * GIB, None);
        assert!(!code.is_empty());
        assert!(
            code.iter()
                .all(|entry| entry.category == InstallCategory::Code)
        );
    }

    #[test]
    fn a_provider_filter_scopes_the_recommendations() {
        let only_ollama: HashSet<InstallProviderId> = [InstallProviderId::ollama()].into();
        let hits = recommended(None, 64 * GIB, Some(&only_ollama));
        assert!(
            hits.iter()
                .all(|entry| entry.provider == InstallProviderId::ollama())
        );
    }

    #[test]
    fn a_big_machine_gets_the_largest_three_that_run_well() {
        let chat = recommended(Some(InstallCategory::Chat), 128 * GIB, None);
        assert_eq!(chat.len(), 3);
        // Sorted ascending by size, so the last is the largest.
        assert!(chat[0].size_gb <= chat[2].size_gb);
    }

    #[test]
    fn a_tiny_machine_still_gets_the_smallest_suggestion() {
        // 2 GiB can't run anything well, but the smallest chat model is returned.
        let chat = recommended(Some(InstallCategory::Chat), 2 * GIB, None);
        assert_eq!(chat.len(), 1);
        assert_eq!(chat[0].reference, "gemma3:1b");
    }

    #[test]
    fn recommended_for_ram_clamps_to_at_least_one_gib() {
        // ram 0 → clamped to 1 GiB; still returns the fallback smallest.
        let chat = recommended_for_ram(Some(InstallCategory::Chat), 0, None);
        assert_eq!(chat.len(), 1);
    }
}
