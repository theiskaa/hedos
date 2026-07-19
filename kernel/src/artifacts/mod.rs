//! Generated outputs: the content-addressed artifact store, its records, gallery
//! arrangement, and provenance rendering. Pure logic and the filesystem — the
//! `ProvenanceArtifactWriter` that bridges the store to the registry is DI glue
//! that lands with the kernel facade.

mod artifact;
mod gallery;
mod provenance;
mod speech_text;
mod store;

pub use artifact::{Artifact, ArtifactDraft};
pub use gallery::{Gallery, GalleryModel, GallerySort};
pub use provenance::Provenance;
pub use speech_text::speakable;
pub use store::{ArtifactStore, ArtifactStoreError};
