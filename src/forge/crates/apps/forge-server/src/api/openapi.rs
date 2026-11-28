//! `GET /api/openapi.json`: an OpenAPI 3.1 description of the whole API
//! (GitHub issue #5, phase 0).
//!
//! Generated from the route inventory scanned at build time
//! (`build_routes.rs`), so every route appears without a hand-kept list.
//! Each operation carries Forge metadata the CLI, MCP tools and parity
//! checks use: `x-forge-action` (Cedar action the handler authorizes),
//! `x-forge-mfa` (step-up guardrail, read from the policy itself),
//! `x-forge-transport` and `x-forge-handler`. Request bodies, query
//! parameters and answers are typed where the handler's module defines the
//! type with `#[derive(JsonSchema)]` (D-077); `x-forge-schema` says `typed`,
//! `partial` or `untyped`, and the types live under `components/schemas`.

use std::collections::{BTreeMap, HashSet};
use std::sync::OnceLock;

use axum::Json;
use forge_core::Action;
use serde_json::{Map, Value, json};

/// One route from the build-time scan.
#[derive(Debug, Clone, Copy)]
pub struct RouteInfo {
    pub method: &'static str,
    pub path: &'static str,
    pub module: &'static str,
    pub handler: &'static str,
    pub summary: &'static str,
    /// Rust identifier of the first `Action::…` the handler uses.
    pub action: Option<&'static str>,
    /// The handler takes a `Caller` (signed-in user, token or API key).
    pub authenticated: bool,
    /// `json`, `websocket`, `sse` or `stream`.
    pub transport: &'static str,
    /// The `Json<…>` body type as written (`None`: no JSON body).
    pub request: Option<&'static str>,
    /// The `Query<…>` type as written.
    pub query: Option<&'static str>,
    /// The `Json<…>` answer type as written.
    pub response: Option<&'static str>,
}

/// Whether a type as written is an untyped JSON value.
fn is_value(t: &str) -> bool {
    matches!(t, "Value" | "serde_json::Value")
}

include!(concat!(env!("OUT_DIR"), "/routes.rs"));

/// Cedar name (`node.onboard`) of an `Action` identifier (`NodeOnboard`).
fn action_name(ident: &str) -> Option<&'static str> {
    Action::ALL.iter().find(|a| format!("{a:?}") == ident).map(|a| a.as_str())
}

/// Actions under the step-up MFA guardrail, read from the policy source.
/// Routes that check for an MFA-verified session in the handler
/// (`Caller::require_mfa`) although their Cedar action is not under the
/// guardrail: handing out access (#13 inventory).
pub const MFA_HANDLERS: &[&str] = &["tenancy::create_user", "tenancy::create_binding", "access::assign"];

pub fn mfa_actions() -> &'static HashSet<String> {
    static SET: OnceLock<HashSet<String>> = OnceLock::new();
    SET.get_or_init(|| {
        let src = forge_authz::GUARDRAILS_SRC;
        let Some(start) = src.find("guard:sensitive-requires-mfa") else { return HashSet::new() };
        let block = &src[start..];
        let list = block.find("action in [").map(|i| &block[i..]).unwrap_or_default();
        let list = &list[..list.find(']').unwrap_or(list.len())];
        list.split("Forge::Action::\"").skip(1).filter_map(|s| s.split('"').next()).map(str::to_owned).collect()
    })
}

fn path_params(path: &str) -> Vec<Value> {
    path.split('/')
        .filter_map(|seg| seg.strip_prefix('{').and_then(|s| s.strip_suffix('}')))
        .map(|name| json!({ "name": name, "in": "path", "required": true, "schema": { "type": "string" } }))
        .collect()
}

