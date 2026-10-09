//! The model a capability route names in its body: it has to resolve, and to a
//! model that declares what the route asks of it. Anything else is refused with
//! the names of the models that would do, rather than answered by whatever else
//! is on the shelf.

use kernel::records::{Capability, ModelRecord, ModelState};

use super::bad_request;
use crate::error::{GatewayError, GatewayErrorKind};
use crate::identity::GatewayIdentity;
use crate::resolver::resolve;

/// What a route asks a model to do, as its refusals say it.
pub(crate) struct Wanted {
    /// The capabilities the model has to declare.
    pub required: Vec<Capability>,
    /// The work in the plural, after "the models that" (`answer typed questions`).
    pub plural: &'static str,
    /// The same in the singular, after "no model on this machine"
    /// (`answers typed questions`).
    pub singular: &'static str,
}

/// The ready, in-scope model on `shelf` that `requested` names, provided it has
/// every capability `wanted` requires. A name that resolves to nothing is a
/// `404` and one that resolves to a model without them is a `400`; both name the
/// models that would do.
pub(crate) fn named(
    shelf: &[ModelRecord],
    identity: &GatewayIdentity,
    requested: &str,
    wanted: &Wanted,
) -> Result<ModelRecord, GatewayError> {
    let would_do = || listed(shelf, identity, wanted);
    match resolve(requested, shelf, &identity.scopes) {
        Ok(record) if has_all(&record, &wanted.required) => Ok(record),
        Ok(record) => Err(bad_request(format!(
            "{} does not {}; {}",
            record.display_name(),
            wanted.plural,
            would_do()
        ))),
        Err(error) if error.kind == GatewayErrorKind::NotFound => Err(GatewayError::new(
            GatewayErrorKind::NotFound,
            format!("{}; {}", error.message, would_do()),
        )),
        Err(error) => Err(error),
    }
}

/// The ready, in-scope models on `shelf` that have every capability `wanted`
/// requires, named for a refusal.
pub(crate) fn listed(shelf: &[ModelRecord], identity: &GatewayIdentity, wanted: &Wanted) -> String {
    let mut names: Vec<&str> = shelf
        .iter()
        .filter(|record| record.state == ModelState::Ready)
        .filter(|record| identity.scopes.permits_model(&record.id))
        .filter(|record| has_all(record, &wanted.required))
        .map(ModelRecord::display_name)
        .collect();
    names.sort_unstable();
    if names.is_empty() {
        format!("no model on this machine {}", wanted.singular)
    } else {
        format!(
            "the models that {} here: {}",
            wanted.plural,
            names.join(", ")
        )
    }
}

fn has_all(record: &ModelRecord, required: &[Capability]) -> bool {
    required
        .iter()
        .all(|capability| record.capabilities.contains(capability))
}
