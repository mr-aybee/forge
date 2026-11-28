//! `forge api`: every API operation from the server's OpenAPI document
//! (`/api/openapi.json`), for scripts and LLM agents. Typed commands come
//! first; `forge api call` reaches what they do not cover yet. The server
//! still authorizes every call.

use std::io::Write as _;
use std::path::PathBuf;

use clap::Subcommand;
use reqwest::Method;
use serde_json::{Value, json};

use crate::error::{CliError, Exit, Result};
use crate::out;
use crate::{Global, connect, target};

#[derive(Subcommand, Debug)]
pub enum ApiCmd {
    /// List operations (filter by tag or text).
    Operations {
        /// Only this tag (API module), e.g. `servers`.
        #[arg(long)]
        tag: Option<String>,
        /// Text to match in the id, path or summary.
        #[arg(long)]
        search: Option<String>,
    },
    /// Show one operation: parameters, permission, MFA, transport.
    Describe { operation: String },
    /// Call an operation by id (`servers.list`) with path params and a JSON body.
    Call {
        operation: String,
        /// Path parameter, e.g. `-p id=0193…` (repeatable).
        #[arg(long = "param", short = 'p', value_parser = kv)]
        params: Vec<(String, String)>,
        /// Query parameter, e.g. `-q limit=20` (repeatable).
        #[arg(long = "query", short = 'q', value_parser = kv)]
        query: Vec<(String, String)>,
        /// JSON body, or `@file.json`, or `@-` for stdin.
        #[arg(long)]
        body: Option<String>,
        /// Write a byte-stream response (downloads) to this file.
        #[arg(long)]
        out_file: Option<PathBuf>,
        /// Idempotency-Key for a mutation (default: a random one per call).
        /// Reuse it to retry safely after a lost answer.
        #[arg(long)]
        idempotency_key: Option<String>,
        /// Send the body without checking it against the operation's schema.
        #[arg(long)]
        no_validate: bool,
        /// Confirm a mutating call (required with --non-interactive).
        #[arg(long)]
        yes: bool,
    },
    /// Raw request: `forge api request GET /api/v1/servers`.
    Request {
        method: String,
        path: String,
        #[arg(long)]
        body: Option<String>,
        /// Idempotency-Key for a mutation (default: a random one per call).
        #[arg(long)]
        idempotency_key: Option<String>,
        /// Send the body without checking it against the operation's schema.
        #[arg(long)]
        no_validate: bool,
        #[arg(long)]
        yes: bool,
    },
}

fn kv(s: &str) -> std::result::Result<(String, String), String> {
    s.split_once('=').map(|(k, v)| (k.to_owned(), v.to_owned())).ok_or_else(|| format!("expected key=value, got `{s}`"))
}

fn enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

fn read_body(spec: Option<&str>) -> Result<Option<Value>> {
    let Some(spec) = spec else { return Ok(None) };
    let text = match spec {
        "@-" => std::io::read_to_string(std::io::stdin())?,
        s if s.starts_with('@') => {
            std::fs::read_to_string(&s[1..]).map_err(|e| CliError::usage(format!("{}: {e}", &s[1..])))?
        }
        s => s.to_owned(),
    };

    serde_json::from_str(&text).map(Some).map_err(|e| CliError::usage(format!("--body is not valid JSON: {e}")))
}

/// `(method, path, operation)` for every operation in the document.
fn operations(doc: &Value) -> Vec<(String, String, Value)> {
    let mut out = Vec::new();

    for (path, item) in doc["paths"].as_object().into_iter().flatten() {
        for (method, op) in item.as_object().into_iter().flatten() {
            out.push((method.to_uppercase(), path.clone(), op.clone()));
        }
    }

    out
}

fn find<'a>(ops: &'a [(String, String, Value)], id: &str) -> Result<&'a (String, String, Value)> {
    ops.iter().find(|(_, _, op)| op["operationId"] == id).ok_or_else(|| {
        let near: Vec<&str> = ops
            .iter()
            .filter_map(|(_, _, op)| op["operationId"].as_str())
            .filter(|o| o.split('.').next() == id.split('.').next())
            .take(8)
            .collect();
        CliError::new(
            Exit::NotFound,
            format!(
                "no operation `{id}`{}",
                if near.is_empty() { String::new() } else { format!(" (did you mean: {})", near.join(", ")) }
            ),
        )
    })
}

/// The component schemas `value` refers to, followed transitively.
fn referenced_schemas(doc: &Value, value: &Value) -> serde_json::Map<String, Value> {
    fn refs(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::Object(m) => {
                if let Some(r) =
                    m.get("$ref").and_then(Value::as_str).and_then(|r| r.strip_prefix("#/components/schemas/"))
                {
                    out.push(r.to_owned());
                }

                m.values().for_each(|x| refs(x, out));
            }
            Value::Array(a) => a.iter().for_each(|x| refs(x, out)),
            _ => {}
        }
    }

    let mut found = serde_json::Map::new();
    let mut todo = Vec::new();
    refs(value, &mut todo);

    while let Some(name) = todo.pop() {
        if found.contains_key(&name) || name == "Problem" {
            continue;
        }

        if let Some(s) = doc["components"]["schemas"].get(&name) {
            refs(s, &mut todo);
            found.insert(name, s.clone());
        }
    }

    found
}

fn confirm_mutation(g: &Global, method: &str, what: &str, yes: bool) -> Result<()> {
    if method == "GET" || yes {
        return Ok(());
    }

    if g.non_interactive || !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        return Err(CliError::usage(format!("{method} {what} changes state: pass --yes to confirm")));
    }

    eprint!("{method} {what} changes state. Type `yes` to continue: ");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;

    if line.trim() == "yes" { Ok(()) } else { Err(CliError::new(Exit::General, "cancelled").with_code("cancelled")) }
}

