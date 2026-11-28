//! Build-time route inventory (GitHub issue #5, phase 0).
//!
//! Reads `src/api/*.rs`, finds every `.route("path", get(h).post(h2)…)`,
//! resolves each handler function and records its doc summary, the first
//! `Action::…` it authorizes, whether it takes a `Caller` (authenticated)
//! and its transport (WebSocket, SSE, byte stream). The server turns this
//! into `/api/openapi.json`, and the CLI's parity checks use it, so a new
//! route is documented automatically and nobody maintains a list by hand.
//! Standard library only (no build dependencies).
//!
//! Typed schemas (D-077): the handler's `Json<T>` body, `Query<T>` and
//! `Json<T>` answer are recorded. When the module itself defines `T` with
//! `#[derive(… JsonSchema …)]`, the generated code asks `schemars` for its
//! schema (`super::<module>::T`); other types stay untyped.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct Route {
    pub method: String,
    pub path: String,
    pub module: String,
    pub handler: String,
    pub summary: String,
    pub action: Option<String>,
    pub authenticated: bool,
    pub transport: &'static str,
    /// Type of the `Json<…>` request body, as written.
    pub request: Option<String>,
    /// Type of the `Query<…>` parameters, as written.
    pub query: Option<String>,
    /// Type inside the `Json<…>` answer, as written.
    pub response: Option<String>,
    /// Rust type expressions with a schema (`super::m::T`, `Vec<super::m::T>`).
    pub request_schema: Option<String>,
    pub query_schema: Option<String>,
    pub response_schema: Option<String>,
}

/// The type between the `<…>` that starts at `open` (balanced).
fn angle(s: &str, open: usize) -> Option<String> {
    let b = s.as_bytes();
    let mut depth = 0i32;

    for (i, &c) in b.iter().enumerate().skip(open) {
        match c {
            b'<' => depth += 1,
            b'>' => {
                depth -= 1;

                if depth == 0 {
                    return Some(s[open + 1..i].split_whitespace().collect::<Vec<_>>().join(" "));
                }
            }
            _ => {}
        }
    }

    None
}

/// The first `{wrapper}<T>` in `s` (`Json`, `Query`), not part of a longer name.
fn wrapped(s: &str, wrapper: &str) -> Option<String> {
    let pat = format!("{wrapper}<");
    let mut from = 0;

    while let Some(rel) = s[from..].find(&pat) {
        let at = from + rel;
        let prev = s[..at].chars().last();
        if prev.is_none_or(|c| !(c.is_alphanumeric() || c == '_')) {
            return angle(s, at + wrapper.len());
        }

        from = at + pat.len();
    }

    None
}

/// Whether `module_src` defines `name` (struct or enum, visible to sibling
/// modules) with `JsonSchema` among its derives.
fn has_schema(module_src: &str, name: &str) -> bool {
    for kw in ["struct", "enum"] {
        for vis in ["pub ", "pub(crate) ", "pub(super) "] {
            let pat = format!("{vis}{kw} {name}");
            let mut from = 0;

            while let Some(rel) = module_src[from..].find(&pat) {
                let at = from + rel;
                let after = module_src[at + pat.len()..].chars().next();
                if after.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                    from = at + pat.len();
                    continue;
                }

                // Attributes and doc lines right above the item, back to the
                // end of the previous item.
                let derives_schema = module_src[..at]
                    .lines()
                    .rev()
                    .take_while(|l| {
                        let t = l.trim();
                        !t.is_empty() && !t.ends_with('}') && !t.ends_with(';')
                    })
                    .any(|l| l.contains("JsonSchema"));

                if derives_schema {
                    return true;
                }

                from = at + pat.len();
            }
        }
    }

    false
}

/// Body types from other crates with a mirror in `api/schema_types.rs`.
const FOREIGN_BODIES: &[&str] = &["NewGrant", "GrantChanges", "RegistrationResponse", "AssertionResponse"];

