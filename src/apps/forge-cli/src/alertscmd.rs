//! `forge alerts …`: alert rules and alarms (#11, D-087).

use clap::Subcommand;
use serde_json::{Value, json};

use crate::admincmd::read_json;
use crate::cmds::{confirm, resolve_node};
use crate::error::Result;
use crate::out::{self, Col};
use crate::{Global, alerttemplates, connect, notifycmd, silencecmd, target};

#[derive(Subcommand, Debug)]
pub enum AlertsCmd {
    /// List alarms, newest first.
    Alarms {
        /// Only firing alarms.
        #[arg(long)]
        active: bool,
        /// Only this rule's alarms (rule id).
        #[arg(long)]
        rule: Option<String>,
        /// Only this node's alarms (id, hostname or name).
        #[arg(long)]
        node: Option<String>,
        /// Only this workspace's alarms (id).
        #[arg(long)]
        workspace: Option<String>,
        #[command(flatten)]
        paging: out::Paging,
    },
    /// Show one alarm and its timeline.
    Alarm { id: String },
    /// Acknowledge an alarm (it keeps firing until it recovers).
    Ack {
        id: String,
        /// A note for the timeline.
        #[arg(long, default_value = "")]
        note: String,
    },
    /// Remove an alarm's acknowledgement.
    Unack { id: String },
    /// Add a note to an alarm's timeline.
    Note { id: String, text: String },
    /// List alert rules.
    Rules {
        /// Only this workspace's rules (id).
        #[arg(long)]
        workspace: Option<String>,
        #[command(flatten)]
        paging: out::Paging,
    },
    /// Show one rule.
    Rule { id: String },
    /// List the subjects a rule can watch: values, units, scopes, and the
    /// health checks seen in the workspace.
    Subjects {
        /// The workspace whose health checks are listed (id).
        #[arg(long)]
        workspace: Option<String>,
    },
    /// List ready-made rules to create with `create-rule --template`.
    Templates {
        /// Only templates for this place.
        #[arg(long, value_enum)]
        place: Option<alerttemplates::Place>,
        /// Check access in this workspace (id; default: the server's).
        #[arg(long)]
        workspace: Option<String>,
    },
    /// Create a rule from a JSON document (`@rule.json`, `@-` for stdin), or
    /// from a template with `--template`.
    CreateRule {
        /// The rule as JSON. Not with `--template`.
        #[arg(required_unless_present = "template", conflicts_with = "template")]
        file: Option<String>,
        #[command(flatten)]
        from: alerttemplates::TemplateFlags,
    },
    /// Replace a rule from a JSON document (same shape as create).
    UpdateRule { id: String, file: String },
    /// Delete a rule (its alarms stay as history).
    DeleteRule {
        id: String,
        #[arg(long)]
        yes: bool,
    },
    /// Notification channels: where alarm messages go.
    #[command(subcommand)]
    Channels(notifycmd::ChannelsCmd),
    /// Notification policies: which alarms notify which channels.
    #[command(subcommand)]
    Policies(notifycmd::PoliciesCmd),
    /// Notification deliveries: the messages sent and their attempts.
    #[command(subcommand)]
    Deliveries(notifycmd::DeliveriesCmd),
    /// Silences and maintenance windows: hold back the notifications of
    /// matching alarms for a while.
    #[command(subcommand)]
    Silences(silencecmd::SilencesCmd),
}

const ALARM_COLS: &[Col] = &[
    ("ID", "/id"),
    ("STATE", "/state"),
    ("SEVERITY", "/severity"),
    ("TITLE", "/title"),
    ("NODE", "/node"),
    ("FIRED (UTC)", "/fired"),
    ("ACKNOWLEDGED", "/acknowledged"),
];

/// `2026-11-14 18:20:25` (UTC) from epoch milliseconds.
fn when(ms: Option<i64>) -> String {
    let Some(t) = ms.and_then(|ms| time::OffsetDateTime::from_unix_timestamp(ms / 1000).ok()) else {
        return "-".into();
    };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        t.year(),
        u8::from(t.month()),
        t.day(),
        t.hour(),
        t.minute(),
        t.second()
    )
}

const RULE_COLS: &[Col] = &[
    ("ID", "/id"),
    ("NAME", "/name"),
    ("ENABLED", "/enabled"),
    ("SEVERITY", "/severity"),
    ("SUBJECT", "/subject"),
    ("REVISION", "/revision"),
];

