//! `forge alerts templates` and `forge alerts create-rule --template`:
//! ready-made alert rules (D-118). The server owns the catalogue; this
//! module lists it and turns one template plus its targets into a rule.

use clap::{Args, ValueEnum};
use serde_json::{Value, json};

use crate::api::Api;
use crate::cmds::{looks_like_id, resolve_node};
use crate::error::{CliError, Exit, Result};

/// Where a template is offered.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    Node,
    Service,
    Database,
    Certificate,
    Any,
}

impl Place {
    pub const fn as_str(self) -> &'static str {
        match self {
            Place::Node => "node",
            Place::Service => "service",
            Place::Database => "database",
            Place::Certificate => "certificate",
            Place::Any => "any",
        }
    }
}

/// An alarm's severity.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Info,
    Warning,
    Major,
    Critical,
}

impl Severity {
    pub const fn as_str(self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Warning => "warning",
            Severity::Major => "major",
            Severity::Critical => "critical",
        }
    }
}

/// The flags of `create-rule --template`.
#[derive(Args, Debug, Clone, Default, PartialEq, Eq)]
pub struct TemplateFlags {
    /// Create the rule from this template (see `forge alerts templates`).
    #[arg(long, value_name = "ID")]
    pub template: Option<String>,
    /// Watch these nodes (id, hostname or name; repeatable). Node templates.
    #[arg(long = "node", value_name = "NODE")]
    pub nodes: Vec<String>,
    /// Watch these services (id or name; repeatable). Service templates.
    #[arg(long = "service", value_name = "SERVICE")]
    pub apps: Vec<String>,
    /// Watch these package installations, databases included (installation
    /// id or package name; repeatable). Database templates; a database-host
    /// template watches the installation's node.
    #[arg(long = "package", value_name = "PACKAGE")]
    pub packages: Vec<String>,
    /// Watch these ingress certificates (id or name; repeatable).
    /// Certificate templates.
    #[arg(long = "certificate", value_name = "CERT")]
    pub certificates: Vec<String>,
    /// The health check a service template watches (fills `{check}`).
    #[arg(long, value_name = "NAME")]
    pub check: Option<String>,
    /// Override the template's severity.
    #[arg(long, value_enum)]
    pub severity: Option<Severity>,
    /// Override the template's name.
    #[arg(long)]
    pub name: Option<String>,
    /// Create the rule in this workspace (id; default: the server's).
    #[arg(long)]
    pub workspace: Option<String>,
    /// Create the rule disabled.
    #[arg(long)]
    pub disabled: bool,
}

/// The ids each flag resolved to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Targets {
    pub nodes: Vec<String>,
    pub apps: Vec<String>,
    pub installations: Vec<String>,
    /// Nodes of the `--package` installations (database-host templates).
    pub installation_nodes: Vec<String>,
    pub certificates: Vec<String>,
}

fn usage(m: impl Into<String>) -> CliError {
    CliError::usage(m)
}

/// The rule body for `POST /api/v1/alert-rules` from a template (as the
/// server lists it) and resolved targets. No targets: every resource of the
/// template's kind in the workspace.
pub fn rule_body(t: &Value, flags: &TemplateFlags, targets: &Targets) -> Result<Value> {
    let id = t["id"].as_str().unwrap_or_default();
    let scope = t["scope"].as_str().unwrap_or_default();
    let wrong = |flag: &str| {
        usage(format!("{flag} does not apply to template `{id}`: it watches {scope}; see `forge alerts templates`"))
    };

    let ids: Vec<String> = match scope {
        "servers" => {
            if !targets.apps.is_empty() {
                return Err(wrong("--service"));
            }

            if !targets.certificates.is_empty() {
                return Err(wrong("--certificate"));
            }

            let mut ids = targets.nodes.clone();
            ids.extend(targets.installation_nodes.iter().cloned());
            ids.sort();
            ids.dedup();
            ids
        }
        "services" => {
            for (flag, set) in [
                ("--node", &targets.nodes),
                ("--package", &targets.installations),
                ("--certificate", &targets.certificates),
            ] {
                if !set.is_empty() {
                    return Err(wrong(flag));
                }
            }

            targets.apps.clone()
        }
        "installations" => {
            for (flag, set) in
                [("--node", &targets.nodes), ("--service", &targets.apps), ("--certificate", &targets.certificates)]
            {
                if !set.is_empty() {
                    return Err(wrong(flag));
                }
            }

            targets.installations.clone()
        }
        "certificates" => {
            for (flag, set) in
                [("--node", &targets.nodes), ("--service", &targets.apps), ("--package", &targets.installations)]
            {
                if !set.is_empty() {
                    return Err(wrong(flag));
                }
            }

            targets.certificates.clone()
        }
        other => return Err(CliError::new(Exit::General, format!("template `{id}` has an unknown scope `{other}`"))),
    };

    let mut subject = t["subject"].as_str().unwrap_or_default().to_owned();
    if subject.contains("{check}") {
        let Some(check) = &flags.check else {
            return Err(usage(format!("template `{id}` watches a health check: pass --check <name>")));
        };

        subject = subject.replace("{check}", check);
    } else if flags.check.is_some() {
        return Err(usage(format!("template `{id}` does not watch a health check; drop --check")));
    }

    let mut body = json!({
        "name": flags.name.clone().unwrap_or_else(|| t["name"].as_str().unwrap_or(id).to_owned()),
        "description": t["description"],
        "enabled": !flags.disabled,
        "severity": flags.severity.map_or_else(|| t["severity"].clone(), |s| json!(s.as_str())),
        "scope": { "kind": scope, "ids": ids },
        "subject": subject,
        "spec": t["spec"],
    });

    if let Some(w) = &flags.workspace {
        body["workspace_id"] = json!(w);
    }

    Ok(body)
}

