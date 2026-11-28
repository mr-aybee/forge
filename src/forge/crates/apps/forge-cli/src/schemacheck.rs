//! Client-side check of request bodies against the server's OpenAPI schemas
//! (issue #5, D-077).
//!
//! Covers the JSON Schema subset `schemars` emits: `$ref` into
//! `#/components/schemas`, `type` (one or a list, `null` included), `enum`,
//! `const`, `properties`, `required`, `additionalProperties: false`,
//! `items`, `anyOf`/`oneOf`/`allOf` and `minimum` for unsigned integers.
//! Anything else passes: the server checks everything again, this only
//! gives early, precise errors (`--no-validate` skips it).

use serde_json::{Map, Value};

use crate::error::{CliError, Result};

/// Most problems reported at once.
const MAX_PROBLEMS: usize = 20;

/// The operation of `method` whose path template matches `path` (`{x}`
/// matches one segment; a query string is ignored).
#[must_use]
pub fn find_operation<'a>(doc: &'a Value, method: &str, path: &str) -> Option<&'a Value> {
    let path = path.split('?').next().unwrap_or(path);
    let want: Vec<&str> = path.trim_end_matches('/').split('/').collect();

    doc["paths"].as_object()?.iter().find_map(|(template, item)| {
        let t: Vec<&str> = template.trim_end_matches('/').split('/').collect();
        let matches = t.len() == want.len()
            && t.iter().zip(&want).all(|(a, b)| (a.starts_with('{') && a.ends_with('}') && !b.is_empty()) || a == b);
        if matches { item.get(method.to_ascii_lowercase()) } else { None }
    })
}

/// The JSON request-body schema of an operation, if it is typed.
#[must_use]
pub fn request_schema(op: &Value) -> Option<&Value> {
    let s = op.pointer("/requestBody/content/application~1json/schema")?;

    // `{ "type": "object" }` alone is the untyped placeholder.
    (s.get("$ref").is_some() || s.get("properties").is_some()).then_some(s)
}

fn resolve<'a>(doc: &'a Value, schema: &'a Value) -> &'a Value {
    let mut s = schema;

    for _ in 0..32 {
        match s.get("$ref").and_then(Value::as_str) {
            Some(r) => match r.strip_prefix("#/") {
                Some(ptr) => match doc.pointer(&format!("/{ptr}")) {
                    Some(t) => s = t,
                    None => return &Value::Bool(true),
                },
                None => return &Value::Bool(true),
            },
            None => return s,
        }
    }

    s
}

fn type_ok(t: &str, v: &Value) -> bool {
    match t {
        "null" => v.is_null(),
        "boolean" => v.is_boolean(),
        "string" => v.is_string(),
        "integer" => v.is_i64() || v.is_u64(),
        "number" => v.is_number(),
        "array" => v.is_array(),
        "object" => v.is_object(),
        _ => true,
    }
}

fn kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(n) if n.is_i64() || n.is_u64() => "integer",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn check(doc: &Value, schema: &Value, v: &Value, at: &str, out: &mut Vec<String>) {
    if out.len() >= MAX_PROBLEMS {
        return;
    }

    let s = resolve(doc, schema);
    let Some(obj) = s.as_object() else { return };
    let here = if at.is_empty() { "the body".to_owned() } else { format!("`{at}`") };

    if let Some(t) = obj.get("type") {
        let types: Vec<&str> = match t {
            Value::String(x) => vec![x.as_str()],
            Value::Array(a) => a.iter().filter_map(Value::as_str).collect(),
            _ => Vec::new(),
        };

        if !types.is_empty() && !types.iter().any(|t| type_ok(t, v)) {
            out.push(format!("{here} must be {} (got {})", types.join(" or "), kind(v)));
            return;
        }
    }

    if let Some(Value::Array(allowed)) = obj.get("enum")
        && !allowed.contains(v)
    {
        let list: Vec<String> = allowed.iter().map(Value::to_string).collect();
        out.push(format!("{here} must be one of {} (got {v})", list.join(", ")));

        return;
    }

    if let Some(c) = obj.get("const")
        && c != v
    {
        out.push(format!("{here} must be {c} (got {v})"));
        return;
    }

    if let (Some(min), Some(n)) = (obj.get("minimum").and_then(Value::as_f64), v.as_f64())
        && n < min
    {
        out.push(format!("{here} must be at least {min}"));
    }

    for (key, all) in [("anyOf", false), ("oneOf", false), ("allOf", true)] {
        if let Some(Value::Array(subs)) = obj.get(key) {
            let results: Vec<Vec<String>> = subs
                .iter()
                .map(|sub| {
                    let mut e = Vec::new();
                    check(doc, sub, v, at, &mut e);
                    e
                })
                .collect();

            if all {
                out.extend(results.into_iter().flatten());
            } else if !results.iter().any(Vec::is_empty) {
                // Report the closest alternative.
                if let Some(best) = results.into_iter().min_by_key(Vec::len) {
                    out.extend(best);
                }
            }
        }
    }

    match v {
        Value::Object(map) => check_object(doc, obj, map, at, out),
        Value::Array(items) => {
            if let Some(item_schema) = obj.get("items") {
                for (i, item) in items.iter().enumerate() {
                    check(doc, item_schema, item, &format!("{at}[{i}]"), out);
                }
            }
        }
        _ => {}
    }
}

