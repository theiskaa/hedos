//! The `KernelGateway` bridge over a minimal, empty kernel: it should delegate
//! cleanly and admit work when the machine is idle.

mod common;

use std::sync::Arc;

use common::TempDir;
use gateway::admission::{GatewayAdmissionState, GatewayWorkKind};
use gateway::kernel_gateway::KernelGateway;
use gateway::port::GatewayPort;
use kernel::artifacts::ArtifactStore;
use kernel::jobs::JobHistoryStore;
use kernel::registry::Registry;
use runtime::Kernel;
use runtime::governor::{GovernorConfig, MemoryGovernor};

fn kernel(dir: &std::path::Path) -> Arc<Kernel> {
    let registry = Registry::open(dir).unwrap();
    let artifacts = ArtifactStore::new(dir);
    let governor = Arc::new(MemoryGovernor::new(GovernorConfig::with_total_mb(262_144)));
    let history = JobHistoryStore::new(dir, 50);
    Arc::new(Kernel::new(
        registry,
        artifacts,
        governor,
        history,
        Vec::new(),
    ))
}

#[tokio::test]
async fn the_bridge_delegates_to_an_empty_kernel() {
    let dir = TempDir::new();
    let gateway = KernelGateway::new(kernel(dir.path()));

    assert!(gateway.shelf().await.is_empty());
    assert!(gateway.voices("m").await.unwrap().is_empty());
    assert!(gateway.artifact_data("missing").await.unwrap().is_none());
    assert!(!gateway.supports_tools("m").await);
    // No job is registered under an arbitrary id.
    assert!(gateway.job("no-such-job").await.is_none());
}

#[tokio::test]
async fn an_idle_kernel_admits_both_work_kinds() {
    let dir = TempDir::new();
    let gateway = KernelGateway::new(kernel(dir.path()));

    assert_eq!(
        gateway
            .admission_state("m", Some(100), GatewayWorkKind::Stream)
            .await,
        GatewayAdmissionState::Ready
    );
    assert_eq!(
        gateway
            .admission_state("m", Some(100), GatewayWorkKind::Job)
            .await,
        GatewayAdmissionState::Ready
    );
}
