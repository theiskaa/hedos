//! Resolving a client's requested model name to a ready [`ModelRecord`] on the
//! shelf, honoring the caller's scopes. This is the pure matching core; the
//! authorization and backpressure wrapper lands with the port bridge.

use kernel::records::{Capability, ModelRecord, ModelState};

use crate::admission::GatewayWorkKind;
use crate::error::{GatewayError, GatewayErrorKind};
use crate::identity::GatewayIdentity;
use crate::port::{GatewayPort, require_admission};
use crate::scopes::GatewayScopes;

/// Resolve `requested` to a single ready, in-scope model.
///
/// An exact id match wins outright. Otherwise the candidates are tried in tiers —
/// alias, exact name, case-insensitive name, then normalized tag — and the first
/// tier with a match resolves it. A tier matching more than one model is
/// ambiguous and rejected; no match at all is a not-found error.
pub fn resolve(
    requested: &str,
    shelf: &[ModelRecord],
    scopes: &GatewayScopes,
) -> Result<ModelRecord, GatewayError> {
    let ready: Vec<&ModelRecord> = shelf
        .iter()
        .filter(|record| scopes.permits_model(&record.id))
        .filter(|record| record.state == ModelState::Ready)
        .collect();

    if let Some(exact) = ready.iter().find(|record| record.id == requested) {
        return Ok((*exact).clone());
    }

    let tiers: [Vec<&ModelRecord>; 4] = [
        select(&ready, |record| record.alias.as_deref() == Some(requested)),
        select(&ready, |record| record.name == requested),
        select(&ready, |record| record.name.eq_ignore_ascii_case(requested)),
        select(&ready, |record| {
            normalized_tag(&record.name) == normalized_tag(requested)
        }),
    ];

    for candidates in tiers {
        if candidates.len() > 1 {
            let mut ids: Vec<String> = candidates.iter().map(|record| record.id.clone()).collect();
            ids.sort();
            return Err(GatewayError::new(
                GatewayErrorKind::BadRequest,
                format!(
                    "{requested} matches more than one model — use an id: {}",
                    ids.join(", ")
                ),
            ));
        }
        if let Some(first) = candidates.first() {
            return Ok((*first).clone());
        }
    }

    Err(GatewayError::new(
        GatewayErrorKind::NotFound,
        format!("no ready model matches {requested}"),
    ))
}

/// Resolve `requested` and authorize it: match it on the port's shelf within the
/// caller's scopes, require the capability scope, and check the machine can admit
/// the work now. The full gate a handler runs before dispatching.
pub async fn resolve_authorized(
    port: &dyn GatewayPort,
    requested: &str,
    capability: Capability,
    kind: GatewayWorkKind,
    identity: &GatewayIdentity,
) -> Result<ModelRecord, GatewayError> {
    let shelf = port.shelf().await;
    let record = resolve(requested, &shelf, &identity.scopes)?;
    identity.require(&record.id, &capability)?;
    require_admission(port, &record, kind).await?;
    Ok(record)
}

fn select<'a>(
    ready: &[&'a ModelRecord],
    predicate: impl Fn(&ModelRecord) -> bool,
) -> Vec<&'a ModelRecord> {
    ready
        .iter()
        .copied()
        .filter(|record| predicate(record))
        .collect()
}

/// A model name lowercased and stripped of a trailing `:latest` tag, so
/// `Llama3:latest` and `llama3` compare equal.
pub fn normalized_tag(name: &str) -> String {
    let lowered = name.to_lowercase();
    match lowered.strip_suffix(":latest") {
        Some(base) => base.to_owned(),
        None => lowered,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel::records::{Modality, ModelSource, SourceKind};

    fn ready(name: &str) -> ModelRecord {
        let mut record = ModelRecord::new(
            name,
            Modality::text(),
            Vec::new(),
            ModelSource::new(SourceKind::ollama(), name),
        );
        record.state = ModelState::Ready;
        record
    }

    #[test]
    fn an_exact_id_match_wins() {
        let record = ready("llama3");
        let shelf = vec![record.clone()];
        let resolved = resolve(&record.id, &shelf, &GatewayScopes::all()).unwrap();
        assert_eq!(resolved.id, record.id);
    }

    #[test]
    fn a_name_resolves_when_unique() {
        let shelf = vec![ready("llama3")];
        let resolved = resolve("llama3", &shelf, &GatewayScopes::all()).unwrap();
        assert_eq!(resolved.name, "llama3");
    }

    #[test]
    fn an_alias_resolves() {
        let mut record = ready("raw-name");
        record.alias = Some("friendly".to_owned());
        let shelf = vec![record];
        let resolved = resolve("friendly", &shelf, &GatewayScopes::all()).unwrap();
        assert_eq!(resolved.alias.as_deref(), Some("friendly"));
    }

    #[test]
    fn a_case_insensitive_name_resolves() {
        let shelf = vec![ready("Llama3")];
        assert!(resolve("llama3", &shelf, &GatewayScopes::all()).is_ok());
    }

    #[test]
    fn a_latest_tag_is_normalized() {
        let shelf = vec![ready("gemma:latest")];
        let resolved = resolve("gemma", &shelf, &GatewayScopes::all()).unwrap();
        assert_eq!(resolved.name, "gemma:latest");
    }

    #[test]
    fn an_unready_model_does_not_resolve() {
        // A model whose weights are missing is not a candidate.
        let mut record = ready("llama3");
        record.state = ModelState::Missing;
        let shelf = vec![record];
        assert!(resolve("llama3", &shelf, &GatewayScopes::all()).is_err());
    }

    #[test]
    fn an_ambiguous_name_lists_the_ids() {
        // Two distinct models (distinct source paths → distinct ids) sharing a
        // name are ambiguous.
        let mut a = ready("dup");
        a.source.path = "a".to_owned();
        let mut b = ready("dup");
        b.source.path = "b".to_owned();
        let shelf = vec![a, b];
        let error = resolve("dup", &shelf, &GatewayScopes::all()).unwrap_err();
        assert_eq!(error.kind, GatewayErrorKind::BadRequest);
        assert!(error.message.contains("more than one"));
    }

    #[test]
    fn an_out_of_scope_model_is_not_found() {
        let record = ready("secret");
        let shelf = vec![record.clone()];
        let scopes = GatewayScopes {
            models: Some(vec!["some-other-id".to_owned()]),
            capabilities: None,
        };
        let error = resolve(&record.id, &shelf, &scopes).unwrap_err();
        assert_eq!(error.kind, GatewayErrorKind::NotFound);
    }

    #[test]
    fn no_match_is_not_found() {
        let shelf = vec![ready("llama3")];
        let error = resolve("mistral", &shelf, &GatewayScopes::all()).unwrap_err();
        assert_eq!(error.kind, GatewayErrorKind::NotFound);
    }
}
