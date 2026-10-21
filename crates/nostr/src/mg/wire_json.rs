//! Duplicate-safe JSON parsing without serde_json's feature-dependent synthetic
//! number maps. Raw tokens distinguish real objects from numbers even when a
//! downstream consumer unifies serde_json's `arbitrary_precision` feature.

use serde::de::{MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::value::RawValue;
use serde_json::{Map, Number, Value};

/// Complete JSON value with unambiguous object member names at every depth.
pub(super) struct UniqueValue(pub(super) Value);

impl<'de> Deserialize<'de> for UniqueValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = <&RawValue>::deserialize(deserializer)?;
        parse(raw, 0).map(Self).map_err(serde::de::Error::custom)
    }
}

fn parse(raw: &RawValue, depth: usize) -> Result<Value, serde_json::Error> {
    let text = raw.get();
    match text.as_bytes()[0] {
        b'{' | b'[' => {
            // serde_json's default budget starts at 128 and rejects a
            // container when decrementing it reaches zero. RawValue scanning
            // bypasses that budget, so apply the same 127-container maximum
            // explicitly across the recursive child parses.
            if depth >= 127 {
                return Err(serde::de::Error::custom("JSON recursion limit exceeded"));
            }
            let mut deserializer = serde_json::Deserializer::from_str(text);
            let visitor = ContainerVisitor { depth: depth + 1 };
            if text.starts_with('{') {
                deserializer.deserialize_map(visitor)
            } else {
                deserializer.deserialize_seq(visitor)
            }
        }
        b'"' => serde_json::from_str(text).map(Value::String),
        b't' | b'f' => serde_json::from_str(text).map(Value::Bool),
        b'n' => Ok(Value::Null),
        _ => {
            // Keep the pinned serde_json/JCS number convention, including
            // arbitrary_precision when enabled by a downstream crate. Never
            // interpret an object with serde_json's private marker as a number.
            let number: Number = serde_json::from_str(text)?;
            if !number.as_f64().is_some_and(f64::is_finite) {
                return Err(serde::de::Error::custom("nonfinite JSON number"));
            }
            Ok(Value::Number(number))
        }
    }
}

/// Visitor for a token already known to be an actual object or array.
struct ContainerVisitor {
    /// Number of enclosing containers, including this one.
    depth: usize,
}

impl<'de> Visitor<'de> for ContainerVisitor {
    type Value = Value;

    fn expecting(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("JSON container with unique object member names")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Self::Value, A::Error> {
        let mut values = Vec::new();
        while let Some(raw) = sequence.next_element::<&RawValue>()? {
            values.push(parse(raw, self.depth).map_err(serde::de::Error::custom)?);
        }
        Ok(Value::Array(values))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut values = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if values.contains_key(&key) {
                return Err(serde::de::Error::custom("duplicate JSON object member"));
            }
            let raw = map.next_value::<&RawValue>()?;
            values.insert(
                key,
                parse(raw, self.depth).map_err(serde::de::Error::custom)?,
            );
        }
        Ok(Value::Object(values))
    }
}