/// `super::<module>::T` (or `Vec<…>`) when `ty` is a type the module defines
/// with a schema, or `super::schema_types::T` for a mirrored foreign body;
/// `None` otherwise (untyped).
fn schema_expr(module: &str, module_src: &str, ty: &str) -> Option<String> {
    let last = ty.rsplit("::").next().unwrap_or(ty);
    if FOREIGN_BODIES.contains(&last) {
        return Some(format!("super::schema_types::{last}"));
    }

    // A sibling module's type written as `super::<module>::T`: kept as is,
    // so the build fails if that type has no schema.
    if let Some(rest) = ty.strip_prefix("super::")
        && let Some((m, t)) = rest.split_once("::")
        && m.chars().all(|c| c.is_ascii_lowercase() || c == '_')
        && t.chars().all(|c| c.is_alphanumeric() || c == '_')
    {
        return Some(ty.to_string());
    }

    let ident = |t: &str| {
        t.chars().all(|c| c.is_alphanumeric() || c == '_') && t.chars().next().is_some_and(char::is_uppercase)
    };

    if ident(ty) && has_schema(module_src, ty) {
        return Some(format!("super::{module}::{ty}"));
    }

    let inner = ty.strip_prefix("Vec<").and_then(|t| t.strip_suffix('>'))?;

    (ident(inner) && has_schema(module_src, inner)).then(|| format!("Vec<super::{module}::{inner}>"))
}

/// Content between the parenthesis at `open` and its match.
fn balanced(s: &str, open: usize) -> Option<(usize, usize)> {
    let b = s.as_bytes();
    if b.get(open) != Some(&b'(') {
        return None;
    }

    let mut depth = 0i32;
    let mut in_str = false;
    let mut i = open;

    while i < b.len() {
        let c = b[i];

        if in_str {
            if c == b'\\' {
                i += 2;
                continue;
            }

            if c == b'"' {
                in_str = false;
            }
        } else {
            match c {
                b'"' => in_str = true,
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;

                    if depth == 0 {
                        return Some((open + 1, i));
                    }
                }
                _ => {}
            }
        }

        i += 1;
    }

    None
}

/// `(method, handler)` pairs from `get(a::b).post(c)` (also `axum::routing::put(x)`).
fn methods(expr: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();

    for m in ["get", "post", "put", "patch", "delete"] {
        let pat = format!("{m}(");
        let mut from = 0;

        while let Some(rel) = expr[from..].find(&pat) {
            let at = from + rel;
            let prev = expr[..at].chars().last();
            let word_start = prev.is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
            if word_start && let Some((a, b)) = balanced(expr, at + m.len()) {
                let h = expr[a..b].trim().to_owned();
                if !h.is_empty() && h.chars().all(|c| c.is_alphanumeric() || c == '_' || c == ':') {
                    out.push((m.to_uppercase(), h));
                }
            }

            from = at + pat.len();
        }
    }

    out
}

/// `(path, handler-expression, byte offset)` for each `.route(` in `src`.
fn routes_in(src: &str) -> Vec<(String, String, usize)> {
    let mut out = Vec::new();
    let mut from = 0;

    while let Some(rel) = src[from..].find(".route(") {
        let at = from + rel;
        let open = at + ".route".len();
        if let Some((a, b)) = balanced(src, open) {
            let inner = &src[a..b];
            if let Some(q1) = inner.find('"')
                && let Some(q2) = inner[q1 + 1..].find('"')
            {
                let path = inner[q1 + 1..q1 + 1 + q2].to_owned();
                let rest = inner[q1 + 2 + q2..].to_owned();
                out.push((path, rest, at));
            }

            from = b;
        } else {
            from = open;
        }
    }

    out
}

#[derive(Debug, Default, Clone)]
struct HandlerInfo {
    summary: String,
    action: Option<String>,
    authenticated: bool,
    transport: &'static str,
    request: Option<String>,
    query: Option<String>,
    response: Option<String>,
}

fn first_action(body: &str) -> Option<String> {
    body.find("Action::")
        .map(|i| body[i + 8..].chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect::<String>())
        .filter(|a| a.chars().next().is_some_and(char::is_uppercase))
}

/// Body text of a free function `name` defined in `src`.
fn fn_body<'a>(src: &'a str, name: &str) -> Option<&'a str> {
    let pats = [
        format!("pub async fn {name}("),
        format!("pub(crate) async fn {name}("),
        format!("async fn {name}("),
        format!("pub fn {name}("),
        format!("pub(crate) fn {name}("),
        format!("fn {name}("),
    ];
    let start = pats.iter().filter_map(|p| src.find(p.as_str())).min()?;
    let tail = &src[start + 3..];
    let end = ["\npub async fn ", "\nasync fn ", "\npub fn ", "\nfn ", "\npub(crate) async fn ", "\npub(crate) fn "]
        .iter()
        .filter_map(|p| tail.find(p))
        .min()
        .unwrap_or(tail.len());

    Some(&src[start..start + 3 + end])
}

