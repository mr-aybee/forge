//! `forge agents …`: agent versions and self-update (GitHub issue #4).

use std::time::{Duration, Instant};

use clap::Subcommand;
use serde_json::{Value, json};

use crate::cmds::{confirm, resolve_node};
use crate::error::{CliError, Exit, Result};
use crate::out::{self, Format};
use crate::{Global, connect, target};

#[derive(Subcommand, Debug)]
pub enum AgentsCmd {
    /// Each node's agent version against the control plane's.
    Versions {
        /// Only outdated nodes.
        #[arg(long)]
        outdated: bool,
    },
    /// Update agents to the control plane's version (verified, with rollback).
    Update {
        /// Nodes (id, hostname or name); omit with --all.
        nodes: Vec<String>,
        /// Every outdated online Linux node.
        #[arg(long, conflicts_with = "nodes")]
        all: bool,
        /// Roll out in batches of N nodes: the first batch is the canary,
        /// the next starts only when the last is healthy, and the first
        /// failure stops the rollout.
        #[arg(long, value_name = "N")]
        canary: Option<usize>,
        /// Wait for the update jobs to finish (exit 10 if one failed).
        #[arg(long)]
        wait: bool,
        #[arg(long, default_value_t = 900)]
        wait_timeout_secs: u64,
        #[arg(long)]
        yes: bool,
    },
    /// Show automatic agent updates, or change them with the flags.
    AutoUpdate {
        /// Turn automatic updates on.
        #[arg(long, conflicts_with = "disable")]
        enable: bool,
        /// Turn automatic updates off.
        #[arg(long)]
        disable: bool,
        /// Weekdays (mon,tue,…); empty: every day.
        #[arg(long, value_delimiter = ',')]
        days: Option<Vec<String>>,
        /// Window start, HH:MM UTC.
        #[arg(long)]
        start: Option<String>,
        /// Window end, HH:MM UTC (may be past midnight).
        #[arg(long)]
        end: Option<String>,
        /// Nodes per batch; the first batch is the canary.
        #[arg(long)]
        batch: Option<usize>,
        #[arg(long)]
        yes: bool,
    },
}

