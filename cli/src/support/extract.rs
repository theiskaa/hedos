//! Asking a contact extractor and reading what it answers. The request is the
//! JSON Tessera reads (an `operation` over a `text`); the reply is the JSON it
//! writes, read here into entities, contacts and address parts for laying out,
//! on the command line and on the shelf alike.

use std::collections::BTreeMap;

use kernel::records::{Capability, JsonValue, ModelRecord};
use serde::Deserialize;

use crate::support::output::Out;
use crate::support::table;
use crate::support::text::printable;

/// What an extractor is asked to do with the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Operation {
    /// Every entity found, each on its own.
    #[default]
    Detect,
    /// The entities grouped into contacts, and what belongs to none.
    Contacts,
    /// The text read as one address and split into its parts.
    Address,
}

impl Operation {
    /// Every operation, in the order they are offered.
    pub(crate) const ALL: [Operation; 3] = [Self::Detect, Self::Contacts, Self::Address];

    /// The operation's name on the wire and on screen.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Detect => "detect",
            Self::Contacts => "contacts",
            Self::Address => "address",
        }
    }
}

/// Whether `record` extracts.
pub(crate) fn is_extractor(record: &ModelRecord) -> bool {
    record.capabilities.contains(&Capability::extract())
}

/// The request for `operation` over `text`, limited to `kinds` and hinted at
/// `countries` when they name any; without a hint the extractor uses the one its
/// model was trained to expect.
pub(crate) fn request(
    operation: Operation,
    text: &str,
    kinds: &[String],
    countries: &[String],
) -> JsonValue {
    let strings = |values: &[String]| {
        JsonValue::Array(values.iter().cloned().map(JsonValue::String).collect())
    };
    let mut request = BTreeMap::from([
        (
            "operation".to_owned(),
            JsonValue::String(operation.as_str().to_owned()),
        ),
        ("text".to_owned(), JsonValue::String(text.to_owned())),
    ]);
    if !kinds.is_empty() {
        request.insert("kinds".to_owned(), strings(kinds));
    }
    if !countries.is_empty() {
        request.insert("country_hint".to_owned(), strings(countries));
    }
    JsonValue::Object(request)
}

/// What an extractor answered, whichever operation it was asked.
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct Reply {
    /// Every entity found, for `detect`.
    #[serde(default)]
    pub entities: Vec<Entity>,
    /// The contacts, for `contacts`.
    #[serde(default)]
    pub contacts: Vec<Contact>,
    /// The entities that belong to no contact, for `contacts`.
    #[serde(default)]
    pub unassigned: Vec<Entity>,
    /// The address and its parts, for `address`.
    pub address: Option<Entity>,
}

/// One thing found: a person, an organization, an address, an email or a phone.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub(crate) struct Entity {
    pub kind: String,
    pub text: String,
    pub confidence: f64,
    /// Whether the extractor suggests a person look at it.
    #[serde(default)]
    pub review_recommended: bool,
    /// The canonical form, when there is one: E.164 for a phone, the address
    /// with its domain lowercased for an email.
    pub normalized: Option<String>,
    /// An address's parts.
    #[serde(default)]
    pub components: Vec<Component>,
}

/// One part of an address: a house number, a road, a city, a postcode.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub(crate) struct Component {
    pub label: String,
    pub text: String,
    pub confidence: f64,
}

/// The entities that belong to one person or organization.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub(crate) struct Contact {
    /// The weakest assignment's confidence.
    pub confidence: f64,
    #[serde(default)]
    pub review_recommended: bool,
    pub person: Option<Entity>,
    pub org: Option<Entity>,
    #[serde(default)]
    pub addresses: Vec<Entity>,
    #[serde(default)]
    pub emails: Vec<Entity>,
    #[serde(default)]
    pub phones: Vec<Entity>,
}

impl Contact {
    /// Who the contact is: the person, else the organization.
    pub(crate) fn head(&self) -> Option<&Entity> {
        self.person.as_ref().or(self.org.as_ref())
    }

    /// Every entity of the contact except its head, organization first.
    pub(crate) fn members(&self) -> impl Iterator<Item = &Entity> {
        let org = self.person.as_ref().and(self.org.as_ref());
        org.into_iter()
            .chain(&self.addresses)
            .chain(&self.emails)
            .chain(&self.phones)
    }
}

impl Reply {
    /// How many entities the reply holds, contacts' members included.
    pub(crate) fn entity_count(&self) -> usize {
        let in_contacts: usize = self
            .contacts
            .iter()
            .map(|contact| usize::from(contact.head().is_some()) + contact.members().count())
            .sum();
        self.entities.len()
            + in_contacts
            + self.unassigned.len()
            + usize::from(self.address.is_some())
    }
}

/// `reply` read as an extractor's answer.
pub(crate) fn parse(reply: &str) -> Result<Reply, String> {
    serde_json::from_str(reply).map_err(|error| format!("the reply was not an extraction: {error}"))
}

/// A confidence as it is shown: two decimals.
pub(crate) fn confidence(value: f64) -> String {
    format!("{value:.2}")
}

