use serde_json::{Map, Value};

/// Returns a copy of `v` with object keys sorted recursively, so hashing does
/// not depend on insertion order or on serde_json's `preserve_order` feature.
pub(crate) fn sort_keys(v: &Value) -> Value {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            let mut out = Map::new();

            for k in keys {
                out.insert(k.clone(), sort_keys(&m[k]));
            }

            Value::Object(out)
        }
        Value::Array(a) => Value::Array(a.iter().map(sort_keys).collect()),
        other => other.clone(),
    }
}
