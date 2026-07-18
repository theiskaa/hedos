//! Tests for `ParamSpec` value normalization: type coercion, range clamping, and
//! the range accessors.

use kernel::records::{JsonValue, ParamSpec, ParamType};

fn make(
    param_type: ParamType,
    range: Option<Vec<JsonValue>>,
    values: Option<Vec<String>>,
) -> ParamSpec {
    ParamSpec {
        key: "p".to_owned(),
        param_type,
        default_value: None,
        range,
        values,
    }
}

fn int_range(lower: i64, upper: i64) -> Option<Vec<JsonValue>> {
    Some(vec![JsonValue::Int(lower), JsonValue::Int(upper)])
}

fn float_range(lower: f64, upper: f64) -> Option<Vec<JsonValue>> {
    Some(vec![JsonValue::Double(lower), JsonValue::Double(upper)])
}

#[test]
fn int_clamps_to_the_declared_range() {
    let spec = make(ParamType::Int, int_range(1, 10), None);
    assert_eq!(spec.normalized(&JsonValue::Int(5)), Some(JsonValue::Int(5)));
    assert_eq!(spec.normalized(&JsonValue::Int(0)), Some(JsonValue::Int(1)));
    assert_eq!(
        spec.normalized(&JsonValue::Int(99)),
        Some(JsonValue::Int(10))
    );
}

#[test]
fn int_without_a_range_passes_through() {
    let spec = make(ParamType::Int, None, None);
    assert_eq!(
        spec.normalized(&JsonValue::Int(1_000_000)),
        Some(JsonValue::Int(1_000_000))
    );
}

#[test]
fn int_coerces_a_float_by_rounding() {
    let spec = make(ParamType::Int, None, None);
    assert_eq!(
        spec.normalized(&JsonValue::Double(2.6)),
        Some(JsonValue::Int(3))
    );
    assert_eq!(
        spec.normalized(&JsonValue::Double(2.4)),
        Some(JsonValue::Int(2))
    );
    // Rounded first, then clamped.
    let ranged = make(ParamType::Int, int_range(0, 3), None);
    assert_eq!(
        ranged.normalized(&JsonValue::Double(9.9)),
        Some(JsonValue::Int(3))
    );
}

#[test]
fn int_rounds_ties_away_from_zero_and_handles_negatives() {
    let spec = make(ParamType::Int, None, None);
    // Ties round away from zero (matching Swift `.rounded()`), not to-even.
    assert_eq!(
        spec.normalized(&JsonValue::Double(2.5)),
        Some(JsonValue::Int(3))
    );
    assert_eq!(
        spec.normalized(&JsonValue::Double(0.5)),
        Some(JsonValue::Int(1))
    );
    assert_eq!(
        spec.normalized(&JsonValue::Double(-2.5)),
        Some(JsonValue::Int(-3))
    );
    assert_eq!(
        spec.normalized(&JsonValue::Double(-2.6)),
        Some(JsonValue::Int(-3))
    );
    assert_eq!(
        spec.normalized(&JsonValue::Double(-2.4)),
        Some(JsonValue::Int(-2))
    );
}

#[test]
fn int_rejects_non_numbers() {
    let spec = make(ParamType::Int, None, None);
    assert_eq!(spec.normalized(&JsonValue::String("5".to_owned())), None);
    assert_eq!(spec.normalized(&JsonValue::Bool(true)), None);
    assert_eq!(spec.normalized(&JsonValue::Null), None);
}

#[test]
fn float_clamps_and_coerces_from_int() {
    let spec = make(ParamType::Float, float_range(0.0, 1.0), None);
    assert_eq!(
        spec.normalized(&JsonValue::Double(0.5)),
        Some(JsonValue::Double(0.5))
    );
    assert_eq!(
        spec.normalized(&JsonValue::Double(-1.0)),
        Some(JsonValue::Double(0.0))
    );
    assert_eq!(
        spec.normalized(&JsonValue::Double(2.0)),
        Some(JsonValue::Double(1.0))
    );
    // An int coerces to a float.
    assert_eq!(
        spec.normalized(&JsonValue::Int(1)),
        Some(JsonValue::Double(1.0))
    );
}

