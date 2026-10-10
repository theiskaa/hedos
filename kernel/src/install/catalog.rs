//! A curated catalog of models worth installing, grouped by task. Each entry
//! names the engine that serves it, what a pull downloads, and what serving it
//! loads, so [`recommend`](super::recommend) can judge it against the machine.

use crate::install::provider::InstallProviderId;
use crate::machine::{Engine, Fit, Machine};

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
    /// Every category, in the order they are shown.
    pub const ALL: [InstallCategory; 4] = [
        InstallCategory::Chat,
        InstallCategory::Code,
        InstallCategory::Voice,
        InstallCategory::Image,
    ];

    /// The stable string form.
    pub fn as_str(&self) -> &'static str {
        match self {
            InstallCategory::Chat => "chat",
            InstallCategory::Code => "code",
            InstallCategory::Voice => "voice",
            InstallCategory::Image => "image",
        }
    }

    /// The category named `name` (its stable string form), if any.
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|category| category.as_str().eq_ignore_ascii_case(name))
    }
}

/// One recommendable model: where it comes from, how big it is, what it's for,
/// and what serves it.
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
    /// What a pull downloads, in bytes: the sum of an Ollama manifest's
    /// layers, or the files a Hugging Face pull selects.
    pub download_bytes: u64,
    /// What serving it loads, in bytes, as the shelf measures it once pulled.
    /// A repo can hold several copies of its weights (a single-file checkpoint
    /// beside the diffusers folders, fp16 beside full precision), and serving
    /// loads one, so this can be far below `download_bytes`.
    pub serving_bytes: u64,
    /// The task it's meant for.
    pub category: InstallCategory,
    /// The engine that serves it once installed.
    pub engine: Engine,
}

impl InstallCatalogEntry {
    /// An Ollama tag: what it downloads is what it serves.
    fn ollama(reference: &str, blurb: &str, bytes: u64, category: InstallCategory) -> Self {
        Self {
            provider: InstallProviderId::ollama(),
            reference: reference.to_owned(),
            name: reference.to_owned(),
            blurb: blurb.to_owned(),
            download_bytes: bytes,
            serving_bytes: bytes,
            category,
            engine: Engine::Ollama,
        }
    }

    /// A Hugging Face repo, `(download, serving)` bytes apart.
    fn hugging_face(
        reference: &str,
        blurb: &str,
        (download_bytes, serving_bytes): (u64, u64),
        category: InstallCategory,
        engine: Engine,
    ) -> Self {
        let name = reference.rsplit('/').next().unwrap_or(reference);
        Self {
            provider: InstallProviderId::huggingface(),
            reference: reference.to_owned(),
            name: name.to_lowercase(),
            blurb: blurb.to_owned(),
            download_bytes,
            serving_bytes,
            category,
            engine,
        }
    }

    /// A stable id: `provider|reference`.
    pub fn id(&self) -> String {
        format!("{}|{}", self.provider.as_str(), self.reference)
    }

    /// What a pull downloads, as the byte count a record carries.
    pub fn download_size(&self) -> i64 {
        i64::try_from(self.download_bytes).unwrap_or(i64::MAX)
    }

    /// What serving it loads, as the byte count a record carries.
    pub fn serving_size(&self) -> i64 {
        i64::try_from(self.serving_bytes).unwrap_or(i64::MAX)
    }

    /// How this model fits on `machine` under its engine, judged by what
    /// serving it loads, or `None` when the engine cannot run there.
    pub fn fit(&self, machine: &Machine) -> Option<Fit> {
        machine.fit(self.engine, Some(self.serving_size()))
    }
}