/// Free-function calls `name(` in `body` (not methods `.x(`, paths `a::x(` or macros `x!(`).
fn called_fns(body: &str) -> Vec<String> {
    let b = body.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;

    while i < b.len() {
        if b[i].is_ascii_lowercase()
            && (i == 0 || !(b[i - 1].is_ascii_alphanumeric() || matches!(b[i - 1], b'_' | b'.' | b':')))
        {
            let start = i;

            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }

            if b.get(i) == Some(&b'(') {
                let name = &body[start..i];
                if !matches!(name, "if" | "match" | "while" | "for" | "loop" | "return" | "move" | "async" | "await")
                    && !out.iter().any(|n| n == name)
                {
                    out.push(name.to_owned());
                }
            }
        } else {
            i += 1;
        }
    }

    out
}

/// `list_keys` → `List keys`.
fn humanise(handler: &str) -> String {
    let words = handler.replace('_', " ");
    let mut c = words.chars();

    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

/// Finds `async fn name(` in `src` and extracts its metadata.
fn handler_info(src: &str, name: &str) -> Option<HandlerInfo> {
    let mut start = None;

    for pat in
        [format!("pub async fn {name}("), format!("async fn {name}("), format!("pub fn {name}("), format!("fn {name}(")]
    {
        if let Some(i) = src.find(&pat) {
            start = Some(i);
            break;
        }
    }

    let start = start?;

    // Doc comment: the `///` lines right above (skipping attributes).
    let mut doc: Vec<&str> = Vec::new();

    for line in src[..start].lines().rev().skip_while(|l| l.trim().is_empty()) {
        let t = line.trim();
        if let Some(d) = t.strip_prefix("///") {
            doc.push(d.trim());
        } else if t.starts_with("#[") {
            continue;
        } else {
            break;
        }
    }

    doc.reverse();
    let doc = doc.join(" ");
    let mut summary = doc.split(". ").next().unwrap_or_default().trim_end_matches('.').trim().to_owned();

    // `GET /x`-style doc lines describe the route, not what it does.
    if let Some((_, rest)) = summary.split_once("`: ") {
        summary = rest.trim().to_owned();
    }

    if summary.is_empty() || summary.starts_with('`') {
        summary = humanise(name);
    }

    // Body: up to the next top-level fn.
    let tail = &src[start + 3..];
    let end = ["\npub async fn ", "\nasync fn ", "\npub fn ", "\nfn ", "\npub(crate) async fn ", "\npub(crate) fn "]
        .iter()
        .filter_map(|p| tail.find(p))
        .min()
        .unwrap_or(tail.len());
    let body = &src[start..start + 3 + end];
    let sig_end = body.find('{').unwrap_or(body.len());
    let sig = &body[..sig_end];

    // The first `Action::…` in the handler, else in a same-module helper it
    // calls (one level: `authorized(&state, …)`, `require_admin(…)`).
    let action = first_action(body).or_else(|| {
        called_fns(&body[sig_end.min(body.len())..])
            .into_iter()
            .filter(|f| f != name)
            .find_map(|f| fn_body(src, &f).and_then(first_action))
    });

    let transport = if sig.contains("WebSocketUpgrade") {
        "websocket"
    } else if body.contains("Sse<") || body.contains("Sse::new") {
        "sse"
    } else if body.contains("Body::from_stream") {
        "stream"
    } else {
        "json"
    };

    let (params, ret) = sig.split_once("->").unwrap_or((sig, ""));

    Some(HandlerInfo {
        summary,
        action: action.filter(|a| !a.is_empty()),
        authenticated: sig.contains("Caller"),
        transport,
        request: wrapped(params, "Json"),
        query: wrapped(params, "Query"),
        response: wrapped(ret, "Json"),
    })
}

/// Scans `api_dir` and returns every route, sorted by path and method.
pub fn scan(api_dir: &Path) -> Vec<Route> {
    let mut sources: BTreeMap<String, String> = BTreeMap::new();

    if let Ok(rd) = std::fs::read_dir(api_dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "rs")
                && let (Some(stem), Ok(src)) = (p.file_stem().and_then(|s| s.to_str()), std::fs::read_to_string(&p))
            {
                sources.insert(stem.to_owned(), src);
            }
        }
    }

    let mut out = Vec::new();

    for (file, src) in &sources {
        // In mod.rs, routes before the root router are nested under /api/v1;
        // `routes()` routers in other modules are merged into /api/v1.
        let root_at = if file == "mod" { src.find("let mut app = Router::new()") } else { None };

        for (path, expr, at) in routes_in(src) {
            let full = if root_at.is_some_and(|r| at > r) { path.clone() } else { format!("/api/v1{path}") };

            for (method, handler) in methods(&expr) {
                let (module, name) = match handler.rsplit_once("::") {
                    Some((m, n)) => (m.rsplit("::").next().unwrap_or(m).to_owned(), n.to_owned()),
                    None => (file.clone(), handler.clone()),
                };

                let module_src = sources.get(&module).map(String::as_str).unwrap_or_default();
                let info = handler_info(module_src, &name).unwrap_or_default();
                let schema = |t: &Option<String>| t.as_deref().and_then(|t| schema_expr(&module, module_src, t));
                let (request_schema, query_schema, response_schema) =
                    (schema(&info.request), schema(&info.query), schema(&info.response));
                out.push(Route {
                    method,
                    path: full.clone(),
                    module: module.clone(),
                    handler: name,
                    summary: info.summary,
                    action: info.action,
                    authenticated: info.authenticated,
                    transport: if info.transport.is_empty() { "json" } else { info.transport },
                    request: info.request,
                    query: info.query,
                    response: info.response,
                    request_schema,
                    query_schema,
                    response_schema,
                });
            }
        }
    }

    out.sort_by(|a, b| (&a.path, &a.method).cmp(&(&b.path, &b.method)));

    out
}

