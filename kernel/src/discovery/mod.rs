//! Finding models already on disk. This module holds the format-agnostic pieces
//! — multi-part GGUF shard grouping and duplicate-weight detection. The
//! directory scanners and reconciliation into the registry build on these.

pub mod duplicates;
pub mod gguf_models;
pub mod gguf_shards;
pub mod habitat;
pub mod hf_scanner;
pub mod lm_studio_scanner;
pub mod loose_file_scanner;
pub mod modality_hints;
pub mod ollama_scanner;
pub mod scanner;
pub mod service;

pub use duplicates::{DEFAULT_THRESHOLD, DuplicateGroup, content_fingerprint, detect};
pub use gguf_models::{discovered_models, is_mmproj_name};
pub use gguf_shards::{Member, ShardGroup, ShardName, group, parse, shard_filename};
pub use habitat::{ModelHabitat, ModelsSettings};
pub use hf_scanner::HFCacheScanner;
pub use lm_studio_scanner::LMStudioScanner;
pub use loose_file_scanner::LooseFileScanner;
pub use modality_hints::Hint;
pub use ollama_scanner::OllamaStoreScanner;
pub use scanner::{DiscoveredModel, ScanResult, StoreScanner};
pub use service::{DiscoveryService, DiscoverySummary, KindStat};
