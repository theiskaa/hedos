//! Installing models: the HTTP fetch layer over the kernel's pure install types.
//! The Hugging Face hub client lands first; the fetch providers + install service
//! build on it.

pub mod hf_hub;
pub mod ollama;
pub mod provider;
pub mod transport;

pub use hf_hub::{HFHubAPI, HFModelInfo};
pub use ollama::OllamaInstallProvider;
pub use provider::{InstallEventStream, InstallFuture, InstallProvider, InstallProviderStatus};
pub use transport::{
    InstallRequest, InstallResponse, InstallTransport, ReqwestTransport, StreamFuture, StreamStart,
    TransportFuture,
};