const SUBJECT_COLS: &[Col] =
    &[("SUBJECT", "/subject"), ("KIND", "/kind"), ("VALUES", "/values"), ("SCOPE", "/scope"), ("SOURCE", "/source")];

const TEMPLATE_COLS: &[Col] = &[
    ("ID", "/id"),
    ("PLACE", "/place"),
    ("SEVERITY", "/severity"),
    ("SUBJECT", "/subject"),
    ("CONDITION", "/condition"),
];

/// Table rows for `alerts subjects`: the catalogue, then the health checks
/// seen in the workspace.
fn subject_rows(v: &Value) -> Value {
    let mut rows: Vec<Value> = v["subjects"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|s| {
            let values = match s["unit"].as_str() {
                Some(u) => u.to_owned(),
                None => s["states"].as_array().into_iter().flatten().filter_map(Value::as_str).collect::<Vec<_>>().join("|"),
            };
            let scope = s["scopes"].as_array().into_iter().flatten().filter_map(Value::as_str).collect::<Vec<_>>().join(",");
            json!({ "subject": s["subject"], "kind": s["kind"], "values": values, "scope": scope, "source": s["source"] })
        })
        .collect();
    for h in v["seen_health"].as_array().into_iter().flatten() {
        rows.push(json!({ "subject": h, "kind": "state", "values": "(seen)", "scope": "services", "source": "probe" }));
    }
    Value::Array(rows)
}