fn check_object(doc: &Value, s: &Map<String, Value>, map: &Map<String, Value>, at: &str, out: &mut Vec<String>) {
    let props = s.get("properties").and_then(Value::as_object);

    for req in s.get("required").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
        if !map.contains_key(req) {
            out.push(format!("`{}` is required", join(at, req)));
        }
    }

    for (k, val) in map {
        match props.and_then(|p| p.get(k)) {
            Some(ps) => check(doc, ps, val, &join(at, k), out),
            None => {
                if s.get("additionalProperties") == Some(&Value::Bool(false)) {
                    let known: Vec<&str> = props.map(|p| p.keys().map(String::as_str).collect()).unwrap_or_default();
                    out.push(format!("`{}` is not a field here (known: {})", join(at, k), known.join(", ")));
                } else if let Some(extra) = s.get("additionalProperties").filter(|x| x.is_object()) {
                    check(doc, extra, val, &join(at, k), out);
                }
            }
        }
    }
}

fn join(at: &str, key: &str) -> String {
    if at.is_empty() { key.to_owned() } else { format!("{at}.{key}") }
}

/// Checks `body` against the request schema of `method path` in `doc`.
/// Untyped operations and unknown paths pass.
///
/// # Errors
/// A usage error listing the problems (exit `2`, code `invalid_body`).
pub fn validate(doc: &Value, method: &str, path: &str, body: &Value) -> Result<()> {
    let Some(op) = find_operation(doc, method, path) else { return Ok(()) };
    let Some(schema) = request_schema(op) else { return Ok(()) };
    let mut problems = Vec::new();
    check(doc, schema, body, "", &mut problems);

    if problems.is_empty() {
        return Ok(());
    }

    let id = op["operationId"].as_str().unwrap_or("this operation");

    Err(CliError::usage(format!(
        "the body does not match {id}'s schema (`forge api describe {id}`): {}{}",
        problems.join("; "),
        if problems.len() >= MAX_PROBLEMS { "; …" } else { "" }
    ))
    .with_code("invalid_body"))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn doc() -> Value {
        json!({
            "paths": {
                "/api/v1/things/{id}": {
                    "put": {
                        "operationId": "things.update",
                        "requestBody": { "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Thing" } } } }
                    }
                },
                "/api/v1/loose": { "post": { "requestBody": { "content": { "application/json": { "schema": { "type": "object" } } } } } }
            },
            "components": { "schemas": {
                "Thing": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["name", "mode"],
                    "properties": {
                        "name": { "type": "string" },
                        "mode": { "$ref": "#/components/schemas/Mode" },
                        "size": { "type": ["integer", "null"], "minimum": 0 },
                        "tags": { "type": "array", "items": { "type": "string" } }
                    }
                },
                "Mode": { "type": "string", "enum": ["read_only", "ask", "auto"] }
            } }
        })
    }

    #[test]
    fn good_bodies_pass_and_bad_ones_say_why() {
        let d = doc();
        assert!(validate(&d, "PUT", "/api/v1/things/42", &json!({ "name": "x", "mode": "ask", "size": null })).is_ok());
        let e = validate(
            &d,
            "PUT",
            "/api/v1/things/42?x=1",
            &json!({ "mode": "nope", "size": -1, "tags": [1], "oops": true }),
        )
        .err()
        .map(|e| e.message)
        .unwrap_or_default();
        assert!(e.contains("`name` is required"), "{e}");
        assert!(e.contains("`mode` must be one of"), "{e}");
        assert!(e.contains("`size` must be at least 0"), "{e}");
        assert!(e.contains("`tags[0]` must be string"), "{e}");
        assert!(e.contains("`oops` is not a field here"), "{e}");
        assert!(e.contains("things.update"), "{e}");

        // Untyped operations and unknown paths are not checked.
        assert!(validate(&d, "POST", "/api/v1/loose", &json!([1, 2])).is_ok());
        assert!(validate(&d, "POST", "/api/v1/unknown", &json!(1)).is_ok());
        assert!(validate(&d, "PUT", "/api/v1/things/42", &json!("text")).is_err());
    }
}