/// The full curated catalog, smallest first within each category.
///
/// Every tag and repo here was checked against its registry, and each download
/// size is what that registry lists; `catalog_sizes_match_the_registries` in
/// the runtime's tests checks them again (it is ignored by default, since it
/// reads the network). Each serving size is what the shelf measures for the
/// pulled repo, pinned by `catalog_servable` over the repo's captured listing.
pub fn entries() -> Vec<InstallCatalogEntry> {
    use InstallCategory::{Chat, Code, Image, Voice};
    vec![
        InstallCatalogEntry::ollama(
            "qwen3.5:0.8b",
            "Tiny and instant. Fits anywhere.",
            1_322_069_043,
            Chat,
        ),
        InstallCatalogEntry::ollama(
            "qwen3.5:4b",
            "Quick everyday chat on modest memory.",
            3_324_173_757,
            Chat,
        ),
        InstallCatalogEntry::ollama(
            "gemma4:e4b",
            "Fast everyday chat, light on memory.",
            6_583_656_264,
            Chat,
        ),
        InstallCatalogEntry::ollama(
            "gemma4:12b",
            "Stronger reasoning, still nimble.",
            8_021_618_699,
            Chat,
        ),
        InstallCatalogEntry::ollama(
            "gpt-oss:20b",
            "OpenAI's open model. A mixture of experts, quick for its size.",
            13_793_441_427,
            Chat,
        ),
        InstallCatalogEntry::ollama(
            "qwen3.8:27b",
            "Qwen's newest, strong on research and long tasks.",
            17_741_871_939,
            Chat,
        ),
        InstallCatalogEntry::ollama(
            "gemma4:26b",
            "A mixture of experts: big-model answers at a small model's pace.",
            18_731_025_387,
            Chat,
        ),
        InstallCatalogEntry::ollama(
            "gemma4:31b",
            "Gemma's flagship, for a machine with room to spare.",
            20_385_457_992,
            Chat,
        ),
        InstallCatalogEntry::ollama(
            "gpt-oss:120b",
            "The big one. Wants a lot of memory.",
            65_369_819_443,
            Chat,
        ),
        InstallCatalogEntry::ollama(
            "qwen2.5-coder:7b",
            "Everyday coding help that fits most machines.",
            4_683_087_074,
            Code,
        ),
        InstallCatalogEntry::ollama(
            "qwen2.5-coder:14b",
            "A strong local coding model with a large context.",
            8_988_123_810,
            Code,
        ),
        InstallCatalogEntry::ollama(
            "qwen3.6:27b-coding",
            "Qwen3.6 tuned for agentic coding.",
            17_769_076_721,
            Code,
        ),
        InstallCatalogEntry::ollama(
            "qwen3-coder:30b",
            "Agentic coding over long context. A mixture of experts.",
            18_556_700_222,
            Code,
        ),
        InstallCatalogEntry::hugging_face(
            "mlx-community/Kokoro-82M-bf16",
            "Tiny, warm text-to-speech. Instant on Apple Silicon.",
            (332_922_211, 327_117_503),
            Voice,
            Engine::Mlx,
        ),
        InstallCatalogEntry::hugging_face(
            "stabilityai/sdxl-turbo",
            "Images in one to four steps, fast enough to iterate on.",
            (26_927_360_164, 26_927_360_164),
            Image,
            Engine::Torch,
        ),
        InstallCatalogEntry::hugging_face(
            "stabilityai/stable-diffusion-xl-base-1.0",
            "Dependable, well-supported image workhorse.",
            (41_165_626_911, 41_165_626_911),
            Image,
            Engine::Torch,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_provider_and_reference() {
        let entry = &entries()[0];
        assert_eq!(entry.id(), format!("ollama|{}", entry.reference));
    }

    #[test]
    fn every_entry_has_a_size_and_a_serving_engine() {
        for entry in entries() {
            assert!(entry.download_bytes > 0, "{}", entry.reference);
            assert!(entry.serving_bytes > 0, "{}", entry.reference);
            assert!(
                entry.serving_bytes <= entry.download_bytes,
                "{}",
                entry.reference
            );
            assert_ne!(entry.engine, Engine::Other, "{}", entry.reference);
        }
    }

    #[test]
    fn references_are_unique() {
        let entries = entries();
        let mut ids: Vec<String> = entries.iter().map(InstallCatalogEntry::id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), entries.len());
    }

    #[test]
    fn ollama_tags_are_served_by_ollama() {
        for entry in entries() {
            assert_eq!(
                entry.provider == InstallProviderId::ollama(),
                entry.engine == Engine::Ollama,
                "{}",
                entry.reference
            );
        }
    }

    #[test]
    fn a_name_is_the_lowercased_last_segment() {
        let entry = entries()
            .into_iter()
            .find(|entry| entry.reference == "mlx-community/Kokoro-82M-bf16")
            .unwrap();
        assert_eq!(entry.name, "kokoro-82m-bf16");
    }

    #[test]
    fn categories_parse_from_their_names() {
        for category in InstallCategory::ALL {
            assert_eq!(InstallCategory::parse(category.as_str()), Some(category));
        }
        assert_eq!(InstallCategory::parse("CODE"), Some(InstallCategory::Code));
        assert_eq!(InstallCategory::parse("speech"), None);
    }
}
