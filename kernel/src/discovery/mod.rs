//! Finding models already on disk. This module holds the format-agnostic pieces
//! — multi-part GGUF shard grouping and duplicate-weight detection. The
//! directory scanners and reconciliation into the registry build on these.

pub mod duplicates;
pub mod gguf_shards;

pub use duplicates::{DEFAULT_THRESHOLD, DuplicateGroup, detect};
pub use gguf_shards::{Member, ShardGroup, ShardName, group, parse, shard_filename};
