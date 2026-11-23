//! Query-filter parity (E-03): every query parameter of an API operation
//! that has a typed `forge` command is a flag on that command.
//!
//! The server's build-time route scanner (`forge-server/build_routes.rs`)
//! finds each handler's `Query<T>` type; this test reads `T`'s fields from
//! the same source file (honouring `#[serde(rename)]`, `flatten` and
//! `skip_deserializing`). A parameter `server_id` is matched by
//! `--server-id`, or by the flag that `parity.json` → `query` maps it to
//! (`"server_id": "--node"`). A mapping that does not start with `--` is the
//! reason the parameter has no flag (the command fills it in itself).

#[allow(dead_code, clippy::all, clippy::pedantic, clippy::nursery)]
#[path = "../../forge-server/build_routes.rs"]
mod build_routes;

use std::collections::BTreeMap;
use std::path::PathBuf;

use clap::CommandFactory;
use serde_json::Value;

fn api_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../forge-server/src/api")
}

/// The query parameter names of `ty`, defined in `src`.
fn fields(src: &str, ty: &str, depth: u8) -> Vec<String> {
    let ty = ty.rsplit("::").next().unwrap_or(ty).trim();
    let Some(start) = src.find(&format!("struct {ty} {{")) else { return Vec::new() };
    let body = &src[start..];
    let body = &body[body.find('{').map_or(0, |i| i + 1)..];
    let body = &body[..body.find("\n}").unwrap_or(body.len())];
    let mut out = Vec::new();
    let mut attrs = String::new();
    for line in body.lines() {
        let t = line.trim();
        if t.starts_with("//") || t.is_empty() {
            continue;
        }
        if t.starts_with("#[") {
            attrs.push_str(t);
            continue;
        }
        let t = t.strip_prefix("pub ").unwrap_or(t);
        if let Some((name, rest)) = t.split_once(':') {
            let name = name.trim().trim_start_matches("r#");
            if attrs.contains("skip_deserializing") || attrs.contains("serde(skip)") {
            } else if attrs.contains("flatten") {
                if depth < 3 {
                    let inner = rest.trim().trim_end_matches(',');
                    out.extend(fields(src, inner, depth + 1));
                }
            } else if let Some(r) = attrs.split("rename = \"").nth(1).and_then(|r| r.split('"').next()) {
                out.push(r.to_owned());
            } else {
                out.push(name.to_owned());
            }
        }
        attrs.clear();
    }
    out
}

/// Long flags of the command named by `how` (`forge a b …`).
fn flags_of(how: &str) -> Option<Vec<String>> {
    let cli = crate::Cli::command();
    let first = how.split(" / ").next().unwrap_or_default();
    let mut cmd = &cli;
    for w in first.split_whitespace().skip(1).take_while(|w| !w.starts_with(['-', '<', '(', '['])) {
        let head = w.split('|').next().unwrap_or_default();
        cmd = cmd.find_subcommand(head)?;
    }
    let mut out: Vec<String> = cmd.get_arguments().filter_map(|a| a.get_long().map(|l| format!("--{l}"))).collect();
    out.extend(cli.get_arguments().filter_map(|a| a.get_long().map(|l| format!("--{l}"))));
    Some(out)
}

/// `"METHOD /path"` → `[(param, …)]` for every route with a query type.
fn query_params() -> BTreeMap<String, Vec<String>> {
    let dir = api_dir();
    let mut out = BTreeMap::new();
    for r in build_routes::scan(&dir) {
        let Some(q) = r.query.as_deref() else { continue };
        let src = std::fs::read_to_string(dir.join(format!("{}.rs", r.module))).unwrap_or_default();
        out.insert(format!("{} {}", r.method, r.path), fields(&src, q, 0));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_query_struct_fields() {
        let src = "pub struct Q {\n    #[serde(default)]\n    pub server_id: Option<X>,\n    #[serde(skip_deserializing)]\n    pub hidden: u8,\n    #[serde(rename = \"q\")]\n    pub query: String,\n}\n";
        assert_eq!(fields(src, "Q", 0), ["server_id", "q"]);
    }

    #[test]
    fn every_query_parameter_has_a_flag() {
        let params = query_params();
        assert!(params.len() > 20, "the route scan found only {} query types", params.len());
        let doc: Value = serde_json::from_str(crate::parity::PARITY_JSON).unwrap_or(Value::Null);
        let mut problems = Vec::new();
        for (op, names) in &params {
            let Some(how) = doc["command"][op].as_str() else { continue };
            let map = &doc["query"][op];
            let how = map["_command"].as_str().unwrap_or(how);
            let Some(flags) = flags_of(how) else {
                problems.push(format!("{op}: `{how}` is not a command"));
                continue;
            };
            for p in names {
                match map[p.as_str()].as_str() {
                    Some(f) if f.starts_with("--") => {
                        if !flags.iter().any(|x| x == f) {
                            problems.push(format!("{op} ?{p}: `{how}` has no `{f}`"));
                        }
                    }
                    Some(reason) if !reason.trim().is_empty() => {}
                    _ => {
                        let want = format!("--{}", p.replace('_', "-"));
                        if !flags.contains(&want) {
                            problems
                                .push(format!("{op} ?{p}: add `{want}` to `{how}`, or map it in parity.json → query"));
                        }
                    }
                }
            }
        }
        // Mappings for parameters that no longer exist are stale.
        for (op, map) in doc["query"].as_object().into_iter().flatten() {
            for k in map.as_object().into_iter().flatten().map(|(k, _)| k).filter(|k| *k != "_command") {
                if !params.get(op).is_some_and(|n| n.contains(k)) {
                    problems.push(format!("parity.json → query → {op}: `{k}` is not a query parameter"));
                }
            }
        }
        assert!(problems.is_empty(), "{} gaps:\n{}", problems.len(), problems.join("\n"));
    }
}
