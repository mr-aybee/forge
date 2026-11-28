//! `forge parity`: how the CLI covers the server's API (issue #5).
//!
//! `parity.json` (next to `Cargo.toml`) classifies every operation of
//! `/api/openapi.json` as a typed command, reachable only through
//! `forge api call` (with the reason), or excluded (with the reason). A
//! forge-server test fails when a route is missing from it, so the list
//! cannot drift; this command compares it with the live server.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::error::{CliError, Result};
use crate::out::{self, Col};
use crate::{Global, connect, target};

pub const PARITY_JSON: &str = include_str!("../parity.json");

/// `"METHOD /path"` → `(class, command or reason)`.
pub fn table() -> Result<BTreeMap<String, (&'static str, String)>> {
    let doc: Value = serde_json::from_str(PARITY_JSON)
        .map_err(|e| CliError::new(crate::error::Exit::General, format!("parity.json: {e}")))?;
    let mut out = BTreeMap::new();

    for class in ["command", "api", "excluded"] {
        for (k, v) in doc[class].as_object().into_iter().flatten() {
            out.insert(k.clone(), (class, v.as_str().unwrap_or_default().to_owned()));
        }
    }

    Ok(out)
}

pub async fn run(g: &Global, gaps: bool) -> Result<()> {
    let t = target(g)?;
    let mut api = connect(g, &t).await?;
    let spec = api.get("/api/openapi.json").await?;
    let known = table()?;
    let mut rows = Vec::new();
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();

    for (path, item) in spec["paths"].as_object().into_iter().flatten() {
        for (method, op) in item.as_object().into_iter().flatten() {
            let key = format!("{} {path}", method.to_uppercase());
            let (class, how) = known.get(&key).map_or(("unclassified", String::new()), |(c, h)| (*c, h.clone()));
            *counts.entry(class).or_default() += 1;

            if gaps && class == "command" {
                continue;
            }

            rows.push(json!({
                "operation": key,
                "id": op["operationId"],
                "class": class,
                "cli": how,
                "action": op["x-forge-action"],
            }));
        }
    }

    let total: usize = counts.values().sum();

    if g.output == out::Format::Table {
        let cols: &[Col] = &[("OPERATION", "/operation"), ("CLASS", "/class"), ("CLI / REASON", "/cli")];
        out::result(g.output, &Value::Array(rows), Some(cols));
        eprintln!(
            "{total} operations: {} typed commands, {} through `forge api call`, {} excluded, {} unclassified (server newer than this CLI)",
            counts.get("command").unwrap_or(&0),
            counts.get("api").unwrap_or(&0),
            counts.get("excluded").unwrap_or(&0),
            counts.get("unclassified").unwrap_or(&0),
        );

        return Ok(());
    }

    out::result(g.output, &json!({ "total": total, "counts": counts, "operations": rows }), None);

    Ok(())
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    /// Every typed entry names commands that exist in the command tree.
    #[test]
    fn typed_entries_name_real_commands() {
        let cli = crate::Cli::command();
        let t = table().unwrap_or_default();
        assert!(t.len() > 200, "parity.json looks truncated");

        for (op, (class, how)) in &t {
            assert!(!how.trim().is_empty(), "{op}: empty {class} entry");

            if *class != "command" {
                continue;
            }

            // "forge a b / c" → check the first alternative's words.
            let first = how.split(" / ").next().unwrap_or_default();
            let words = first.split_whitespace().skip(1).take_while(|w| !w.starts_with(['-', '<', '(', '[']));
            let mut cmd = &cli;

            for w in words {
                // `a|b|c` lists sibling commands; each must exist.
                let mut alts = w.split('|');
                let head = alts.next().unwrap_or_default();

                for alt in alts {
                    assert!(cmd.find_subcommand(alt).is_some(), "{op}: `{first}`: no command `{alt}`");
                }

                cmd = cmd.find_subcommand(head).unwrap_or_else(|| panic!("{op}: `{first}`: no command `{head}`"));
            }

            assert_ne!(cmd.get_name(), "forge", "{op}: `{first}` names no command");
        }
    }
}