/// `> 90 for 10 min` from a rule spec, for the templates table.
fn describe_spec(spec: &Value) -> String {
    let t = &spec["trigger"];
    let cond = match t["kind"].as_str() {
        Some("threshold") => {
            let cmp = match t["cmp"].as_str() {
                Some("gt") => ">",
                Some("ge") => ">=",
                Some("lt") => "<",
                Some("le") => "<=",
                Some("eq") => "=",
                _ => "!=",
            };
            format!("{cmp} {}", t["value"])
        }
        Some(k) => {
            let states: Vec<&str> = t["states"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
            format!("{} {}", if k == "state_in" { "is" } else { "is not" }, states.join("|"))
        }
        None => String::new(),
    };
    let m = &spec["mode"];
    let min = |key: &str| m[key].as_i64().unwrap_or(0) / 60_000;
    match m["mode"].as_str() {
        Some("sustained") => format!("{cond} for {} min", min("for_ms")),
        Some("window_average") => format!("average {cond} over {} min", min("window_ms")),
        Some("repeated") => format!("{cond} {}x in {} min", m["count"], min("window_ms")),
        Some("any_in_window") => format!("{cond} any time in {} min", min("window_ms")),
        _ => cond,
    }
}

fn show(g: &Global, v: &Value, cols: Option<&[Col]>) -> Result<()> {
    out::result(g.output, v, cols);
    Ok(())
}

fn enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

pub async fn run(g: &Global, cmd: AlertsCmd) -> Result<()> {
    let t = target(g)?;
    let mut api = connect(g, &t).await?;
    match cmd {
        AlertsCmd::Alarms { active, rule, node, workspace, paging } => {
            let page: out::Page = paging.into();
            // The server caps a request at 500 alarms.
            let fetch = page.fetch().unwrap_or(500).min(500);
            let mut q = format!("/api/v1/alarms?limit={fetch}");
            if let Some(w) = workspace {
                q.push_str(&format!("&workspace={}", enc(&w)));
            }
            if active {
                q.push_str("&active=true");
            }
            if let Some(r) = rule {
                q.push_str(&format!("&rule={}", enc(&r)));
            }
            if let Some(n) = node {
                let id = resolve_node(&mut api, &n).await?["id"].as_str().unwrap_or_default().to_owned();
                q.push_str(&format!("&server={}", enc(&id)));
            }
            let mut v = api.get(&q).await?;
            // Readable times and node names for the table (JSON keeps the raw fields).
            let nodes = api.get("/api/v1/servers").await.unwrap_or(Value::Null);
            for a in v.as_array_mut().into_iter().flatten() {
                let id = a["server"].as_str().or(a["resource"]["id"].as_str()).unwrap_or_default().to_owned();
                let name = nodes
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|n| n["id"] == id.as_str())
                    .and_then(|n| n["display_name"].as_str().or(n["hostname"].as_str()))
                    .map_or_else(|| id.chars().take(8).collect(), str::to_owned);
                a["node"] = json!(name);
                a["fired"] = json!(when(a["fired_at"].as_i64()));
                a["acknowledged"] = json!(if a["acknowledged_at"].is_number() { "yes" } else { "no" });
            }
            let rows = crate::cmds::rows_of(v);
            let total = if rows.len() >= fetch { out::Total::AtLeast } else { out::Total::Rows };
            out::list(g.output, rows, Some(ALARM_COLS), page, total);
            Ok(())
        }
        AlertsCmd::Alarm { id } => {
            let v = api.get(&format!("/api/v1/alarms/{}", enc(&id))).await?;
            show(g, &v, None)
        }
        AlertsCmd::Ack { id, note } => {
            let v = api.post(&format!("/api/v1/alarms/{}/acknowledge", enc(&id)), &json!({ "note": note })).await?;
            show(g, &v, None)
        }
        AlertsCmd::Unack { id } => {
            let v = api.post(&format!("/api/v1/alarms/{}/unacknowledge", enc(&id)), &json!({})).await?;
            show(g, &v, None)
        }
        AlertsCmd::Note { id, text } => {
            api.post(&format!("/api/v1/alarms/{}/notes", enc(&id)), &json!({ "note": text })).await?;
            show(g, &json!({ "added": true }), None)
        }
        AlertsCmd::Rules { workspace, paging } => {
            let path = match workspace {
                Some(w) => format!("/api/v1/alert-rules?workspace={}", enc(&w)),
                None => "/api/v1/alert-rules".to_owned(),
            };
            let v = api.get(&path).await?;
            out::list_value(g.output, v, "", Some(RULE_COLS), paging);
            Ok(())
        }
        AlertsCmd::Rule { id } => {
            let v = api.get(&format!("/api/v1/alert-rules/{}", enc(&id))).await?;
            show(g, &v, None)
        }
        AlertsCmd::Subjects { workspace } => {
            let path = match workspace {
                Some(w) => format!("/api/v1/alert-subjects?workspace={}", enc(&w)),
                None => "/api/v1/alert-subjects".to_owned(),
            };
            let v = api.get(&path).await?;
            if g.output != out::Format::Table {
                return show(g, &v, None);
            }
            show(g, &subject_rows(&v), Some(SUBJECT_COLS))
        }
        AlertsCmd::Templates { place, workspace } => {
            let mut q = Vec::new();
            if let Some(p) = place {
                q.push(format!("place={}", p.as_str()));
            }
            if let Some(w) = workspace {
                q.push(format!("workspace={}", enc(&w)));
            }
            let path = if q.is_empty() {
                "/api/v1/alert-rule-templates".to_owned()
            } else {
                format!("/api/v1/alert-rule-templates?{}", q.join("&"))
            };
            let mut v = api.get(&path).await?;
            for t in v.as_array_mut().into_iter().flatten() {
                t["condition"] = json!(describe_spec(&t["spec"]));
            }
            show(g, &v, Some(TEMPLATE_COLS))
        }
        AlertsCmd::CreateRule { file, from } => {
            let body = match (file, &from.template) {
                (Some(file), _) => read_json(&file)?,
                (None, Some(id)) => {
                    let t = alerttemplates::fetch(&mut api, id).await?;
                    let targets = alerttemplates::resolve(&mut api, &from).await?;
                    alerttemplates::rule_body(&t, &from, &targets)?
                }
                (None, None) => return Err(crate::error::CliError::usage("pass a JSON file or --template <id>")),
            };
            let v = api.post("/api/v1/alert-rules", &body).await?;
            show(g, &v, None)
        }
        AlertsCmd::UpdateRule { id, file } => {
            let body = read_json(&file)?;
            let v = api.call(reqwest::Method::PUT, &format!("/api/v1/alert-rules/{}", enc(&id)), Some(&body)).await?;
            show(g, &v, None)
        }
        AlertsCmd::DeleteRule { id, yes } => {
            confirm(g, yes, &format!("Delete alert rule {id} on {}? Its alarms stay as history.", t.server))?;
            api.call(reqwest::Method::DELETE, &format!("/api/v1/alert-rules/{}", enc(&id)), None).await?;
            show(g, &json!({ "deleted": id }), None)
        }
        AlertsCmd::Channels(c) => notifycmd::channels(g, &t.server, &mut api, c).await,
        AlertsCmd::Policies(c) => notifycmd::policies(g, &t.server, &mut api, c).await,
        AlertsCmd::Deliveries(c) => notifycmd::deliveries(g, &mut api, c).await,
        AlertsCmd::Silences(c) => silencecmd::run(g, &t.server, &mut api, c).await,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // reason: tests
mod tests {
    use clap::Parser;

    use super::*;
    use crate::{Cli, Cmd};

    fn parse(args: &[&str]) -> std::result::Result<AlertsCmd, clap::Error> {
        let mut all = vec!["forge", "alerts"];
        all.extend_from_slice(args);
        Cli::try_parse_from(all).map(|c| match c.cmd {
            Cmd::Alerts(a) => a,
            other => panic!("{other:?}"),
        })
    }

    #[test]
    fn create_rule_takes_a_file_or_a_template() {
        match parse(&["create-rule", "@rule.json"]).unwrap() {
            AlertsCmd::CreateRule { file, from } => {
                assert_eq!(file.as_deref(), Some("@rule.json"));
                assert_eq!(from.template, None);
            }
            other => panic!("{other:?}"),
        }
        let cmd = parse(&[
            "create-rule",
            "--template",
            "node-disk-filling",
            "--node",
            "web-1",
            "--node",
            "web-2",
            "--severity",
            "critical",
            "--name",
            "Disk",
        ])
        .unwrap();
        let AlertsCmd::CreateRule { file: None, from } = cmd else { panic!("{cmd:?}") };
        assert_eq!(from.template.as_deref(), Some("node-disk-filling"));
        assert_eq!(from.nodes, vec!["web-1", "web-2"]);
        assert_eq!(from.severity, Some(alerttemplates::Severity::Critical));
        assert_eq!(from.name.as_deref(), Some("Disk"));
        let AlertsCmd::CreateRule { from, .. } =
            parse(&["create-rule", "--template", "database-stopped", "--package", "postgresql"]).unwrap()
        else {
            panic!()
        };
        assert_eq!(from.packages, vec!["postgresql"]);
        let AlertsCmd::CreateRule { from, .. } = parse(&[
            "create-rule",
            "--template",
            "service-liveness-failing",
            "--service",
            "shop",
            "--check",
            "http",
            "--disabled",
        ])
        .unwrap() else {
            panic!()
        };
        assert_eq!((from.apps, from.check, from.disabled), (vec!["shop".to_owned()], Some("http".to_owned()), true));
        assert!(parse(&["create-rule"]).is_err(), "a file or a template");
        assert!(parse(&["create-rule", "@r.json", "--template", "x"]).is_err(), "not both");
        assert!(parse(&["create-rule", "--template", "x", "--severity", "huge"]).is_err());
    }

    #[test]
    fn templates_and_subjects_parse() {
        assert!(matches!(
            parse(&["templates", "--place", "database"]).unwrap(),
            AlertsCmd::Templates { place: Some(alerttemplates::Place::Database), workspace: None }
        ));
        assert!(matches!(parse(&["templates"]).unwrap(), AlertsCmd::Templates { place: None, workspace: None }));
        assert!(parse(&["templates", "--place", "moon"]).is_err());
        assert!(matches!(parse(&["subjects"]).unwrap(), AlertsCmd::Subjects { workspace: None }));
    }

    #[test]
    fn specs_read_as_conditions() {
        let spec = json!({
            "trigger": { "kind": "threshold", "cmp": "gt", "value": 90.0 },
            "mode": { "mode": "window_average", "window_ms": 600_000, "min_coverage_pct": 80 },
            "recovery": {}
        });
        assert_eq!(describe_spec(&spec), "average > 90.0 over 10 min");
        let spec = json!({
            "trigger": { "kind": "state_in", "states": ["failing"] },
            "mode": { "mode": "repeated", "count": 3, "window_ms": 900_000, "min_spacing_ms": 0 },
            "recovery": {}
        });
        assert_eq!(describe_spec(&spec), "is failing 3x in 15 min");
    }
}
