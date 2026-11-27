//! The CLI reference stays in sync with the binary (issue #5): every
//! command has a `### forge …` section on its page under
//! `docs/src/content/docs/reference/cli/`, and that section names each of
//! the command's own flags and arguments. Global flags are documented once
//! on the index page.

use std::collections::BTreeMap;
use std::path::PathBuf;

use clap::CommandFactory;

/// The docs page (file stem) for a command path.
#[must_use]
pub fn page_of(path: &[&str]) -> &'static str {
    match path {
        ["nodes", "files", ..] => "nodes-files",
        ["tenants" | "environments", ..] => "tenants-environments",
        ["sites" | "certificates", ..] => "sites-certificates",
        ["version" | "capabilities" | "completion" | "commands", ..] => "index",
        [g, ..] => match *g {
            "contexts" => "contexts",
            "auth" => "auth",
            "nodes" => "nodes",
            "agents" => "agents",
            "jobs" => "jobs",
            "audit" => "audit",
            "plugins" => "plugins",
            "stacks" => "stacks",
            "swarm" => "swarm",
            "projects" => "projects",
            "services" => "services",
            "approvals" => "approvals",
            "users" => "users",
            "access" => "access",
            "api-keys" => "api-keys",
            "service-accounts" => "service-accounts",
            "connectors" => "connectors",
            "settings" => "settings",
            "clusters" => "clusters",
            "dns" => "dns",
            "alerts" => "alerts",
            "tunnel" => "tunnel",
            "docker" => "docker",
            "registries" => "registries",
            "webhooks" => "webhooks",
            "deploy" => "deploy",
            "vault" => "vault",
            "system" => "system",
            "packages" => "packages",
            "sessions" => "sessions",
            "update" => "update",
            "parity" => "parity",
            "api" => "api",
            _ => "unknown",
        },
        [] => "index",
    }
}

/// `(path, own flags and arguments)` for every runnable command.
fn leaves() -> Vec<(Vec<String>, Vec<String>)> {
    fn walk(c: &clap::Command, path: &mut Vec<String>, out: &mut Vec<(Vec<String>, Vec<String>)>) {
        let subs: Vec<&clap::Command> =
            c.get_subcommands().filter(|s| !s.is_hide_set() && s.get_name() != "help").collect();

        if subs.is_empty() && !path.is_empty() {
            let args = c
                .get_arguments()
                .filter(|a| !a.is_hide_set() && !a.is_global_set())
                .filter(|a| !matches!(a.get_id().as_str(), "help" | "version"))
                .map(|a| match a.get_long() {
                    Some(l) => format!("--{l}"),
                    None => format!("<{}>", a.get_id().as_str().replace('_', "-")),
                })
                .collect();
            out.push((path.clone(), args));
        }

        for s in subs {
            path.push(s.get_name().to_owned());
            walk(s, path, out);
            path.pop();
        }
    }

    let mut out = Vec::new();
    walk(&crate::Cli::command(), &mut Vec::new(), &mut out);

    out
}

fn docs_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../docs/src/content/docs/reference/cli")
}

/// The text of `### forge <path>` up to the next `##`/`###` heading.
fn section<'a>(page: &'a str, path: &str) -> Option<&'a str> {
    let head = format!("### forge {path}\n");
    let start = page.find(&head)? + head.len();
    let rest = &page[start..];
    let end = rest.find("\n### ").or_else(|| rest.find("\n## ")).unwrap_or(rest.len());

    Some(&rest[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_command_and_flag_is_documented() {
        let dir = docs_dir();
        let mut pages: BTreeMap<String, String> = BTreeMap::new();
        let mut problems = Vec::new();

        for (path, args) in leaves() {
            let refs: Vec<&str> = path.iter().map(String::as_str).collect();
            let page = page_of(&refs);
            let text = pages
                .entry(page.to_owned())
                .or_insert_with(|| std::fs::read_to_string(dir.join(format!("{page}.md"))).unwrap_or_default());
            let joined = path.join(" ");
            match section(text, &joined) {
                None => problems.push(format!("reference/cli/{page}.md: no `### forge {joined}` section")),
                Some(s) => {
                    for a in args {
                        // A flag as `--x`, an argument as `<x>` (either spelling).
                        let alt = a.replace('-', "_");
                        if !s.contains(&a) && !s.contains(&alt) {
                            problems.push(format!("reference/cli/{page}.md `forge {joined}`: `{a}` is not documented"));
                        }
                    }
                }
            }
        }

        assert!(problems.is_empty(), "{} gaps:\n{}", problems.len(), problems.join("\n"));
    }

    #[test]
    fn every_command_has_a_page() {
        for (path, _) in leaves() {
            let refs: Vec<&str> = path.iter().map(String::as_str).collect();
            assert_ne!(page_of(&refs), "unknown", "add `forge {}` to page_of()", path.join(" "));
        }
    }
}
