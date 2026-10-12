use serde::ser::Error as _;
use serde::{Serialize, Serializer};
use serde_json::Value;

pub(super) fn serialize<S: Serializer>(value: &Value, serializer: S) -> Result<S::Ok, S::Error> {
    NativeJson(value).serialize(serializer)
}

// Bypass serde_json's arbitrary_precision number envelope and preserve_order
// map iteration while retaining the protocol's native values and lexical keys.
struct NativeJson<'a>(&'a Value);

impl Serialize for NativeJson<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            Value::Null => serializer.serialize_unit(),
            Value::Bool(value) => serializer.serialize_bool(*value),
            Value::String(value) => serializer.serialize_str(value),
            Value::Number(value) => {
                if let Some(value) = value.as_u64() {
                    serializer.serialize_u64(value)
                } else if let Some(value) = value.as_i64() {
                    serializer.serialize_i64(value)
                } else if value.is_f64() {
                    // Only decimal/exponent numbers may round to f64. An integer
                    // outside i64::MIN..=u64::MAX must not silently lose precision.
                    serializer
                        .serialize_f64(value.as_f64().expect("is_f64 guarantees a finite f64"))
                } else {
                    Err(S::Error::custom(
                        "unsupported info number: expected i64, u64, or finite f64",
                    ))
                }
            }
            Value::Array(values) => serializer.collect_seq(values.iter().map(NativeJson)),
            Value::Object(values) => {
                if values.keys().is_sorted() {
                    // The default BTreeMap representation needs no allocation.
                    serializer
                        .collect_map(values.iter().map(|(key, value)| (key, NativeJson(value))))
                } else {
                    let mut entries: Vec<_> = values.iter().collect();
                    entries.sort_unstable_by_key(|(key, _)| *key);
                    serializer.collect_map(
                        entries
                            .into_iter()
                            .map(|(key, value)| (key, NativeJson(value))),
                    )
                }
            }
        }
    }
}