/// The OpenAPI document (also used by tests and the CLI parity check).
#[must_use]
pub fn document(server_url: &str) -> Value {
    let mfa = mfa_actions();
    let mut paths: BTreeMap<&str, Map<String, Value>> = BTreeMap::new();
    let mut ids: HashSet<String> = HashSet::new();
    let mut settings = schemars::generate::SchemaSettings::draft2020_12();
    settings.definitions_path = "/components/schemas/".into();
    let mut generator = settings.into_generator();

    for (i, r) in ROUTES.iter().enumerate() {
        let [req_schema, query_schema, resp_schema] = route_schemas(i, &mut generator);

        if paths.get(r.path).is_some_and(|m| m.contains_key(&r.method.to_lowercase())) {
            continue; // registered twice (see DUPLICATE_ROUTES)
        }

        let mut id = format!("{}.{}", r.module, r.handler);
        if !ids.insert(id.clone()) {
            id = format!("{id}.{}", r.method.to_lowercase());
            ids.insert(id.clone());
        }

        let action = r.action.and_then(action_name);
        let needs_mfa = action.is_some_and(|a| mfa.contains(a))
            || MFA_HANDLERS.contains(&format!("{}::{}", r.module, r.handler).as_str());
        let mut op = json!({
            "operationId": id,
            "summary": if r.summary.is_empty() { format!("{} {}", r.method, r.path) } else { r.summary.to_owned() },
            "tags": [r.module],
            "parameters": path_params(r.path),
            "responses": {
                "200": { "description": "Success" },
                "default": {
                    "description": "Error (RFC 9457 problem details)",
                    "content": { "application/problem+json": { "schema": { "$ref": "#/components/schemas/Problem" } } }
                }
            },
            "security": if r.authenticated { json!([{ "bearer": [] }, { "session": [] }]) } else { json!([]) },
            "x-forge-handler": format!("{}::{}", r.module, r.handler),
            "x-forge-action": action,
            // Where no static action was found, the handler decides at run
            // time (per command, job kind or file operation) or acts only on
            // the caller's own account.
            "x-forge-authz": if !r.authenticated { "public" } else if action.is_some() { "static" } else { "in-handler" },
            "x-forge-mfa": needs_mfa,
            "x-forge-transport": r.transport,
            "x-forge-schema": schema_state(r, req_schema.is_some(), query_schema.is_some(), resp_schema.is_some()),
        });

        if let Some(o) = op.as_object_mut() {
            let body_needed = r.request.is_some_and(|t| !is_value(t));

            match (req_schema, r.request) {
                (Some(s), _) => {
                    o.insert(
                        "requestBody".into(),
                        json!({ "required": body_needed, "content": { "application/json": { "schema": s } } }),
                    );
                }
                (None, Some(_)) => {
                    o.insert(
                        "requestBody".into(),
                        json!({ "required": body_needed, "content": { "application/json": { "schema": { "type": "object" } } } }),
                    );
                }
                (None, None) if matches!(r.method, "POST" | "PUT" | "PATCH") => {
                    o.insert(
                        "requestBody".into(),
                        json!({ "required": false, "content": { "application/json": { "schema": { "type": "object" } } } }),
                    );
                }
                (None, None) => {}
            }

            if let Some(q) = query_schema
                && let Some(params) = o.get_mut("parameters").and_then(Value::as_array_mut)
            {
                params.extend(query_params(&q, &generator));
            }

            if let Some(s) = resp_schema {
                o.insert(
                    "responses".into(),
                    json!({
                        "200": { "description": "Success", "content": { "application/json": { "schema": s } } },
                        "default": {
                            "description": "Error (RFC 9457 problem details)",
                            "content": { "application/problem+json": { "schema": { "$ref": "#/components/schemas/Problem" } } }
                        }
                    }),
                );
            }
        }

        paths.entry(r.path).or_default().insert(r.method.to_lowercase(), op);
    }

    json!({
        "openapi": "3.1.0",
        "info": {
            "title": "Orbyn Forge API",
            "version": crate::VERSION,
            "description": "Generated from the server's routes. Authorization is enforced by the server (Cedar); `x-forge-*` fields describe it.",
        },
        "servers": [{ "url": server_url }],
        "paths": paths,
        "components": {
            "securitySchemes": {
                "bearer": { "type": "http", "scheme": "bearer", "description": "OAuth access token (CLI, apps) or personal API key (`fk_…`)" },
                "session": { "type": "apiKey", "in": "cookie", "name": "forge_session", "description": "Console session (plus the CSRF header on writes)" }
            },
            "schemas": schemas_with_problem(generator.take_definitions(true)),
        }
    })
}