/// Lay `reply` to `operation` out for the terminal: what was found as a table
/// of kind, text and confidence; contacts as one table each under who they
/// are, then what belongs to none; an address as its parts.
pub(crate) fn render(out: &Out, reply: &Reply, operation: Operation) {
    match operation {
        Operation::Detect => entity_table(out, &reply.entities),
        Operation::Contacts => {
            if reply.contacts.is_empty() && reply.unassigned.is_empty() {
                out.line("nothing found");
            }
            for contact in &reply.contacts {
                let head = contact.head().map_or_else(
                    || "a contact".to_owned(),
                    |head| printable(&head.text).into_owned(),
                );
                out.line(&format!(
                    "{head} · {}{}",
                    confidence(contact.confidence),
                    if contact.review_recommended {
                        " · review"
                    } else {
                        ""
                    }
                ));
                let rows: Vec<Vec<String>> = contact.members().map(entity_row).collect();
                indented(out, &rows);
            }
            if !reply.unassigned.is_empty() {
                out.line("in no contact");
                let rows: Vec<Vec<String>> = reply.unassigned.iter().map(entity_row).collect();
                indented(out, &rows);
            }
        }
        Operation::Address => match &reply.address {
            Some(address) if !address.components.is_empty() => {
                let rows: Vec<Vec<String>> = address
                    .components
                    .iter()
                    .map(|part| {
                        vec![
                            part.label.replace('_', " "),
                            printable(&part.text).into_owned(),
                            confidence(part.confidence),
                        ]
                    })
                    .collect();
                out.line(&table::render(&["PART", "TEXT", "CONFIDENCE"], &rows));
            }
            _ => out.line("the address could not be split into parts"),
        },
    }
}

/// `rows` as an aligned table without a header, each line indented under the
/// line that names what they belong to.
fn indented(out: &Out, rows: &[Vec<String>]) {
    let widths = table::widths(rows, None);
    for row in rows {
        out.line(&format!("  {}", table::row(row, &widths).trim_end()));
    }
}

fn entity_table(out: &Out, entities: &[Entity]) {
    if entities.is_empty() {
        out.line("nothing found");
        return;
    }
    let rows: Vec<Vec<String>> = entities.iter().map(entity_row).collect();
    out.line(&table::render(
        &["KIND", "TEXT", "CONFIDENCE", "NORMALIZED"],
        &rows,
    ));
}

/// An entity as a row: kind, text, confidence (marked when worth a look), and
/// its canonical form.
fn entity_row(entity: &Entity) -> Vec<String> {
    vec![
        entity.kind.clone(),
        printable(&entity.text).into_owned(),
        format!(
            "{}{}",
            confidence(entity.confidence),
            if entity.review_recommended {
                " review"
            } else {
                ""
            }
        ),
        entity
            .normalized
            .as_deref()
            .map_or_else(String::new, |normalized| printable(normalized).into_owned()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONTACTS: &str = r#"{"model":"tessera","operation":"contacts","contacts":[{"start":0,"end":80,"confidence":0.71,"review_recommended":false,"person":{"kind":"person","text":"Jordan Lee","start":0,"end":10,"confidence":0.93,"review_recommended":false,"source":"model"},"addresses":[{"kind":"address","text":"123 Main St, Bismarck, ND 58501","start":12,"end":43,"confidence":0.88,"review_recommended":false,"source":"model","components":[{"label":"house_number","text":"123","start":12,"end":15,"confidence":0.99}]}],"emails":[],"phones":[{"kind":"phone","text":"(701) 555-0142","start":45,"end":59,"confidence":0.97,"review_recommended":false,"source":"rules","normalized":"+17015550142","region":"US"}]}],"unassigned":[]}"#;

    #[test]
    fn a_request_names_only_what_was_given() {
        let plain = serde_json::to_value(request(Operation::Detect, "hi", &[], &[])).unwrap();
        assert_eq!(
            plain,
            serde_json::json!({"operation": "detect", "text": "hi"})
        );
        let narrowed = serde_json::to_value(request(
            Operation::Address,
            "123 Main St",
            &["address".to_owned()],
            &["US".to_owned()],
        ))
        .unwrap();
        assert_eq!(
            narrowed,
            serde_json::json!({"operation": "address", "text": "123 Main St", "kinds": ["address"], "country_hint": ["US"]})
        );
    }

    #[test]
    fn a_contact_reads_with_its_head_and_members() {
        let reply = parse(CONTACTS).unwrap();
        let contact = &reply.contacts[0];
        assert_eq!(
            contact.head().map(|head| head.text.as_str()),
            Some("Jordan Lee")
        );
        let members: Vec<&str> = contact
            .members()
            .map(|entity| entity.kind.as_str())
            .collect();
        assert_eq!(members, ["address", "phone"]);
        assert_eq!(
            contact.phones[0].normalized.as_deref(),
            Some("+17015550142")
        );
        assert_eq!(reply.entity_count(), 3);
        assert!(parse("not json").is_err());
    }
}
