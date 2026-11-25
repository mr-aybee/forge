//! `forge clusters ingress`: a cluster's ingress profile, the Nginx layout on
//! its edge nodes (`/api/v1/clusters/{id}/profile`). `set` saves it; `apply`
//! pushes it to the edges; `remove` takes it off them.

use std::collections::HashMap;

use clap::{Subcommand, ValueEnum};
use reqwest::Method;
use serde_json::{Map, Value, json};

use crate::admincmd::{check_body, cluster_id, node_id, read_json};
use crate::cmds::confirm;
use crate::error::{CliError, Exit, Result};
use crate::out::{self, Col};
use crate::{Global, connect, target};

/// How the edges of a cluster share the traffic.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// One edge node.
    #[value(name = "standalone")]
    Standalone,
    /// One primary edge serves; the backups take over when it fails.
    #[value(name = "active_standby")]
    ActiveStandby,
    /// Every edge serves.
    #[value(name = "active_active")]
    ActiveActive,
}

impl Mode {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Standalone => "standalone",
            Self::ActiveStandby => "active_standby",
            Self::ActiveActive => "active_active",
        }
    }
}

#[derive(Subcommand, Debug)]
pub enum IngressCmd {
    /// The cluster's ingress profile and what each edge last reported.
    #[command(alias = "show")]
    Get {
        /// The cluster (id, name or abbreviation).
        cluster: String,
    },
    /// Save the ingress profile: from a JSON file, from --edge and --mode, or
    /// both (the flags replace the file's edges and mode). Run `apply` next.
    Set {
        /// The cluster (id, name or abbreviation).
        cluster: String,
        /// The whole profile as JSON (`@-` reads stdin). Without it the
        /// flags change the saved profile and keep its other settings.
        #[arg(long)]
        file: Option<String>,
        /// An edge node: `<node>[:primary|backup][=<public-ip>]`. Repeat for
        /// more; the list replaces the profile's edges. Without a role the
        /// first edge is the primary and the others are backups.
        #[arg(long = "edge", value_name = "EDGE")]
        edges: Vec<String>,
        /// How the edges share the traffic. Default: the saved mode, or
        /// standalone for a single edge.
        #[arg(long, value_enum)]
        mode: Option<Mode>,
        /// Do not ask first (required with --non-interactive).
        #[arg(long)]
        yes: bool,
    },
    /// Push the saved profile to its edge nodes (configures and reloads Nginx there).
    Apply {
        /// The cluster (id, name or abbreviation).
        cluster: String,
        /// Do not ask first (required with --non-interactive).
        #[arg(long)]
        yes: bool,
    },
    /// Take the ingress configuration off the edge nodes. An edge that
    /// reports it removed leaves the profile.
    Remove {
        /// The cluster (id, name or abbreviation).
        cluster: String,
        /// Do not ask first (required with --non-interactive).
        #[arg(long)]
        yes: bool,
    },
}

/// The role of an edge in the profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeRole {
    Primary,
    Backup,
}

impl EdgeRole {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Backup => "backup",
        }
    }
}

/// One `--edge`: the node as typed, its role if given, its public address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EdgeArg {
    pub node: String,
    pub role: Option<EdgeRole>,
    pub public_ip: Option<String>,
}

/// Parses `<node>[:primary|backup][=<public-ip>]`.
pub fn parse_edge(spec: &str) -> Result<EdgeArg> {
    let bad = |why: &str| CliError::usage(format!("--edge {spec}: {why}"));

    let (left, ip) = match spec.split_once('=') {
        Some((l, ip)) => (l, Some(ip)),
        None => (spec, None),
    };

    if let Some(ip) = ip
        && ip.parse::<std::net::IpAddr>().is_err()
    {
        return Err(bad("the public IP is not an IP address"));
    }

    let (node, role) = match left.rsplit_once(':') {
        None => (left, None),
        Some((n, "primary")) => (n, Some(EdgeRole::Primary)),
        Some((n, "backup")) => (n, Some(EdgeRole::Backup)),
        Some((_, _)) => return Err(bad("the role is primary or backup")),
    };

    if node.is_empty() {
        return Err(bad("name the node"));
    }

    Ok(EdgeArg { node: node.to_owned(), role, public_ip: ip.map(str::to_owned) })
}

