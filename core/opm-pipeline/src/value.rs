//! JSON-shaped value type used for pipeline defaults and runtime params.
//!
//! Replaces `serde_json::Value` in the opm-pipeline API so that nonfinite
//! floats (YAML `.nan`/`.inf`, runtime `NaN`/`Infinity`) stay representable in
//! memory and produce the exact validation blockers pinned by the Python
//! contract (validation rules for non-finite values and bounds).

use serde::de::{self, Deserialize, DeserializeSeed, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};
use std::collections::BTreeMap;
use std::fmt;

use crate::pyfmt::py_float;

/// A JSON/YAML-shaped value.
///
/// Wire representation matches JSON: `Int`/`Float` serialize as JSON numbers,
/// `Map` as a JSON object. Nonfinite `Float` values serialize only through
/// formats that can express them (YAML emits `.nan`/`.inf`); strict JSON
/// serialization rejects them, mirroring `ParseError::InvalidJson` on parse.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Seq(Vec<Value>),
    Map(BTreeMap<String, Value>),
}

impl Serialize for Value {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Value::Null => serializer.serialize_none(),
            Value::Bool(b) => serializer.serialize_bool(*b),
            Value::Int(i) => serializer.serialize_i64(*i),
            Value::Float(f) => serializer.serialize_f64(*f),
            Value::Str(s) => serializer.serialize_str(s),
            Value::Seq(items) => {
                let mut seq = serializer.serialize_seq(Some(items.len()))?;
                for item in items {
                    seq.serialize_element(item)?;
                }
                seq.end()
            }
            Value::Map(map) => {
                let mut out = serializer.serialize_map(Some(map.len()))?;
                for (key, value) in map {
                    out.serialize_entry(key, value)?;
                }
                out.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for Value {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(ValueVisitor)
    }
}

struct ValueVisitor;

impl<'de> Visitor<'de> for ValueVisitor {
    type Value = Value;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON/YAML value")
    }

    fn visit_bool<E: de::Error>(self, v: bool) -> Result<Value, E> {
        Ok(Value::Bool(v))
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<Value, E> {
        Ok(Value::Int(v))
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<Value, E> {
        match i64::try_from(v) {
            Ok(i) => Ok(Value::Int(i)),
            // Wider than i64: keep the number (Python ints are unbounded);
            // value semantics beyond 2^63 are not exercised by the contract.
            Err(_) => Ok(Value::Float(v as f64)),
        }
    }

    fn visit_f64<E: de::Error>(self, v: f64) -> Result<Value, E> {
        Ok(Value::Float(v))
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<Value, E> {
        Ok(Value::Str(v.to_owned()))
    }

    fn visit_string<E: de::Error>(self, v: String) -> Result<Value, E> {
        Ok(Value::Str(v))
    }

    fn visit_none<E: de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_unit<E: de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_some<D: Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        Value::deserialize(deserializer)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        let mut items = Vec::new();
        while let Some(item) = seq.next_element()? {
            items.push(item);
        }
        Ok(Value::Seq(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        let mut out = BTreeMap::new();
        while let Some((key, value)) = map.next_entry_seed(KeyString, PhantomValue)? {
            out.insert(key, value);
        }
        Ok(Value::Map(out))
    }
}

/// Non-string map keys (legal in YAML) are stringified the way Python's
/// `str()` would render them, so unknown top-level keys keep flowing into
/// Pydantic-style "ignore unknown fields" instead of failing the parse.
struct KeyString;

impl<'de> DeserializeSeed<'de> for KeyString {
    type Value = String;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<String, D::Error> {
        deserializer.deserialize_any(KeyVisitor)
    }
}

struct KeyVisitor;

impl<'de> Visitor<'de> for KeyVisitor {
    type Value = String;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a map key")
    }

    fn visit_bool<E: de::Error>(self, v: bool) -> Result<String, E> {
        Ok(if v { "True" } else { "False" }.to_owned())
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<String, E> {
        Ok(v.to_string())
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<String, E> {
        Ok(v.to_string())
    }

    fn visit_f64<E: de::Error>(self, v: f64) -> Result<String, E> {
        Ok(py_float(v))
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<String, E> {
        Ok(v.to_owned())
    }

    fn visit_string<E: de::Error>(self, v: String) -> Result<String, E> {
        Ok(v)
    }

    fn visit_unit<E: de::Error>(self) -> Result<String, E> {
        Ok("None".to_owned())
    }
}

/// Value seed used as a map `MapAccess` value (the value type is already
/// fixed: `Value`).
struct PhantomValue;

impl<'de> DeserializeSeed<'de> for PhantomValue {
    type Value = Value;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        Value::deserialize(deserializer)
    }
}
