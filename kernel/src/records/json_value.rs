//! A dynamic JSON value used for model parameters, tool arguments, and payloads.
//!
//! Unlike `serde_json::Value` this keeps integers and floats distinct yet treats
//! them as equal when they represent the same number (`Int(2) == Double(2.0)`),
//! matching the semantics the rest of the kernel relies on when comparing
//! parameter values. Objects use a `BTreeMap` so serialization is key-sorted and
//! deterministic.

use std::collections::BTreeMap;
use std::fmt;

use serde::de::{Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};

const MIN_I64_AS_F64: f64 = i64::MIN as f64;
const MAX_I64_AS_F64: f64 = i64::MAX as f64;

/// A JSON value with integers and floats kept distinct.
#[derive(Debug, Clone, Default)]
pub enum JsonValue {
    /// JSON `null`.
    #[default]
    Null,
    /// A boolean.
    Bool(bool),
    /// An integer.
    Int(i64),
    /// A floating-point number. Non-finite values (NaN, infinities) are not valid
    /// JSON and serialize to `null`; avoid constructing them.
    Double(f64),
    /// A string.
    String(String),
    /// An array of values.
    Array(Vec<JsonValue>),
    /// An object with string keys, kept sorted.
    Object(BTreeMap<String, JsonValue>),
}

impl JsonValue {
    /// Build an object from a fixed set of `(key, value)` pairs.
    pub fn object<const N: usize>(pairs: [(&str, JsonValue); N]) -> Self {
        JsonValue::Object(
            pairs
                .into_iter()
                .map(|(key, value)| (key.to_owned(), value))
                .collect(),
        )
    }

    /// The object's fields, if this is an object.
    pub fn as_object(&self) -> Option<&BTreeMap<String, JsonValue>> {
        match self {
            JsonValue::Object(fields) => Some(fields),
            _ => None,
        }
    }

    /// The array's elements, if this is an array.
    pub fn as_array(&self) -> Option<&[JsonValue]> {
        match self {
            JsonValue::Array(items) => Some(items),
            _ => None,
        }
    }

    /// The string, if this is a string.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            JsonValue::String(value) => Some(value),
            _ => None,
        }
    }

    /// The boolean, if this is a boolean.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            JsonValue::Bool(value) => Some(*value),
            _ => None,
        }
    }

    /// The value as an integer, accepting a finite in-range float (truncated
    /// toward zero) as well as an integer. A non-finite or out-of-range float
    /// yields `None` rather than a saturated, misleading value.
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            JsonValue::Int(value) => Some(*value),
            JsonValue::Double(value)
                if value.is_finite() && *value >= MIN_I64_AS_F64 && *value < MAX_I64_AS_F64 =>
            {
                Some(*value as i64)
            }
            _ => None,
        }
    }

    /// The value as a float, accepting an integer as well as a float.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            JsonValue::Double(value) => Some(*value),
            JsonValue::Int(value) => Some(*value as f64),
            _ => None,
        }
    }
}

impl From<bool> for JsonValue {
    fn from(value: bool) -> Self {
        JsonValue::Bool(value)
    }
}

impl From<i64> for JsonValue {
    fn from(value: i64) -> Self {
        JsonValue::Int(value)
    }
}

impl From<f64> for JsonValue {
    fn from(value: f64) -> Self {
        JsonValue::Double(value)
    }
}

impl From<&str> for JsonValue {
    fn from(value: &str) -> Self {
        JsonValue::String(value.to_owned())
    }
}

impl From<String> for JsonValue {
    fn from(value: String) -> Self {
        JsonValue::String(value)
    }
}

impl From<Vec<JsonValue>> for JsonValue {
    fn from(value: Vec<JsonValue>) -> Self {
        JsonValue::Array(value)
    }
}

impl From<BTreeMap<String, JsonValue>> for JsonValue {
    fn from(value: BTreeMap<String, JsonValue>) -> Self {
        JsonValue::Object(value)
    }
}

impl PartialEq for JsonValue {
    fn eq(&self, other: &Self) -> bool {
        use JsonValue::{Array, Bool, Double, Int, Null, Object, String};
        match (self, other) {
            (Null, Null) => true,
            (Bool(a), Bool(b)) => a == b,
            (String(a), String(b)) => a == b,
            (Array(a), Array(b)) => a == b,
            (Object(a), Object(b)) => a == b,
            (Int(a), Int(b)) => a == b,
            (Double(a), Double(b)) => a == b,
            (Int(a), Double(b)) => (*a as f64) == *b,
            (Double(a), Int(b)) => *a == (*b as f64),
            _ => false,
        }
    }
}

impl Serialize for JsonValue {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            JsonValue::Null => serializer.serialize_unit(),
            JsonValue::Bool(value) => serializer.serialize_bool(*value),
            JsonValue::Int(value) => serializer.serialize_i64(*value),
            JsonValue::Double(value) => serializer.serialize_f64(*value),
            JsonValue::String(value) => serializer.serialize_str(value),
            JsonValue::Array(items) => {
                let mut seq = serializer.serialize_seq(Some(items.len()))?;
                for item in items {
                    seq.serialize_element(item)?;
                }
                seq.end()
            }
            JsonValue::Object(fields) => {
                let mut map = serializer.serialize_map(Some(fields.len()))?;
                for (key, value) in fields {
                    map.serialize_entry(key, value)?;
                }
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for JsonValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(JsonValueVisitor)
    }
}

struct JsonValueVisitor;

impl<'de> Visitor<'de> for JsonValueVisitor {
    type Value = JsonValue;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("any JSON value")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(JsonValue::Bool(value))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(JsonValue::Int(value))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        if value <= i64::MAX as u64 {
            Ok(JsonValue::Int(value as i64))
        } else {
            Ok(JsonValue::Double(value as f64))
        }
    }

    fn visit_i128<E>(self, value: i128) -> Result<Self::Value, E> {
        Ok(i64::try_from(value).map_or_else(|_| JsonValue::Double(value as f64), JsonValue::Int))
    }

    fn visit_u128<E>(self, value: u128) -> Result<Self::Value, E> {
        Ok(i64::try_from(value).map_or_else(|_| JsonValue::Double(value as f64), JsonValue::Int))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E> {
        Ok(JsonValue::Double(value))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
        Ok(JsonValue::String(value.to_owned()))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(JsonValue::String(value))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(JsonValue::Null)
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(JsonValue::Null)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut items = Vec::new();
        while let Some(item) = seq.next_element()? {
            items.push(item);
        }
        Ok(JsonValue::Array(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut fields = BTreeMap::new();
        while let Some((key, value)) = map.next_entry::<String, JsonValue>()? {
            fields.insert(key, value);
        }
        Ok(JsonValue::Object(fields))
    }
}