/// `METHOD path (handler, handler)` for routes registered more than once:
/// axum panics on these when the router is built.
pub fn duplicates(routes: &[Route]) -> Vec<String> {
    let mut out = Vec::new();

    for w in routes.windows(2) {
        if w[0].path == w[1].path && w[0].method == w[1].method {
            out.push(format!(
                "{} {} ({}::{}, {}::{})",
                w[0].method, w[0].path, w[0].module, w[0].handler, w[1].module, w[1].handler
            ));
        }
    }

    out
}

/// Rust source for `OUT_DIR/routes.rs`.
pub fn render(routes: &[Route]) -> String {
    let mut code = String::from(
        "/// Every API route, scanned from `src/api` at build time (`build_routes.rs`).\n\
         pub static ROUTES: &[RouteInfo] = &[\n",
    );

    for r in routes {
        let _ = writeln!(
            code,
            "    RouteInfo {{ method: {:?}, path: {:?}, module: {:?}, handler: {:?}, summary: {:?}, action: {:?}, authenticated: {}, transport: {:?}, request: {:?}, query: {:?}, response: {:?} }},",
            r.method,
            r.path,
            r.module,
            r.handler,
            r.summary,
            r.action.as_deref(),
            r.authenticated,
            r.transport,
            r.request.as_deref(),
            r.query.as_deref(),
            r.response.as_deref(),
        );
    }

    code.push_str("];\n");

    // Schemas by route index: [request, query, response].
    code.push_str(
        "/// Schemas of route `i` in [`ROUTES`]: `[request, query, response]`.\n\
         #[allow(clippy::match_single_binding)]\n\
         pub(crate) fn route_schemas(i: usize, g: &mut schemars::SchemaGenerator) -> [Option<schemars::Schema>; 3] {\n\
         \x20   let _ = &g;\n\
         \x20   match i {\n",
    );

    for (i, r) in routes.iter().enumerate() {
        if r.request_schema.is_none() && r.query_schema.is_none() && r.response_schema.is_none() {
            continue;
        }

        let part =
            |s: &Option<String>| s.as_ref().map_or("None".to_owned(), |t| format!("Some(g.subschema_for::<{t}>())"));
        let _ = writeln!(
            code,
            "        {i} => [{}, {}, {}],",
            part(&r.request_schema),
            part(&r.query_schema),
            part(&r.response_schema)
        );
    }

    code.push_str("        _ => [None, None, None],\n    }\n}\n");
    let _ = writeln!(
        code,
        "/// Routes registered twice (must be empty; checked by a test).\n#[cfg_attr(not(test), allow(dead_code))]\npub static DUPLICATE_ROUTES: &[&str] = &{:?};",
        duplicates(routes)
    );

    code
}