/// `typed`: every JSON part the route has is typed; `partial`: some;
/// `untyped`: none (or it has no JSON parts besides `Value`).
fn schema_state(r: &RouteInfo, req: bool, query: bool, resp: bool) -> &'static str {
    let wanted = [
        (r.request.is_some_and(|t| !is_value(t)), req),
        (r.query.is_some(), query),
        (r.response.is_some_and(|t| !is_value(t)), resp),
    ];
    let want = wanted.iter().filter(|(w, _)| *w).count();
    let have = wanted.iter().filter(|(w, h)| *w && *h).count();
    if want == 0 || have == 0 {
        "untyped"
    } else if have == want {
        "typed"
    } else {
        "partial"
    }
}

/// Query parameters from a query type's schema (its properties, or the
/// properties of the definition it refers to).
fn query_params(schema: &schemars::Schema, generator: &schemars::SchemaGenerator) -> Vec<Value> {
    let v = schema.as_value();
    let resolved = v
        .get("$ref")
        .and_then(Value::as_str)
        .and_then(|r| r.rsplit('/').next())
        .and_then(|name| generator.definitions().get(name).cloned())
        .unwrap_or_else(|| v.clone());
    let required: HashSet<&str> = resolved
        .get("required")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();

    resolved
        .get("properties")
        .and_then(Value::as_object)
        .map(|props| {
            props
                .iter()
                .map(|(name, s)| {
                    let mut p = json!({ "name": name, "in": "query", "required": required.contains(name.as_str()), "schema": s });

                    if let Some(d) = s.get("description") {
                        p["description"] = d.clone();
                    }

                    p
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The generated definitions plus the shared `Problem` schema.
fn schemas_with_problem(defs: serde_json::Map<String, Value>) -> Value {
    let mut all = defs;
    all.insert(
        "Problem".into(),
        json!({
            "type": "object",
            "properties": {
                "type": { "type": "string" },
                "title": { "type": "string" },
                "status": { "type": "integer" },
                "detail": { "type": "string" }
            }
        }),
    );

    Value::Object(all)
}

/// `GET /api/openapi.json` (public, like `/api/version`).
pub async fn spec(origin: crate::request_origin::RequestOrigin) -> Json<Value> {
    Json(document(&origin.origin()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_route_is_described_with_a_handler() {
        assert!(ROUTES.len() > 50, "the scan found only {} routes", ROUTES.len());
        let doc = document("http://x");
        let paths = doc["paths"].as_object().unwrap_or(&Map::new()).clone();

        // Known routes are present with the right metadata.
        assert!(paths.contains_key("/api/v1/me"));
        assert!(paths.contains_key("/oauth/token"));
        assert!(paths.contains_key("/api/openapi.json"));
        let me = &paths["/api/v1/me"]["get"];
        assert_eq!(me["operationId"], "session.me");
        assert_ne!(me["security"], json!([]));
        assert_eq!(paths["/api/version"]["get"]["security"], json!([]));

        // Unique operation ids, every handler resolved.
        let mut seen = HashSet::new();

        for (p, item) in &paths {
            for (m, op) in item.as_object().into_iter().flatten() {
                assert!(
                    seen.insert(op["operationId"].as_str().unwrap_or_default().to_owned()),
                    "duplicate id at {m} {p}"
                );
                assert!(op["x-forge-handler"].as_str().is_some_and(|h| h.contains("::")), "{m} {p}");
            }
        }
    }

    /// Issue #5 parity gate: every route is classified in the CLI's
    /// `parity.json` (typed command, `forge api call` only, or excluded, each
    /// with its command or reason), and no entry names a route that is gone.
    #[test]
    fn every_route_is_classified_for_the_cli() {
        let parity: Value = serde_json::from_str(include_str!("../../../forge-cli/parity.json")).unwrap_or(Value::Null);
        let mut classified = HashSet::new();

        for class in ["command", "api", "excluded"] {
            for (k, v) in parity[class].as_object().into_iter().flatten() {
                assert!(v.as_str().is_some_and(|s| !s.trim().is_empty()), "{k}: no command or reason");
                assert!(classified.insert(k.clone()), "{k} is classified twice");
            }
        }

        let doc = document("http://x");
        let mut live = HashSet::new();

        for (p, item) in doc["paths"].as_object().into_iter().flatten() {
            for m in item.as_object().into_iter().flatten().map(|(m, _)| m.to_uppercase()) {
                live.insert(format!("{m} {p}"));
            }
        }

        let mut missing: Vec<&String> = live.difference(&classified).collect();
        missing.sort();
        assert!(
            missing.is_empty(),
            "routes missing from src/forge/crates/apps/forge-cli/parity.json (add a command, or an `api`/`excluded` reason): {missing:?}"
        );
        let mut stale: Vec<&String> = classified.difference(&live).collect();
        stale.sort();
        assert!(stale.is_empty(), "parity.json names routes that no longer exist: {stale:?}");
    }

    /// JSON request bodies without a field-level schema (D-077): a large
    /// forge-catalog type that forge-agent shares, so it cannot derive
    /// `JsonSchema` (schemars stays out of the agent's build) and is too big
    /// to mirror safely. The cluster-profile reference page documents its
    /// fields. Nothing else may be added here.
    const UNTYPED_BODIES: &[&str] = &["PUT /api/v1/clusters/{id}/profile"];

    /// Typed-schema gate (D-077): every JSON request body has a schema unless
    /// it is listed above, and nothing listed is typed already.
    #[test]
    fn json_request_bodies_are_typed() {
        let mut g = schemars::generate::SchemaSettings::draft2020_12().into_generator();
        let (mut missing, mut stale) = (Vec::new(), Vec::new());

        for (i, r) in ROUTES.iter().enumerate() {
            let Some(t) = r.request else { continue };
            if is_value(t) {
                continue;
            }

            let typed = route_schemas(i, &mut g)[0].is_some();
            let key = format!("{} {}", r.method, r.path);
            let listed = UNTYPED_BODIES.contains(&key.as_str());
            if !typed && !listed {
                missing.push(format!("{key} ({t})"));
            } else if typed && listed {
                stale.push(key);
            }
        }

        assert!(missing.is_empty(), "derive JsonSchema for these request bodies: {missing:?}");
        assert!(stale.is_empty(), "typed now; remove from UNTYPED_BODIES: {stale:?}");

        // The document still builds with the schemas in components, and a
        // typed body refers to its component.
        let doc = document("http://x");
        assert!(doc["components"]["schemas"]["Problem"].is_object());
        let save = &doc["paths"]["/api/v1/servers/{id}/files/save"]["post"];
        assert_eq!(save["x-forge-schema"], "typed", "{save}");
        let r = save["requestBody"]["content"]["application/json"]["schema"]["$ref"].as_str().unwrap_or_default();
        let name = r.strip_prefix("#/components/schemas/").unwrap_or_default();
        let def = &doc["components"]["schemas"][name];
        assert!(def["properties"].is_object(), "{r}: {def}");

        // Ids are plain strings; enums keep their values.
        let jobs = &doc["paths"]["/api/v1/jobs"]["get"]["parameters"];
        let status = jobs.as_array().into_iter().flatten().find(|p| p["name"] == "status");
        assert!(status.is_some(), "{jobs}");
    }

    #[test]
    fn no_route_is_registered_twice() {
        // axum panics when building a router with overlapping routes; catch
        // it here with the handler names instead of in every integration test.
        assert!(DUPLICATE_ROUTES.is_empty(), "routes registered twice: {DUPLICATE_ROUTES:?}");
    }

    #[test]
    fn mfa_guardrail_is_read_from_the_policy() {
        let set = mfa_actions();
        assert!(set.contains("terminal.open"));
        assert!(set.contains("secret.read"));
        assert!(!set.contains("view"));

        // Every MFA action name is a real action.
        for a in set {
            assert!(Action::ALL.iter().any(|x| x.as_str() == a), "unknown action {a} in guardrails");
        }
    }

    #[test]
    fn terminal_is_a_websocket_with_mfa() {
        let doc = document("http://x");
        let op = &doc["paths"]["/api/v1/servers/{id}/terminal"]["get"];
        assert_eq!(op["x-forge-transport"], "websocket");
    }

    #[test]
    fn handing_out_access_is_marked_as_needing_mfa() {
        let doc = document("http://x");
        assert_eq!(doc["paths"]["/api/v1/users"]["post"]["x-forge-mfa"], true);
        assert_eq!(doc["paths"]["/api/v1/role-bindings"]["post"]["x-forge-mfa"], true);
        assert_eq!(doc["paths"]["/api/v1/role-bindings/{id}"]["delete"]["x-forge-mfa"], false, "revoking is not");
        assert_eq!(doc["paths"]["/api/v1/users"]["get"]["x-forge-mfa"], false);
    }
}
