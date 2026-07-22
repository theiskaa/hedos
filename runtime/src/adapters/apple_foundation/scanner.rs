//! Discovery of Apple's on-device model: a [`StoreScanner`] that puts one
//! "Apple Intelligence" record on the shelf when the model is available, and
//! surfaces why it isn't otherwise. It lives in the runtime crate — unlike the
//! filesystem scanners — because availability comes from the backend bridge,
//! not from files on disk.

use std::sync::Arc;

use kernel::discovery::{DiscoveredModel, ScanResult, StoreScanner};
use kernel::records::{Capability, ExecutionMode, Modality, ModelSource, SourceKind};

use super::backend::{AppleFoundationBackend, BuiltinAvailability};

/// The path a builtin record points at: the framework stands in for weights.
const FRAMEWORK_PATH: &str = "/System/Library/Frameworks/FoundationModels.framework";

/// The scanner over Apple's built-in model.
pub struct AppleFoundationScanner {
    backend: Arc<dyn AppleFoundationBackend>,
}

impl AppleFoundationScanner {
    /// A scanner probing availability through `backend`.
    pub fn new(backend: Arc<dyn AppleFoundationBackend>) -> Self {
        Self { backend }
    }
}

impl StoreScanner for AppleFoundationScanner {
    fn kinds(&self) -> Vec<SourceKind> {
        vec![SourceKind::builtin()]
    }

    fn scan(&self) -> ScanResult {
        match self.backend.availability() {
            BuiltinAvailability::Available => {
                let mut model = DiscoveredModel::new(
                    "Apple Intelligence",
                    ModelSource::new(SourceKind::builtin(), FRAMEWORK_PATH),
                );
                model.modality_hint = Some(Modality::text());
                model.capabilities_hint = vec![Capability::chat(), Capability::complete()];
                model.execution_hint = ExecutionMode::Stream;
                ScanResult {
                    discovered: vec![model],
                    ..ScanResult::default()
                }
            }
            BuiltinAvailability::NotEnabled => ScanResult {
                issues: vec![
                    "Apple Intelligence is turned off — turn it on in System Settings to put Apple's model on the shelf."
                        .to_owned(),
                ],
                ..ScanResult::default()
            },
            BuiltinAvailability::NotReady => ScanResult {
                issues: vec![
                    "Apple's model is still downloading — it will appear on the shelf when it's ready."
                        .to_owned(),
                ],
                ..ScanResult::default()
            },
            // An ineligible machine stays silent: no record, no nagging issue.
            BuiltinAvailability::NotEligible => ScanResult::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::backend::{BuiltinEventStream, BuiltinOptions};
    use super::*;
    use crate::adapters::{RuntimeError, RuntimeStream};
    use kernel::capabilities::ChatMessage;

    struct Fixed(BuiltinAvailability);

    impl AppleFoundationBackend for Fixed {
        fn availability(&self) -> BuiltinAvailability {
            self.0
        }

        fn stream(
            &self,
            _messages: Vec<ChatMessage>,
            _options: BuiltinOptions,
        ) -> BuiltinEventStream {
            RuntimeStream::failed(RuntimeError::Unavailable("not under test".to_owned()))
        }
    }

    fn scan(availability: BuiltinAvailability) -> ScanResult {
        AppleFoundationScanner::new(Arc::new(Fixed(availability))).scan()
    }

    #[test]
    fn it_scans_the_builtin_kind() {
        let scanner = AppleFoundationScanner::new(Arc::new(Fixed(BuiltinAvailability::Available)));
        assert_eq!(scanner.kinds(), vec![SourceKind::builtin()]);
    }

    #[test]
    fn an_available_model_lands_on_the_shelf() {
        let result = scan(BuiltinAvailability::Available);
        assert!(result.issues.is_empty());
        assert_eq!(result.discovered.len(), 1);
        let model = &result.discovered[0];
        assert_eq!(model.name, "Apple Intelligence");
        assert_eq!(model.source.kind, SourceKind::builtin());
        assert_eq!(model.source.path, FRAMEWORK_PATH);
        assert_eq!(model.modality_hint, Some(Modality::text()));
        assert_eq!(
            model.capabilities_hint,
            vec![Capability::chat(), Capability::complete()]
        );
        assert_eq!(model.execution_hint, ExecutionMode::Stream);
        assert_eq!(model.footprint_bytes, 0);
    }

    #[test]
    fn not_enabled_and_not_ready_surface_issues_without_records() {
        for availability in [
            BuiltinAvailability::NotEnabled,
            BuiltinAvailability::NotReady,
        ] {
            let result = scan(availability);
            assert!(result.discovered.is_empty());
            assert_eq!(result.issues.len(), 1, "for {availability:?}");
            assert!(result.failed_kinds.is_empty());
        }
        assert!(scan(BuiltinAvailability::NotEnabled).issues[0].contains("System Settings"));
        assert!(scan(BuiltinAvailability::NotReady).issues[0].contains("downloading"));
    }

    #[test]
    fn an_ineligible_machine_stays_silent() {
        assert_eq!(
            scan(BuiltinAvailability::NotEligible),
            ScanResult::default()
        );
    }
}
