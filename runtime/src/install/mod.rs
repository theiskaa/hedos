//! Installing models: the HTTP fetch layer over the kernel's pure install types.
//! The Hugging Face hub client lands first; the fetch providers + install service
//! build on it.

pub mod hf_cache;
pub mod hf_hub;
pub mod huggingface;
pub mod ollama;
pub mod provider;
pub mod service;
pub mod transport;

pub use hf_cache::{HFCacheLayout, HFCacheWriter};
pub use hf_hub::{HFHubAPI, HFModelInfo};
pub use huggingface::HuggingFaceInstallProvider;
pub use ollama::OllamaInstallProvider;
pub use provider::{InstallEventStream, InstallFuture, InstallProvider, InstallProviderStatus};
pub use service::{CompletionFeed, InstallEventFeed, InstallService};
pub use transport::{
    InstallRequest, InstallResponse, InstallTransport, ReqwestTransport, StreamFuture, StreamStart,
    TransportFuture,
};