/// The profile `set` sends: `base` (the file, else the saved profile) with
/// the edges and mode from the flags. `edges` pairs each `--edge` with its
/// node id.
pub fn profile_body(base: Option<Value>, mode: Option<Mode>, edges: &[(String, EdgeArg)]) -> Result<Value> {
    let mut body = match base {
        Some(Value::Object(m)) => m,
        Some(Value::Null) | None => Map::new(),
        Some(_) => return Err(CliError::usage("the profile must be a JSON object")),
    };

    let mut ingress = match body.remove("ingress") {
        Some(Value::Object(m)) => m,
        _ => Map::new(),
    };

    if !edges.is_empty() {
        let list: Vec<Value> = edges
            .iter()
            .enumerate()
            .map(|(i, (id, e))| {
                let role = e.role.unwrap_or(if i == 0 { EdgeRole::Primary } else { EdgeRole::Backup });
                let mut edge = json!({ "server_id": id, "role": role.as_str() });

                if let Some(ip) = &e.public_ip {
                    edge["public_ip"] = json!(ip);
                }

                edge
            })
            .collect();
        ingress.insert("edges".into(), Value::Array(list));
    }

    let count = ingress.get("edges").and_then(Value::as_array).map_or(0, Vec::len);
    if count == 0 {
        return Err(CliError::usage("the profile has no edge node: pass --edge <node> or --file"));
    }

    match mode {
        Some(m) => {
            ingress.insert("mode".into(), json!(m.as_str()));
        }
        None if ingress.get("mode").is_some_and(Value::is_string) => {}
        None if count == 1 => {
            ingress.insert("mode".into(), json!(Mode::Standalone.as_str()));
        }
        None => {
            return Err(CliError::usage(format!(
                "{count} edge nodes need a mode: pass --mode active_standby or --mode active_active"
            )));
        }
    }

    body.insert("ingress".into(), Value::Object(ingress));

    Ok(Value::Object(body))
}

const EDGE_COLS: &[Col] = &[("NODE", "/node"), ("STATUS", "/status"), ("ERROR", "/error"), ("JOB", "/job_id")];

/// Node id → name, from the `members` of a profile answer.
fn member_names(profile: &Value) -> HashMap<String, String> {
    profile["members"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| {
            let name = m["hostname"].as_str().filter(|h| !h.is_empty()).or_else(|| m["name"].as_str())?;
            Some((m["id"].as_str()?.to_owned(), name.to_owned()))
        })
        .collect()
}

/// The per-edge results of `apply` / `remove` (`{<server id>: {status, …}}`) as rows.
fn edge_rows(edges: &Value, names: &HashMap<String, String>) -> Vec<Value> {
    let mut rows: Vec<Value> = edges
        .as_object()
        .into_iter()
        .flatten()
        .map(|(id, r)| {
            json!({
                "node": names.get(id).cloned().unwrap_or_else(|| id.clone()),
                "server_id": id,
                "status": r["status"],
                "error": r["error"],
                "job_id": r["job_id"],
            })
        })
        .collect();
    rows.sort_by(|a, b| a["node"].as_str().cmp(&b["node"].as_str()));

    rows
}

