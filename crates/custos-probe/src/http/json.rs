//! JSON content assertions for the HTTP probe: path extraction and
//! comparison operators over the response body.

use std::cmp::Ordering;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonAssertion {
    /// Dot-path like `$.status` or `items.0.name` (leading `$` optional).
    pub path: String,
    pub op: JsonOp,
    /// The comparison value as text; numeric ops coerce both sides.
    pub value: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonOp {
    /// The path must exist and equal the value (text or number).
    Equals,
    /// The path must exist and not equal the value.
    NotEquals,
    /// The path must exist (value ignored).
    Exists,
    /// The path must exist and be numerically greater than the value.
    GreaterThan,
    /// The path must exist and be numerically less than the value.
    LessThan,
}

impl JsonOp {
    pub(super) fn from_str(value: &str) -> Result<Self, String> {
        match value {
            "==" | "eq" => Ok(JsonOp::Equals),
            "!=" | "ne" => Ok(JsonOp::NotEquals),
            "exists" => Ok(JsonOp::Exists),
            ">" | "gt" => Ok(JsonOp::GreaterThan),
            "<" | "lt" => Ok(JsonOp::LessThan),
            other => Err(format!(
                "invalid `probe.json_assertions[].op` {other:?} (expected ==, !=, exists, >, <)"
            )),
        }
    }
}

pub fn json_path<'a>(root: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    let trimmed = path.trim_start_matches('$');
    let trimmed = trimmed.trim_start_matches('.');
    let mut current = root;
    for segment in trimmed.split('.') {
        if segment.is_empty() {
            return None;
        }
        current = match current {
            serde_json::Value::Object(map) => map.get(segment)?,
            serde_json::Value::Array(items) => items.get(segment.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(current)
}

pub fn check_json_assertion(root: &serde_json::Value, assertion: &JsonAssertion) -> Option<String> {
    let Some(actual) = json_path(root, &assertion.path) else {
        return Some(format!("json path {} missing", assertion.path));
    };
    // One total match over every operator (R18): the old pair of parallel
    // matches with `JsonOp::Exists => unreachable!()` was a landmine —
    // dead only while a dedicated outer arm intercepted `Exists` first,
    // a panic away under any refactor that shared the evaluation path.
    let equal = json_values_equal(actual, &assertion.value);
    let (op_symbol, ok) = match assertion.op {
        JsonOp::Exists => ("exists", true),
        JsonOp::Equals => ("==", equal),
        JsonOp::NotEquals => ("!=", !equal),
        JsonOp::GreaterThan => (">", json_greater(actual, &assertion.value)),
        JsonOp::LessThan => ("<", json_less(actual, &assertion.value)),
    };
    if ok {
        None
    } else {
        Some(format!(
            "json path {} = {actual} does not satisfy {op_symbol} {}",
            assertion.path, assertion.value
        ))
    }
}

fn json_values_equal(actual: &serde_json::Value, expected: &str) -> bool {
    match actual {
        serde_json::Value::String(text) => text == expected,
        serde_json::Value::Number(num) => num.to_string() == expected,
        serde_json::Value::Bool(flag) => flag.to_string() == expected,
        other => other == expected,
    }
}

fn numeric_of(value: &serde_json::Value) -> Option<f64> {
    match value {
        serde_json::Value::Number(num) => num.as_f64(),
        serde_json::Value::String(text) => text.trim().parse::<f64>().ok(),
        _ => None,
    }
}

fn json_compare(actual: &serde_json::Value, expected: &str, op: Ordering) -> bool {
    match (numeric_of(actual), expected.trim().parse::<f64>()) {
        (Some(left), Ok(right)) => left.partial_cmp(&right) == Some(op),
        _ => false,
    }
}

fn json_greater(actual: &serde_json::Value, expected: &str) -> bool {
    json_compare(actual, expected, Ordering::Greater)
}

fn json_less(actual: &serde_json::Value, expected: &str) -> bool {
    json_compare(actual, expected, Ordering::Less)
}