pub async fn run(g: &Global, cmd: AgentsCmd) -> Result<()> {
    let t = target(g)?;
    let mut api = connect(g, &t).await?;
    match cmd {
        AgentsCmd::AutoUpdate { enable, disable, days, start, end, batch, yes } => {
            let cur = api.get("/api/v1/agents/auto-update").await?;
            let changing = enable || disable || days.is_some() || start.is_some() || end.is_some() || batch.is_some();
            if !changing {
                out::result(g.output, &cur, None);
                return Ok(());
            }
            let enabled = if enable {
                true
            } else if disable {
                false
            } else {
                cur["enabled"].as_bool().unwrap_or(false)
            };
            let body = json!({
                "enabled": enabled,
                "days": days.unwrap_or_else(|| {
                    cur["days"].as_array().into_iter().flatten().filter_map(|d| d.as_str().map(str::to_owned)).collect()
                }),
                // A setting never saved reads back as "" / 0: use the defaults then.
                "start": start.unwrap_or_else(|| cur["start"].as_str().filter(|s| !s.is_empty()).unwrap_or("02:00").to_owned()),
                "end": end.unwrap_or_else(|| cur["end"].as_str().filter(|s| !s.is_empty()).unwrap_or("05:00").to_owned()),
                "batch": batch
                    .or_else(|| cur["batch"].as_u64().filter(|b| *b > 0).and_then(|b| usize::try_from(b).ok()))
                    .unwrap_or(1),
            });
            confirm(
                g,
                yes,
                &format!(
                    "{} automatic agent updates on {} ({} {}-{} UTC, batches of {})? Rollouts will run as you.",
                    if enabled { "Turn on" } else { "Turn off" },
                    t.server,
                    body["days"],
                    body["start"].as_str().unwrap_or("?"),
                    body["end"].as_str().unwrap_or("?"),
                    body["batch"]
                ),
            )?;
            let v = api.call(reqwest::Method::PUT, "/api/v1/agents/auto-update", Some(&body)).await?;
            out::result(g.output, &v, None);
        }
        AgentsCmd::Versions { outdated } => {
            let v = api.get("/api/v1/agents/versions").await?;
            let mut nodes = v["nodes"].clone();
            if outdated && let Value::Array(rows) = &mut nodes {
                rows.retain(|n| n["outdated"] == true);
            }
            if g.output == Format::Table {
                eprintln!("control plane: {} · {} outdated", v["target"].as_str().unwrap_or("?"), v["outdated"]);
                out::result(
                    g.output,
                    &nodes,
                    Some(&[
                        ("NODE", "/hostname"),
                        ("STATUS", "/status"),
                        ("OS", "/os"),
                        ("AGENT", "/agent_version"),
                        ("OUTDATED", "/outdated"),
                    ]),
                );
            } else {
                out::result(g.output, &v, None);
            }
        }
        AgentsCmd::Update { nodes, all, canary, wait, wait_timeout_secs, yes } => {
            if nodes.is_empty() && !all {
                return Err(CliError::usage("name the nodes to update, or pass --all"));
            }
            let mut ids = Vec::new();
            for n in &nodes {
                ids.push(resolve_node(&mut api, n).await?["id"].as_str().unwrap_or_default().to_owned());
            }
            let what = if all { "every outdated online Linux node".to_owned() } else { nodes.join(", ") };
            let how = canary.map(|n| format!(" in batches of {n} (canary first)")).unwrap_or_default();
            confirm(
                g,
                yes,
                &format!(
                    "Update the agent on {what}{how} at {}? Nodes restart their agent (rollback is automatic).",
                    t.server
                ),
            )?;
            if let Some(batch) = canary {
                let r = api.post("/api/v1/agents/rollouts", &json!({ "server_ids": ids, "batch": batch })).await?;
                let job = r["job"]["id"].as_str().unwrap_or_default().to_owned();
                if !wait {
                    out::result(g.output, &r, None);
                    return Ok(());
                }
                let done = crate::cmds::wait_jobs(
                    &mut api,
                    std::slice::from_ref(&job),
                    Duration::from_secs(wait_timeout_secs),
                )
                .await?;
                out::result(g.output, &json!({ "target": r["target"], "rollout": done.first() }), None);
                return Ok(());
            }
            let r = api.post("/api/v1/agents/update", &json!({ "server_ids": ids })).await?;
            if !wait {
                out::result(g.output, &r, None);
                return Ok(());
            }
            let jobs: Vec<String> = r["jobs"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|j| j["job_id"].as_str().map(str::to_owned))
                .collect();
            let deadline = Instant::now() + Duration::from_secs(wait_timeout_secs);
            let mut results = Vec::new();
            for id in &jobs {
                loop {
                    let j = api.get(&format!("/api/v1/jobs/{id}")).await?;
                    let s = j["status"].as_str().unwrap_or_default().to_owned();
                    if matches!(s.as_str(), "succeeded" | "failed" | "cancelled") {
                        results.push(json!({ "job_id": id, "status": s, "error": j["last_error"] }));
                        break;
                    }
                    if Instant::now() >= deadline {
                        return Err(CliError::new(Exit::Timeout, format!("job {id} is still {s}; it keeps running")));
                    }
                    tokio::time::sleep(Duration::from_secs(3)).await;
                }
            }
            let failed = results.iter().filter(|r| r["status"] != "succeeded").count();
            out::result(g.output, &json!({ "target": r["target"], "results": results, "refused": r["refused"] }), None);
            if failed > 0 {
                return Err(CliError::new(Exit::JobFailed, format!("{failed} update(s) did not succeed"))
                    .with_code("partial_failure"));
            }
        }
    }
    Ok(())
}
