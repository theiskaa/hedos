//! Per-client access limits: which models and capabilities a caller may reach.

use kernel::records::{Capability, ModelRecord};

/// The models and capabilities a client is allowed to use. A `None` list means
/// "no restriction" on that axis; an empty list permits nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GatewayScopes {
    /// The permitted model ids, or `None` for all models.
    pub models: Option<Vec<String>>,
    /// The permitted capability names, or `None` for all capabilities.
    pub capabilities: Option<Vec<String>>,
}

impl GatewayScopes {
    /// Unrestricted access to every model and capability.
    pub fn all() -> Self {
        Self::default()
    }

    /// Whether both the model and the capability are permitted.
    pub fn permits(&self, model_id: &str, capability: &Capability) -> bool {
        self.permits_model(model_id) && self.permits_capability(capability)
    }

    /// Whether `model_id` is in scope.
    pub fn permits_model(&self, model_id: &str) -> bool {
        match &self.models {
            None => true,
            Some(models) => models.iter().any(|allowed| allowed == model_id),
        }
    }

    /// Whether `capability` is in scope.
    pub fn permits_capability(&self, capability: &Capability) -> bool {
        match &self.capabilities {
            None => true,
            Some(capabilities) => capabilities
                .iter()
                .any(|allowed| allowed == capability.as_str()),
        }
    }

    /// The subset of `shelf` whose models are in scope.
    pub fn filter(&self, shelf: &[ModelRecord]) -> Vec<ModelRecord> {
        shelf
            .iter()
            .filter(|record| self.permits_model(&record.id))
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel::records::{Modality, ModelSource, SourceKind};

    fn record(name: &str) -> ModelRecord {
        // The id derives from the source, so give each record a distinct path.
        ModelRecord::new(
            name,
            Modality::text(),
            Vec::new(),
            ModelSource::new(SourceKind::ollama(), name),
        )
    }

    #[test]
    fn the_default_scope_permits_everything() {
        let scopes = GatewayScopes::all();
        assert!(scopes.permits_model("anything"));
        assert!(scopes.permits_capability(&Capability::chat()));
        assert!(scopes.permits("m", &Capability::embed()));
    }

    #[test]
    fn a_model_list_restricts_to_its_members() {
        let scopes = GatewayScopes {
            models: Some(vec!["allowed".to_owned()]),
            capabilities: None,
        };
        assert!(scopes.permits_model("allowed"));
        assert!(!scopes.permits_model("other"));
    }

    #[test]
    fn an_empty_list_permits_nothing_on_that_axis() {
        let scopes = GatewayScopes {
            models: Some(Vec::new()),
            capabilities: None,
        };
        assert!(!scopes.permits_model("anything"));
    }

    #[test]
    fn a_capability_list_matches_on_the_raw_name() {
        let scopes = GatewayScopes {
            models: None,
            capabilities: Some(vec!["chat".to_owned()]),
        };
        assert!(scopes.permits_capability(&Capability::chat()));
        assert!(!scopes.permits_capability(&Capability::embed()));
    }

    #[test]
    fn filter_keeps_only_in_scope_models() {
        let shelf = vec![record("keep"), record("drop")];
        let keep_id = shelf[0].id.clone();
        let scopes = GatewayScopes {
            models: Some(vec![keep_id.clone()]),
            capabilities: None,
        };
        let filtered = scopes.filter(&shelf);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].id, keep_id);
    }
}