pub async fn run(g: &Global, cmd: ApiCmd) -> Result<()> {
    let t = target(g)?;
    let mut api = connect(g, &t).await?;

    match cmd {
        ApiCmd::Request { method, path, body, idempotency_key, no_validate, yes } => {
            api.set_idempotency_key(idempotency_key);
            let m = Method::from_bytes(method.to_uppercase().as_bytes())
                .map_err(|_| CliError::usage(format!("bad method {method}")))?;

            if !path.starts_with('/') {
                return Err(CliError::usage("the path must start with / (e.g. /api/v1/servers)"));
            }

            confirm_mutation(g, m.as_str(), &path, yes)?;
            let body = read_body(body.as_deref())?;
            if let (Some(b), false) = (&body, no_validate)
                && let Ok(doc) = api.get("/api/openapi.json").await
            {
                crate::schemacheck::validate(&doc, m.as_str(), &path, b)?;
            }

            let v = api.call(m, &path, body.as_ref()).await?;
            out::result(g.output, &v, None);
            Ok(())
        }
        cmd => {
            let doc = api.get("/api/openapi.json").await.map_err(|e| {
                if e.exit == Exit::NotFound {
                    CliError::new(Exit::NotFound, "this server does not publish /api/openapi.json (update the server)")
                } else {
                    e
                }
            })?;
            let ops = operations(&doc);

            match cmd {
                ApiCmd::Operations { tag, search } => {
                    let s = search.map(|s| s.to_lowercase());
                    let rows: Vec<Value> = ops
                        .iter()
                        .filter(|(_, _, op)| tag.as_deref().is_none_or(|t| op["tags"][0] == t))
                        .filter(|(m, p, op)| {
                            s.as_deref().is_none_or(|s| {
                                format!("{m} {p} {} {}", op["operationId"], op["summary"]).to_lowercase().contains(s)
                            })
                        })
                        .map(|(m, p, op)| {
                            json!({
                                "id": op["operationId"], "method": m, "path": p, "summary": op["summary"],
                                "action": op["x-forge-action"], "mfa": op["x-forge-mfa"], "transport": op["x-forge-transport"],
                            })
                        })
                        .collect();
                    out::result(
                        g.output,
                        &Value::Array(rows),
                        Some(&[
                            ("ID", "/id"),
                            ("METHOD", "/method"),
                            ("PATH", "/path"),
                            ("MFA", "/mfa"),
                            ("SUMMARY", "/summary"),
                        ]),
                    );
                    Ok(())
                }
                ApiCmd::Describe { operation } => {
                    let (m, p, op) = find(&ops, &operation)?;
                    let mut v = op.clone();
                    v["method"] = json!(m);
                    v["path"] = json!(p);
                    let schemas = referenced_schemas(&doc, op);
                    if !schemas.is_empty() {
                        v["components"] = json!({ "schemas": schemas });
                    }

                    // A schema reads best as YAML for people.
                    if g.output.is_machine() {
                        out::result(g.output, &v, None);
                    } else {
                        out::emit(&out::to_yaml(&v));
                    }

                    Ok(())
                }
                ApiCmd::Call { operation, params, query, body, out_file, idempotency_key, no_validate, yes } => {
                    api.set_idempotency_key(idempotency_key);
                    let (m, p, op) = find(&ops, &operation)?;
                    let transport = op["x-forge-transport"].as_str().unwrap_or("json");
                    if transport == "websocket" || transport == "sse" {
                        return Err(CliError::usage(format!(
                            "{operation} is a {transport} stream; use its dedicated command"
                        )));
                    }

                    let mut path = p.clone();

                    for (k, v) in &params {
                        path = path.replace(&format!("{{{k}}}"), &enc(v));
                    }

                    if let Some(missing) = path.split('/').find(|s| s.starts_with('{')) {
                        return Err(CliError::usage(format!(
                            "missing path parameter {missing}: add -p {}=…",
                            missing.trim_matches(['{', '}'])
                        )));
                    }

                    if !query.is_empty() {
                        let q: Vec<String> = query.iter().map(|(k, v)| format!("{}={}", enc(k), enc(v))).collect();
                        path = format!("{path}?{}", q.join("&"));
                    }

                    confirm_mutation(g, m, &format!("{operation} ({path})"), yes)?;
                    let body = read_body(body.as_deref())?;
                    if let (Some(b), false) = (&body, no_validate) {
                        crate::schemacheck::validate(&doc, m, p, b)?;
                    }

                    let method = Method::from_bytes(m.as_bytes()).map_err(|_| CliError::usage("bad method"))?;

                    if transport == "stream" {
                        let file = out_file.ok_or_else(|| {
                            CliError::usage(format!("{operation} returns bytes: add --out-file <path>"))
                        })?;

                        if file.exists() {
                            return Err(CliError::new(
                                Exit::Conflict,
                                format!("{} exists; refusing to overwrite", file.display()),
                            ));
                        }

                        let mut resp = api.send(method, &path, body.as_ref()).await?;
                        let mut f = std::fs::File::create_new(&file)?;
                        let mut written = 0u64;

                        while let Some(chunk) =
                            resp.chunk().await.map_err(|e| CliError::new(Exit::Transport, e.to_string()))?
                        {
                            f.write_all(&chunk)?;
                            written += chunk.len() as u64;
                        }

                        out::result(g.output, &json!({ "file": file, "bytes": written }), None);

                        return Ok(());
                    }

                    let v = api.call(method, &path, body.as_ref()).await?;
                    out::result(g.output, &v, None);
                    Ok(())
                }
                ApiCmd::Request { .. } => Ok(()),
            }
        }
    }
}