/// `forge clusters ingress get` for people: the layout and one line per edge.
fn summary(v: &Value) -> Value {
    let cluster = v["cluster"]["name"].clone();
    let ingress = &v["profile"]["ingress"];
    if !ingress.is_object() {
        return json!({ "cluster": cluster, "profile": "none (save one with `forge clusters ingress set`)" });
    }

    let names = member_names(v);
    let edges: Vec<Value> = ingress["edges"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|e| {
            let id = e["server_id"].as_str().unwrap_or_default();
            let last = &v["status"]["edges"][id];
            json!({
                "node": names.get(id).cloned().unwrap_or_else(|| id.to_owned()),
                "role": e["role"],
                "public_ip": e["public_ip"],
                "status": if last["status"].is_string() { last["status"].clone() } else { json!("not applied") },
                "error": last["error"],
            })
        })
        .collect();
    let mut s = Map::new();
    s.insert("cluster".into(), cluster);
    s.insert("mode".into(), ingress["mode"].clone());
    s.insert("failover".into(), ingress["failover"].clone());

    if let Some(vips) = ingress["vrrp"]["vips"].as_array().filter(|a| !a.is_empty()) {
        s.insert("virtual_ips".into(), json!(vips));
    }

    // `dirty`: saved but not applied since.
    s.insert(
        "applied".into(),
        json!(if v["status"]["dirty"] == false { "yes" } else { "no (run `forge clusters ingress apply`)" }),
    );

    if let Some(at) = v["status"]["applied_at"].as_i64() {
        s.insert("applied_at".into(), json!(at));
    }

    s.insert("edges".into(), Value::Array(edges));

    Value::Object(s)
}

/// Prints the per-edge results; a failed edge makes the command fail (exit 10).
fn edges_result(g: &Global, what: &str, cluster: &str, v: &Value, names: &HashMap<String, String>) -> Result<()> {
    let rows = edge_rows(&v["edges"], names);
    let failed: Vec<String> = rows
        .iter()
        .filter(|r| r["status"] == "failed")
        .map(|r| format!("{}: {}", r["node"].as_str().unwrap_or("?"), r["error"].as_str().unwrap_or("failed")))
        .collect();

    if g.output.is_machine() {
        if failed.is_empty() {
            out::result(g.output, v, None);
        }
    } else {
        out::result(g.output, &Value::Array(rows), Some(EDGE_COLS));
    }

    if failed.is_empty() {
        return Ok(());
    }

    Err(CliError::new(
        Exit::JobFailed,
        format!("{what} of the ingress of cluster {cluster} failed: {}", failed.join("; ")),
    )
    .with_code("ingress_failed"))
}

fn edge_list(profile: &Value, names: &HashMap<String, String>) -> String {
    let list: Vec<String> = profile["profile"]["ingress"]["edges"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| e["server_id"].as_str())
        .map(|id| names.get(id).cloned().unwrap_or_else(|| id.to_owned()))
        .collect();
    if list.is_empty() { "no edge node".to_owned() } else { list.join(", ") }
}