/// One template from the server by id.
pub async fn fetch(api: &mut Api, id: &str) -> Result<Value> {
    let all = api.get("/api/v1/alert-rule-templates").await?;
    all.as_array().into_iter().flatten().find(|t| t["id"].as_str() == Some(id)).cloned().ok_or_else(|| {
        CliError::new(Exit::NotFound, format!("no alert rule template `{id}`; see `forge alerts templates`"))
    })
}

/// Exactly one match by id or name, never the first of several.
fn one<'a>(what: &str, key: &str, matches: &[&'a Value]) -> Result<&'a Value> {
    match matches {
        [one] => Ok(one),
        [] => Err(CliError::new(Exit::NotFound, format!("no {what} `{key}` that you can see"))),
        many => Err(usage(format!(
            "`{key}` matches {} {what}s; use an id: {}",
            many.len(),
            many.iter().filter_map(|v| v["id"].as_str()).collect::<Vec<_>>().join(", ")
        ))
        .with_code("ambiguous")),
    }
}

/// Resolves every target flag to ids.
pub async fn resolve(api: &mut Api, flags: &TemplateFlags) -> Result<Targets> {
    let mut t = Targets::default();

    for n in &flags.nodes {
        t.nodes.push(resolve_node(api, n).await?["id"].as_str().unwrap_or_default().to_owned());
    }

    for a in &flags.apps {
        t.apps.push(crate::rescmd::resolve_app(api, a, None).await?["id"].as_str().unwrap_or_default().to_owned());
    }

    if !flags.packages.is_empty() {
        let all = api.get("/api/v1/installations").await?;

        for p in &flags.packages {
            let m: Vec<&Value> = all
                .as_array()
                .into_iter()
                .flatten()
                .filter(
                    |i| if looks_like_id(p) { i["id"].as_str() == Some(p) } else { i["package"].as_str() == Some(p) },
                )
                .collect();
            let i = one("installation", p, &m)?;
            t.installations.push(i["id"].as_str().unwrap_or_default().to_owned());
            t.installation_nodes.push(i["server_id"].as_str().unwrap_or_default().to_owned());
        }
    }

    if !flags.certificates.is_empty() {
        let all = api.get("/api/v1/certificates").await?;

        for c in &flags.certificates {
            let m: Vec<&Value> = all
                .as_array()
                .into_iter()
                .flatten()
                .filter(|x| x["id"].as_str() == Some(c) || x["name"].as_str() == Some(c))
                .collect();
            t.certificates.push(one("certificate", c, &m)?["id"].as_str().unwrap_or_default().to_owned());
        }
    }

    Ok(t)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // reason: tests
mod tests {
    use super::*;

    fn template(scope: &str, subject: &str) -> Value {
        json!({
            "id": "t", "name": "T", "description": "d", "severity": "warning",
            "scope": scope, "subject": subject, "spec": { "trigger": {}, "mode": {}, "recovery": {} },
        })
    }

    #[test]
    fn a_node_template_targets_nodes_and_package_hosts() {
        let targets = Targets {
            nodes: vec!["n1".into()],
            installation_nodes: vec!["n2".into(), "n1".into()],
            ..Targets::default()
        };
        let b = rule_body(&template("servers", "node.mem.used_percent"), &TemplateFlags::default(), &targets).unwrap();
        assert_eq!(b["scope"], json!({ "kind": "servers", "ids": ["n1", "n2"] }));
        assert_eq!(b["severity"], "warning");
        assert_eq!(b["name"], "T");
        assert_eq!(b["enabled"], true);
        assert!(b.get("workspace_id").is_none());
    }

    #[test]
    fn overrides_and_check_names_apply() {
        let flags = TemplateFlags {
            check: Some("http".into()),
            severity: Some(Severity::Critical),
            name: Some("Shop down".into()),
            workspace: Some("w1".into()),
            disabled: true,
            ..TemplateFlags::default()
        };
        let targets = Targets { apps: vec!["a1".into()], ..Targets::default() };
        let b = rule_body(&template("services", "health.liveness.{check}"), &flags, &targets).unwrap();
        assert_eq!(b["subject"], "health.liveness.http");
        assert_eq!(b["severity"], "critical");
        assert_eq!(b["name"], "Shop down");
        assert_eq!(b["workspace_id"], "w1");
        assert_eq!(b["enabled"], false);
    }

    #[test]
    fn mismatched_targets_and_missing_checks_are_refused() {
        let flags = TemplateFlags::default();
        let nodes = Targets { nodes: vec!["n".into()], ..Targets::default() };
        assert!(rule_body(&template("installations", "package.state"), &flags, &nodes).is_err());
        assert!(rule_body(&template("services", "health.readiness.{check}"), &flags, &Targets::default()).is_err());
        let with_check = TemplateFlags { check: Some("x".into()), ..TemplateFlags::default() };
        assert!(rule_body(&template("servers", "node.cpu.percent"), &with_check, &Targets::default()).is_err());
        let apps = Targets { apps: vec!["a".into()], ..Targets::default() };
        assert!(rule_body(&template("certificates", "tls.cert.expires_in_days"), &flags, &apps).is_err());
    }
}
