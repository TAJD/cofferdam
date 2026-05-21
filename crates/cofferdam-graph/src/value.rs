//! Attribute payload for canonical-graph nodes and edges.
//!
//! `Value` and `AttrMap` carry the data that adapter-defined
//! `Extension { ns, kind, attrs }` nodes and edges need without
//! widening the closed core taxonomy. Every variant is
//! JSON-roundtrippable so the graph can be serialised to disk for
//! incremental analysis (cd-9hp.4) and to the SARIF / JSON output
//! formats.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use smol_str::SmolStr;

/// Map from attribute name to value. `BTreeMap` (not `HashMap`) so the
/// canonical order of keys is deterministic — required for stable
/// [`crate::NodeId`] computation.
pub type AttrMap = BTreeMap<SmolStr, Value>;

/// A graph-attribute value. The shape is intentionally narrow so
/// adapters don't smuggle arbitrary structure through; if a new
/// adapter needs a richer payload, raise it as a MINOR schema bump.
///
/// Variants:
/// - [`Value::String`] — UTF-8 string; the common case for names,
///   specifiers, glob patterns.
/// - [`Value::Int`] — signed 64-bit integer. Wider than counts need
///   today, but symmetric with JSON's number domain.
/// - [`Value::Bool`] — primitive flags (e.g. `type_only` on import
///   edges, `frozen` on boundary extensions).
/// - [`Value::List`] — homogeneous-or-heterogeneous list of values.
///   Used sparingly; prefer flattened key/value pairs when possible.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Value {
    String(SmolStr),
    Int(i64),
    Bool(bool),
    List(Vec<Value>),
}

impl Value {
    /// True for variants whose payload is empty (`String("")`,
    /// `List([])`). Used by [`crate::id::compute_node_id`] to skip
    /// keys whose contribution to identity is empty — keeps the
    /// hash stable when adapters add and then clear an attribute.
    pub fn is_empty(&self) -> bool {
        match self {
            Value::String(s) => s.is_empty(),
            Value::List(xs) => xs.is_empty(),
            Value::Int(_) | Value::Bool(_) => false,
        }
    }
}

impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::String(SmolStr::new(s))
    }
}

impl From<String> for Value {
    fn from(s: String) -> Self {
        Value::String(SmolStr::new(s))
    }
}

impl From<SmolStr> for Value {
    fn from(s: SmolStr) -> Self {
        Value::String(s)
    }
}

impl From<i64> for Value {
    fn from(i: i64) -> Self {
        Value::Int(i)
    }
}

impl From<bool> for Value {
    fn from(b: bool) -> Self {
        Value::Bool(b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_each_variant() {
        let cases: Vec<Value> = vec![
            Value::String(SmolStr::new("hello")),
            Value::Int(42),
            Value::Int(-7),
            Value::Bool(true),
            Value::Bool(false),
            Value::List(vec![Value::Int(1), Value::String(SmolStr::new("a"))]),
        ];
        for v in cases {
            let s = serde_json::to_string(&v).expect("serialize");
            let r: Value = serde_json::from_str(&s).expect("deserialize");
            assert_eq!(v, r, "round-trip failed for {v:?} (json: {s})");
        }
    }

    #[test]
    fn attrmap_serialises_as_json_object() {
        let mut m: AttrMap = AttrMap::new();
        m.insert(SmolStr::new("name"), Value::from("widget"));
        m.insert(SmolStr::new("count"), Value::from(3_i64));
        let s = serde_json::to_string(&m).expect("serialize");
        // BTreeMap ordering means keys are sorted alphabetically.
        assert_eq!(s, r#"{"count":3,"name":"widget"}"#);
    }

    #[test]
    fn is_empty_detects_zero_payload() {
        assert!(Value::String(SmolStr::new("")).is_empty());
        assert!(Value::List(vec![]).is_empty());
        assert!(!Value::String(SmolStr::new("x")).is_empty());
        assert!(!Value::Int(0).is_empty(), "0 is a real value, not empty");
        assert!(!Value::Bool(false).is_empty(), "false is a real value");
    }

    #[test]
    fn from_impls_cover_common_cases() {
        let _: Value = "x".into();
        let _: Value = String::from("y").into();
        let _: Value = 7i64.into();
        let _: Value = true.into();
    }
}
