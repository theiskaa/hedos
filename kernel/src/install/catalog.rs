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
    /// Music, songs and sound effects from a description.
    Audio,
}

impl InstallCategory {
    /// Every category, in the order they are shown.
    pub const ALL: [InstallCategory; 5] = [
        InstallCategory::Chat,
        InstallCategory::Code,
        InstallCategory::Voice,
        InstallCategory::Image,
        InstallCategory::Audio,
    ];

    /// The stable string form.
    pub fn as_str(&self) -> &'static str {
        match self {
            InstallCategory::Chat => "chat",
            InstallCategory::Code => "code",
            InstallCategory::Voice => "voice",
            InstallCategory::Image => "image",
            InstallCategory::Audio => "audio",
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
    /// A Hugging Face repo of GGUF weights, served by llama.cpp, shown as
    /// `name` rather than by the repo's own (`gemma-4-e4b`, not
    /// `gemma-4-e4b-it-gguf`).
    fn gguf(
        reference: &str,
        name: &str,
        blurb: &str,
        bytes: (u64, u64),
        category: InstallCategory,
    ) -> Self {
        Self {
            name: name.to_owned(),
            ..Self::hugging_face(reference, blurb, bytes, category, Engine::LlamaCpp)
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
    use InstallCategory::{Audio, Chat, Code, Image, Voice};
    vec![
        InstallCatalogEntry::gguf(
            "unsloth/Qwen3.5-0.8B-GGUF",
            "qwen3.5-0.8b",
            "Tiny and instant. Fits anywhere.",
            (738_702_022, 738_639_072),
            Chat,
        ),
        InstallCatalogEntry::gguf(
            "unsloth/Qwen3.5-4B-GGUF",
            "qwen3.5-4b",
            "Quick everyday chat on modest memory.",
            (3_417_066_906, 3_416_988_000),
            Chat,
        ),
        InstallCatalogEntry::gguf(
            "ggml-org/gemma-4-E4B-it-GGUF",
            "gemma-4-e4b",
            "Fast everyday chat, light on memory.",
            (5_151_390_940, 5_151_390_280),
            Chat,
        ),
        InstallCatalogEntry::gguf(
            "ggml-org/gemma-4-12B-it-GGUF",
            "gemma-4-12b",
            "Stronger reasoning, still nimble.",
            (7_379_100_685, 7_379_100_025),
            Chat,
        ),
        InstallCatalogEntry::gguf(
            "ggml-org/gpt-oss-20b-GGUF",
            "gpt-oss-20b",
            "OpenAI's open model. A mixture of experts, quick for its size.",
            (12_109_723_039, 12_109_722_538),
            Chat,
        ),
        InstallCatalogEntry::gguf(
            "ggml-org/gemma-4-26B-A4B-it-GGUF",
            "gemma-4-26b",
            "A mixture of experts: big-model answers at a small model's pace.",
            (15_425_099_233, 15_425_098_493),
            Chat,
        ),
        InstallCatalogEntry::gguf(
            "ggml-org/gemma-4-31B-it-GGUF",
            "gemma-4-31b",
            "Gemma's flagship, for a machine with room to spare.",
            (18_802_495_759, 18_802_495_051),
            Chat,
        ),
        InstallCatalogEntry::gguf(
            "ggml-org/Qwen3.8-27B-GGUF",
            "qwen3.8-27b",
            "Qwen's newest, strong on research and long tasks.",
            (19_603_657_041, 19_603_656_553),
            Chat,
        ),
        InstallCatalogEntry::gguf(
            "ggml-org/gpt-oss-120b-GGUF",
            "gpt-oss-120b",
            "The big one. Wants a lot of memory.",
            (63_387_561_263, 63_387_560_767),
            Chat,
        ),
        InstallCatalogEntry::gguf(
            "Qwen/Qwen2.5-Coder-7B-Instruct-GGUF",
            "qwen2.5-coder-7b",
            "Everyday coding help that fits most machines.",
            (4_683_090_549, 4_683_084_879),
            Code,
        ),
        InstallCatalogEntry::gguf(
            "Qwen/Qwen2.5-Coder-14B-Instruct-GGUF",
            "qwen2.5-coder-14b",
            "A strong local coding model with a large context.",
            (8_988_127_305, 8_988_121_615),
            Code,
        ),
        InstallCatalogEntry::gguf(
            "unsloth/Qwen3-Coder-30B-A3B-Instruct-GGUF",
            "qwen3-coder-30b",
            "Agentic coding over long context. A mixture of experts.",
            (18_556_699_641, 18_556_691_228),
            Code,
        ),
        InstallCatalogEntry::gguf(
            "ggml-org/Qwen3.6-27B-GGUF",
            "qwen3.6-27b",
            "Qwen3.6, strong at agentic coding.",
            (19_725_525_457, 19_725_524_971),
            Code,
        ),
        InstallCatalogEntry::hugging_face(
            "mlx-community/Kokoro-82M-bf16",
            "Tiny, warm text-to-speech. Instant on Apple Silicon.",
            (332_922_747, 327_117_503),
            Voice,
            Engine::Mlx,
        ),
        InstallCatalogEntry::hugging_face(
            "stabilityai/sdxl-turbo",
            "Images in one to four steps, fast enough to iterate on.",
            (26_927_366_047, 26_927_366_047),
            Image,
            Engine::Torch,
        ),
        InstallCatalogEntry::hugging_face(
            "stabilityai/stable-diffusion-xl-base-1.0",
            "Dependable, well-supported image workhorse.",
            (41_165_635_579, 41_165_635_579),
            Image,
            Engine::Torch,
        ),
        InstallCatalogEntry::hugging_face(
            "facebook/musicgen-small",
            "Instrumental clips up to 30 seconds from a description. Non-commercial licence.",
            (2_367_666_280, 2_367_653_971),
            Audio,
            Engine::Torch,
        ),
        InstallCatalogEntry::hugging_face(
            "ACE-Step/acestep-v15-xl-turbo-diffusers",
            "Whole songs with vocals from lyrics and a style, in a few steps.",
            (11_101_505_469, 11_101_505_469),
            Audio,
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
        assert_eq!(entry.id(), format!("huggingface|{}", entry.reference));
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
    fn every_chat_and_code_entry_is_a_hugging_face_gguf_llama_cpp_serves() {
        for entry in entries() {
            if matches!(
                entry.category,
                InstallCategory::Chat | InstallCategory::Code
            ) {
                assert_eq!(
                    entry.provider,
                    InstallProviderId::huggingface(),
                    "{}",
                    entry.reference
                );
                assert_eq!(entry.engine, Engine::LlamaCpp, "{}", entry.reference);
            }
            assert_ne!(entry.engine, Engine::Ollama, "{}", entry.reference);
        }
    }

    #[test]
    fn a_gguf_entry_is_shown_by_its_short_name() {
        let entry = entries()
            .into_iter()
            .find(|entry| entry.reference == "ggml-org/gemma-4-E4B-it-GGUF")
            .unwrap();
        assert_eq!(entry.name, "gemma-4-e4b");
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