#[test]
fn float_without_a_range_passes_through() {
    let spec = make(ParamType::Float, None, None);
    assert_eq!(
        spec.normalized(&JsonValue::Double(42.5)),
        Some(JsonValue::Double(42.5))
    );
}

#[test]
fn enum_accepts_only_listed_values() {
    let spec = make(
        ParamType::Enum,
        None,
        Some(vec!["low".to_owned(), "high".to_owned()]),
    );
    assert_eq!(
        spec.normalized(&JsonValue::String("high".to_owned())),
        Some(JsonValue::String("high".to_owned()))
    );
    assert_eq!(spec.normalized(&JsonValue::String("mid".to_owned())), None);
    assert_eq!(spec.normalized(&JsonValue::Int(1)), None);

    // An enum with no allowed set rejects everything.
    let empty = spec.clone();
    let empty = ParamSpec {
        values: None,
        ..empty
    };
    assert_eq!(
        empty.normalized(&JsonValue::String("high".to_owned())),
        None
    );
}

#[test]
fn bool_and_string_require_their_type() {
    let boolean = make(ParamType::Bool, None, None);
    assert_eq!(
        boolean.normalized(&JsonValue::Bool(false)),
        Some(JsonValue::Bool(false))
    );
    assert_eq!(
        boolean.normalized(&JsonValue::String("true".to_owned())),
        None
    );

    let string = make(ParamType::String, None, None);
    assert_eq!(
        string.normalized(&JsonValue::String("hi".to_owned())),
        Some(JsonValue::String("hi".to_owned()))
    );
    assert_eq!(string.normalized(&JsonValue::Int(1)), None);
}

#[test]
fn ranges_require_two_ordered_numeric_bounds() {
    assert_eq!(
        make(ParamType::Int, int_range(1, 10), None).int_range(),
        Some((1, 10))
    );
    // Reversed bounds are rejected.
    assert_eq!(
        make(ParamType::Int, int_range(10, 1), None).int_range(),
        None
    );
    // Wrong arity is rejected.
    let one = Some(vec![JsonValue::Int(1)]);
    assert_eq!(make(ParamType::Int, one, None).int_range(), None);
    let three = Some(vec![
        JsonValue::Int(1),
        JsonValue::Int(2),
        JsonValue::Int(3),
    ]);
    assert_eq!(make(ParamType::Int, three, None).int_range(), None);
    // Non-numeric bounds are rejected.
    let bad = Some(vec![JsonValue::String("a".to_owned()), JsonValue::Int(2)]);
    assert_eq!(make(ParamType::Int, bad, None).int_range(), None);
    // No range at all.
    assert_eq!(make(ParamType::Int, None, None).int_range(), None);
}

#[test]
fn double_range_coerces_int_bounds() {
    let spec = make(ParamType::Float, int_range(0, 2), None);
    assert_eq!(spec.double_range(), Some((0.0, 2.0)));
}

#[test]
fn double_range_rejects_bad_ranges() {
    assert_eq!(
        make(ParamType::Float, float_range(0.0, 1.0), None).double_range(),
        Some((0.0, 1.0))
    );
    // Reversed bounds.
    assert_eq!(
        make(ParamType::Float, float_range(1.0, 0.0), None).double_range(),
        None
    );
    // Wrong arity.
    let one = Some(vec![JsonValue::Double(1.0)]);
    assert_eq!(make(ParamType::Float, one, None).double_range(), None);
    // Non-numeric bound.
    let bad = Some(vec![JsonValue::Bool(true), JsonValue::Double(1.0)]);
    assert_eq!(make(ParamType::Float, bad, None).double_range(), None);
    // No range at all.
    assert_eq!(make(ParamType::Float, None, None).double_range(), None);
}

#[test]
fn a_float_range_clamps_an_out_of_range_int_value() {
    // Regression: an int value against a float spec should coerce then clamp.
    let spec = make(ParamType::Float, float_range(0.0, 1.0), None);
    assert_eq!(
        spec.normalized(&JsonValue::Int(5)),
        Some(JsonValue::Double(1.0))
    );
}
