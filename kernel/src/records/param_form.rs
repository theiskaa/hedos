//! Parameter normalization: coercing a wire value to a [`ParamSpec`]'s type and
//! clamping numbers to its declared range. `merged` runs saved parameter values
//! through [`ParamSpec::normalized`] so an out-of-range or wrong-typed value can
//! never reach a runtime.

use crate::records::{JsonValue, ParamSpec, ParamType};

impl ParamSpec {
    /// The integer `(min, max)` range, if the spec declares a two-element numeric
    /// range with `min <= max`.
    pub fn int_range(&self) -> Option<(i64, i64)> {
        let [lower, upper] = self.range.as_deref()? else {
            return None;
        };
        let lower = int_scalar(lower)?;
        let upper = int_scalar(upper)?;
        (lower <= upper).then_some((lower, upper))
    }

    /// The floating-point `(min, max)` range, if the spec declares a two-element
    /// numeric range with `min <= max`.
    pub fn double_range(&self) -> Option<(f64, f64)> {
        let [lower, upper] = self.range.as_deref()? else {
            return None;
        };
        let lower = double_scalar(lower)?;
        let upper = double_scalar(upper)?;
        (lower <= upper).then_some((lower, upper))
    }

    /// Coerce and clamp `value` to this spec, or `None` if it cannot be a value of
    /// this type. Ints and floats are clamped to the range when one is declared
    /// (a float coerces from an int and vice versa); an enum must be a listed
    /// value; a bool or string must already match the type.
    pub fn normalized(&self, value: &JsonValue) -> Option<JsonValue> {
        match self.param_type {
            ParamType::Int => {
                let raw = int_scalar(value)?;
                let clamped = match self.int_range() {
                    Some((lower, upper)) => raw.clamp(lower, upper),
                    None => raw,
                };
                Some(JsonValue::Int(clamped))
            }
            ParamType::Float => {
                let raw = double_scalar(value)?;
                // `max().min()` rather than `clamp()` so a stray non-finite value
                // can't panic; finite values clamp identically.
                let clamped = match self.double_range() {
                    Some((lower, upper)) => raw.max(lower).min(upper),
                    None => raw,
                };
                Some(JsonValue::Double(clamped))
            }
            ParamType::Enum => match value {
                JsonValue::String(raw)
                    if self
                        .values
                        .as_ref()
                        .is_some_and(|allowed| allowed.contains(raw)) =>
                {
                    Some(JsonValue::String(raw.clone()))
                }
                _ => None,
            },
            ParamType::Bool => match value {
                JsonValue::Bool(raw) => Some(JsonValue::Bool(*raw)),
                _ => None,
            },
            ParamType::String => match value {
                JsonValue::String(raw) => Some(JsonValue::String(raw.clone())),
                _ => None,
            },
        }
    }
}

/// A value read as an integer: an int as-is, a float rounded to nearest.
fn int_scalar(value: &JsonValue) -> Option<i64> {
    match value {
        JsonValue::Int(raw) => Some(*raw),
        // `round()` is half-away-from-zero. A non-finite or out-of-range double
        // saturates here rather than trapping.
        JsonValue::Double(raw) => Some(raw.round() as i64),
        _ => None,
    }
}

/// A value read as a double: a double as-is, an int widened.
fn double_scalar(value: &JsonValue) -> Option<f64> {
    match value {
        JsonValue::Int(raw) => Some(*raw as f64),
        JsonValue::Double(raw) => Some(*raw),
        _ => None,
    }
}