pub async fn run(g: &Global, cmd: IngressCmd) -> Result<()> {
    let t = target(g)?;
    let mut api = connect(g, &t).await?;

    match cmd {
        IngressCmd::Get { cluster } => {
            let id = cluster_id(&mut api, &cluster).await?;
            let v = api.get(&format!("/api/v1/clusters/{id}/profile")).await?;

            if g.output.is_machine() {
                out::result(g.output, &v, None);
            } else {
                out::result(g.output, &summary(&v), None);
            }

            Ok(())
        }
        IngressCmd::Set { cluster, file, edges, mode, yes } => {
            if file.is_none() && edges.is_empty() && mode.is_none() {
                return Err(CliError::usage("pass --file, or --edge <node> (and --mode)"));
            }

            let args: Vec<EdgeArg> = edges.iter().map(|e| parse_edge(e)).collect::<Result<_>>()?;
            let id = cluster_id(&mut api, &cluster).await?;
            let path = format!("/api/v1/clusters/{id}/profile");

            let base = match &file {
                Some(f) => read_json(f)?,
                None => api.get(&path).await?["profile"].take(),
            };

            let mut resolved = Vec::new();

            for e in args {
                resolved.push((node_id(&mut api, &e.node).await?, e));
            }

            let body = profile_body(Some(base), mode, &resolved)?;
            check_body(&mut api, "PUT", &path, &body).await?;
            confirm(
                g,
                yes,
                &format!(
                    "Save the ingress profile of cluster {cluster} ({}, {} edge node(s)) on {}? Apply it afterwards.",
                    body["ingress"]["mode"].as_str().unwrap_or("?"),
                    body["ingress"]["edges"].as_array().map_or(0, Vec::len),
                    t.server
                ),
            )?;
            let v = api.call(Method::PUT, &path, Some(&body)).await?;

            if !g.output.is_machine() {
                eprintln!("Saved. Run `forge clusters ingress apply {cluster}` to push it to the edge nodes.");
            }

            out::result(g.output, &v, None);
            Ok(())
        }
        IngressCmd::Apply { cluster, yes } => {
            let id = cluster_id(&mut api, &cluster).await?;
            let profile = api.get(&format!("/api/v1/clusters/{id}/profile")).await?;
            let names = member_names(&profile);
            confirm(
                g,
                yes,
                &format!(
                    "Apply the ingress profile of cluster {cluster} to {} ({})? Nginx is configured and reloaded there.",
                    edge_list(&profile, &names),
                    t.server
                ),
            )?;
            let v = api.post(&format!("/api/v1/clusters/{id}/profile/apply"), &json!({})).await?;
            edges_result(g, "apply", &cluster, &v, &names)
        }
        IngressCmd::Remove { cluster, yes } => {
            let id = cluster_id(&mut api, &cluster).await?;
            let profile = api.get(&format!("/api/v1/clusters/{id}/profile")).await?;
            let names = member_names(&profile);
            confirm(
                g,
                yes,
                &format!(
                    "Remove the ingress configuration of cluster {cluster} from {} ({})? Sites served there stop answering.",
                    edge_list(&profile, &names),
                    t.server
                ),
            )?;
            let v = api.post(&format!("/api/v1/clusters/{id}/profile/remove"), &json!({})).await?;
            edges_result(g, "removal", &cluster, &v, &names)
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;
    use crate::cmds::ClustersCmd;
    use crate::{Cli, Cmd};

    fn edge(node: &str, role: Option<EdgeRole>, ip: Option<&str>) -> EdgeArg {
        EdgeArg { node: node.into(), role, public_ip: ip.map(str::to_owned) }
    }

    #[test]
    fn edges_parse_role_and_public_ip() {
        assert_eq!(parse_edge("lab-b").ok(), Some(edge("lab-b", None, None)));
        assert_eq!(parse_edge("lab-b:backup").ok(), Some(edge("lab-b", Some(EdgeRole::Backup), None)));
        assert_eq!(
            parse_edge("lab-b:primary=203.0.113.10").ok(),
            Some(edge("lab-b", Some(EdgeRole::Primary), Some("203.0.113.10")))
        );
        assert_eq!(parse_edge("lab-b=2001:db8::1").ok(), Some(edge("lab-b", None, Some("2001:db8::1"))));

        for bad in ["lab-b:active", "lab-b=not-an-ip", ":primary", "=203.0.113.10"] {
            assert!(parse_edge(bad).is_err_and(|e| e.exit == Exit::Usage), "{bad}");
        }
    }

    #[test]
    fn the_simple_form_builds_a_profile() {
        // One edge: standalone without --mode.
        let one = profile_body(None, None, &[("s1".into(), edge("lab-b", None, None))]).unwrap_or_default();
        assert_eq!(
            one,
            json!({ "ingress": { "mode": "standalone", "edges": [{ "server_id": "s1", "role": "primary" }] } })
        );

        // Several: the first is the primary unless roles are given, and a mode is required.
        let two = [("s1".to_owned(), edge("a", None, Some("203.0.113.10"))), ("s2".to_owned(), edge("b", None, None))];
        assert!(profile_body(None, None, &two).is_err_and(|e| e.message.contains("--mode")));
        let body = profile_body(None, Some(Mode::ActiveStandby), &two).unwrap_or_default();
        assert_eq!(body["ingress"]["mode"], "active_standby");
        assert_eq!(
            body["ingress"]["edges"],
            json!([
                { "server_id": "s1", "role": "primary", "public_ip": "203.0.113.10" },
                { "server_id": "s2", "role": "backup" },
            ])
        );
        assert!(profile_body(None, Some(Mode::Standalone), &[]).is_err(), "no edges, no profile");
    }

    #[test]
    fn flags_change_a_saved_profile_and_keep_the_rest() {
        let saved = json!({ "ingress": {
            "mode": "active_active", "failover": "dns",
            "edges": [{ "server_id": "old", "role": "primary" }],
            "nginx": { "client_max_body_mb": 100 },
        } });
        let two = [
            ("s1".to_owned(), edge("a", Some(EdgeRole::Backup), None)),
            ("s2".to_owned(), edge("b", Some(EdgeRole::Primary), None)),
        ];
        let body = profile_body(Some(saved.clone()), None, &two).unwrap_or_default();
        assert_eq!(body["ingress"]["mode"], "active_active", "the saved mode stays");
        assert_eq!(body["ingress"]["nginx"]["client_max_body_mb"], 100);
        assert_eq!(body["ingress"]["edges"][0], json!({ "server_id": "s1", "role": "backup" }));

        // --mode alone keeps the saved edges.
        let body = profile_body(Some(saved), Some(Mode::Standalone), &[]).unwrap_or_default();
        assert_eq!(
            (&body["ingress"]["mode"], &body["ingress"]["edges"][0]["server_id"]),
            (&json!("standalone"), &json!("old"))
        );
    }

    #[test]
    fn results_name_the_edges_and_summaries_show_what_was_applied() {
        let profile = json!({
            "cluster": { "name": "lab" },
            "profile": { "ingress": { "mode": "active_standby", "failover": "dns", "edges": [
                { "server_id": "s1", "role": "primary", "public_ip": "203.0.113.10" },
                { "server_id": "s2", "role": "backup" },
            ] } },
            "status": { "dirty": false, "edges": { "s1": { "status": "applied" } } },
            "members": [{ "id": "s1", "hostname": "lab-b", "name": "Edge B" }, { "id": "s2", "hostname": "", "name": "lab-c" }],
        });
        let names = member_names(&profile);
        let s = summary(&profile);
        assert_eq!((&s["mode"], &s["applied"]), (&json!("active_standby"), &json!("yes")));
        assert_eq!(
            s["edges"][0],
            json!({ "node": "lab-b", "role": "primary", "public_ip": "203.0.113.10", "status": "applied", "error": null })
        );
        assert_eq!(s["edges"][1]["node"], "lab-c");
        assert_eq!(s["edges"][1]["status"], "not applied");
        assert_eq!(edge_list(&profile, &names), "lab-b, lab-c");
        let rows = edge_rows(
            &json!({ "s2": { "status": "failed", "error": "nginx: test failed", "job_id": "j2" },
                                      "s1": { "status": "applied", "job_id": "j1" } }),
            &names,
        );
        assert_eq!(rows[0]["node"], "lab-b");
        assert_eq!((&rows[1]["node"], &rows[1]["error"]), (&json!("lab-c"), &json!("nginx: test failed")));

        // No profile yet.
        let none = summary(&json!({ "cluster": { "name": "lab" }, "profile": null, "status": {} }));
        assert!(none["profile"].as_str().is_some_and(|p| p.starts_with("none")));
    }

    #[test]
    fn set_parses_edges_and_mode() {
        let cli = Cli::try_parse_from([
            "forge",
            "clusters",
            "ingress",
            "set",
            "lab",
            "--edge",
            "lab-b:primary=203.0.113.10",
            "--edge",
            "lab-c",
            "--mode",
            "active_standby",
            "--yes",
        ]);

        match cli.map(|c| c.cmd) {
            Ok(Cmd::Clusters(ClustersCmd::Ingress(IngressCmd::Set { cluster, file, edges, mode, yes }))) => {
                assert_eq!(
                    (cluster.as_str(), file, edges.len(), mode, yes),
                    ("lab", None, 2, Some(Mode::ActiveStandby), true)
                );
            }
            other => panic!("{other:?}"),
        }

        assert!(
            Cli::try_parse_from(["forge", "clusters", "ingress", "set", "lab", "--mode", "active-standby"]).is_err()
        );
    }
}
