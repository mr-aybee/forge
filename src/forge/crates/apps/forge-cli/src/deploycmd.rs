//! `forge deploy …`: releases and deployments of services from a
//! `forge.deploy.yaml` (#73).
//!
//! * `validate`, `plan`, `apply [--wait]` take the document (`-f FILE`, `-`
//!   for stdin);
//! * `status`, `pause`, `resume`, `abort`, `approve`, `reject` act on one
//!   deployment;
//! * `history`, `releases`, `release`, `slots`, `redeploy`, `rollback` act on
//!   a service.
//!
//! In CI: a service account token restricted to `deploy` on the service, then
//! `forge deploy apply -f forge.deploy.yaml --wait`. The exit status is 0 when
//! the release is live and 10 when the rollout failed, was aborted or was
//! rolled back.

use std::io::Read;
use std::time::{Duration, Instant};

use clap::{Args, Subcommand};
use serde_json::{Value, json};

use crate::admincmd::enc;
use crate::error::{CliError, Exit, Result};
use crate::out::{self, Col, Format};
use crate::{Global, connect, target};

#[derive(Args, Debug)]
pub struct DocArgs {
    /// The deployment document (`-` reads stdin).
    #[arg(long, short = 'f', value_name = "FILE", default_value = "forge.deploy.yaml")]
    pub file: String,
    /// Apply to this environment instead of `metadata.environment`; its
    /// `overrides` entry is merged in.
    #[arg(long, value_name = "SLUG")]
    pub environment: Option<String>,
}

#[derive(Args, Debug)]
pub struct WaitArgs {
    /// Follow the rollout until it ends; exit 10 unless the release went live.
    #[arg(long)]
    pub wait: bool,
    /// Longest wait with `--wait`, in minutes.
    #[arg(long, default_value_t = 60, value_name = "MINUTES")]
    pub timeout: u64,
}

#[derive(Args, Debug)]
pub struct ServiceArg {
    /// Service name or id.
    pub service: String,
    /// The environment when the name is not unique.
    #[arg(long, short = 'n')]
    pub environment: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum DeployCmd {
    /// Check a document (schema, artifact, policy) without changing anything.
    Validate(DocArgs),
    /// Show the release, nodes and steps a document would roll out, without starting.
    Plan(DocArgs),
    /// Create a release from the document and roll it out.
    Apply {
        #[command(flatten)]
        doc: DocArgs,
        #[command(flatten)]
        wait: WaitArgs,
    },
    /// One deployment: state, steps and events.
    Status {
        /// Deployment id.
        deployment: String,
        /// Follow it until it ends.
        #[arg(long)]
        follow: bool,
    },
    /// A service's deployments, newest first.
    History {
        #[command(flatten)]
        svc: ServiceArg,
        /// Most entries.
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
    /// A service's releases, newest first.
    Releases {
        #[command(flatten)]
        svc: ServiceArg,
        /// Most entries.
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
    /// One release, with the document it was made from.
    Release {
        #[command(flatten)]
        svc: ServiceArg,
        /// Release number.
        number: i64,
    },
    /// The copies of a service on each node, their slot, release and traffic weight.
    Slots {
        #[command(flatten)]
        svc: ServiceArg,
    },
    /// Roll out an earlier release again (with its own strategy).
    Redeploy {
        #[command(flatten)]
        svc: ServiceArg,
        /// Release number.
        number: i64,
        #[command(flatten)]
        wait: WaitArgs,
    },
    /// Go back to the release before the current one (or `--to`).
    Rollback {
        #[command(flatten)]
        svc: ServiceArg,
        /// Release number to go back to.
        #[arg(long)]
        to: Option<i64>,
        #[command(flatten)]
        wait: WaitArgs,
    },
    /// Hold a running deployment before its next step.
    Pause {
        /// Deployment id.
        deployment: String,
    },
    /// Continue a paused deployment.
    Resume {
        /// Deployment id.
        deployment: String,
    },
    /// Stop a deployment; new copies that never took traffic are removed.
    Abort {
        /// Deployment id.
        deployment: String,
    },
    /// Let a deployment that waits for an approval go on (not your own).
    Approve {
        /// Deployment id.
        deployment: String,
    },
    /// Stop a deployment that waits for an approval (not your own).
    Reject {
        /// Deployment id.
        deployment: String,
    },
}

const DEPLOYMENT_COLS: &[Col] = &[
    ("ID", "/id"),
    ("RELEASE", "/release_number"),
    ("FROM", "/from_release"),
    ("KIND", "/kind"),
    ("STRATEGY", "/strategy"),
    ("STATE", "/state"),
    ("STEP", "/step"),
    ("ERROR", "/error"),
];
const RELEASE_COLS: &[Col] = &[
    ("NUMBER", "/number"),
    ("ARTIFACT", "/artifact_label"),
    ("CREATED", "/created_at"),
    ("DESCRIPTION", "/description"),
];
const SLOT_COLS: &[Col] = &[
    ("NODE", "/server_id"),
    ("SLOT", "/slot"),
    ("RELEASE", "/release_number"),
    ("WEIGHT", "/weight"),
    ("STATE", "/state"),
    ("PORTS", "/ports"),
];

fn read_doc(file: &str) -> Result<String> {
    let mut text = String::new();

    if file == "-" {
        std::io::stdin().read_to_string(&mut text).map_err(|e| CliError::usage(format!("stdin: {e}")))?;
    } else {
        text = std::fs::read_to_string(file).map_err(|e| CliError::usage(format!("{file}: {e}")))?;
    }

    Ok(text)
}

fn body(doc: &DocArgs) -> Result<Value> {
    Ok(json!({ "yaml": read_doc(&doc.file)?, "environment": doc.environment }))
}

async fn service_id(api: &mut crate::api::Api, svc: &ServiceArg) -> Result<String> {
    let app = crate::rescmd::resolve_app(api, &svc.service, svc.environment.as_deref()).await?;
    Ok(app["id"].as_str().unwrap_or_default().to_owned())
}

/// Prints a plan for people: what, where, and each step.
fn print_plan(fmt: Format, plan: &Value) {
    if fmt != Format::Table {
        out::result(fmt, plan, None);
        return;
    }

    let s = |p: &str| plan.pointer(p).and_then(Value::as_str).unwrap_or_default().to_owned();
    let new = if plan["creates_service"] == true { " (new service)" } else { "" };
    println!("Service      {}{new} in {}", s("/service"), s("/environment"));
    println!("Release      {}  {}", plan["release"], s("/artifact"));
    println!(
        "Strategy     {}{}",
        s("/strategy"),
        if plan["slotted"] == true { ", side by side" } else { ", in place" }
    );
    let list = |p: &str| {
        plan[p]
            .as_array()
            .map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", "))
            .unwrap_or_default()
    };
    println!("Nodes        {}", list("nodes"));

    if !list("added_nodes").is_empty() {
        println!("Adds         {}", list("added_nodes"));
    }

    if !list("removed_nodes").is_empty() {
        println!("Removes      {}", list("removed_nodes"));
    }

    println!("Downtime     {}", if plan["downtime"] == true { "yes" } else { "no" });
    println!("\nSteps");

    for (i, step) in plan["steps"].as_array().into_iter().flatten().enumerate() {
        println!("  {:>2}. {}", i + 1, step.as_str().unwrap_or_default());
    }

    for n in plan["notes"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        println!("\nNote: {n}");
    }
}

fn terminal(state: &str) -> bool {
    matches!(state, "succeeded" | "failed" | "aborted" | "rolled_back")
}

/// Follows a deployment, printing its events, until it ends. Ok when the
/// release went live.
async fn follow(g: &Global, api: &mut crate::api::Api, id: &str, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    let mut after = 0i64;
    let mut last_state = String::new();

    loop {
        let events = api.get(&format!("/api/v1/deployments/{}/events?after={after}", enc(id))).await?;

        for e in events.as_array().into_iter().flatten() {
            after = after.max(e["seq"].as_i64().unwrap_or(after));

            if g.output == Format::Table {
                let level = e["level"].as_str().unwrap_or("info");
                let mark = match level {
                    "error" => "✗",
                    "warn" => "!",
                    _ => "·",
                };

                eprintln!("{mark} {}", e["message"].as_str().unwrap_or_default());
            }
        }

        let d = api.get(&format!("/api/v1/deployments/{}", enc(id))).await?;
        let state = d["state"].as_str().unwrap_or_default().to_owned();
        if state != last_state && state == "awaiting_approval" && g.output == Format::Table {
            eprintln!("! waiting for approval: forge deploy approve {id}");
        }

        last_state.clone_from(&state);

        if terminal(&state) {
            let summary = json!({
                "id": d["id"], "service": d["service"], "release_number": d["release_number"],
                "state": d["state"], "error": d["error"],
            });
            out::result(g.output, &summary, Some(DEPLOYMENT_COLS));

            if state == "succeeded" {
                return Ok(());
            }

            let why = d["error"].as_str().unwrap_or(&state).to_owned();

            return Err(CliError::new(Exit::JobFailed, format!("the deployment ended {state}: {why}")));
        }

        if Instant::now() >= deadline {
            return Err(CliError::new(
                Exit::Timeout,
                format!("the deployment is still {state}; follow it with `forge deploy status {id} --follow`"),
            ));
        }

        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

/// After a start: the plan, then either the deployment or the rollout.
async fn started(g: &Global, api: &mut crate::api::Api, v: &Value, wait: &WaitArgs) -> Result<()> {
    let id = v.pointer("/deployment/id").and_then(Value::as_str).unwrap_or_default().to_owned();

    if !wait.wait {
        out::result(g.output, &v["deployment"], Some(DEPLOYMENT_COLS));
        return Ok(());
    }

    if g.output == Format::Table {
        print_plan(g.output, &v["plan"]);
        eprintln!("\nDeployment {id}");
    }

    follow(g, api, &id, Duration::from_secs(wait.timeout.max(1) * 60)).await
}

pub async fn run(g: &Global, cmd: DeployCmd) -> Result<()> {
    let t = target(g)?;
    let mut api = connect(g, &t).await?;

    match cmd {
        DeployCmd::Validate(doc) => {
            let v = api.post("/api/v1/deployments/validate", &body(&doc)?).await?;
            out::result(g.output, &v, None);
            Ok(())
        }
        DeployCmd::Plan(doc) => {
            let v = api.post("/api/v1/deployments/plan", &body(&doc)?).await?;
            print_plan(g.output, &v);
            Ok(())
        }
        DeployCmd::Apply { doc, wait } => {
            let v = api.post("/api/v1/deployments", &body(&doc)?).await?;
            started(g, &mut api, &v, &wait).await
        }
        DeployCmd::Status { deployment, follow: f } => {
            if f {
                return follow(g, &mut api, &deployment, Duration::from_secs(24 * 3600)).await;
            }

            let v = api.get(&format!("/api/v1/deployments/{}", enc(&deployment))).await?;

            if g.output != Format::Table {
                out::result(g.output, &v, None);
                return Ok(());
            }

            out::result(g.output, &v, Some(DEPLOYMENT_COLS));
            println!("\nSteps");
            let done = v["step"].as_i64().unwrap_or(0);

            for (i, s) in v["steps"].as_array().into_iter().flatten().enumerate() {
                let mark = if (i as i64) < done { "✓" } else { " " };
                println!("  {mark} {:>2}. {}", i + 1, s.as_str().unwrap_or_default());
            }

            println!("\nEvents");

            for e in v["events"].as_array().into_iter().flatten() {
                println!("  {} {}", e["level"].as_str().unwrap_or_default(), e["message"].as_str().unwrap_or_default());
            }

            Ok(())
        }
        DeployCmd::History { svc, limit } => {
            let id = service_id(&mut api, &svc).await?;
            let v = api.get(&format!("/api/v1/services/{}/deployments?limit={limit}", enc(&id))).await?;
            out::result(g.output, &v, Some(DEPLOYMENT_COLS));
            Ok(())
        }
        DeployCmd::Releases { svc, limit } => {
            let id = service_id(&mut api, &svc).await?;
            let mut v = api.get(&format!("/api/v1/services/{}/releases?limit={limit}", enc(&id))).await?;

            for r in v.as_array_mut().into_iter().flatten() {
                r["artifact_label"] = json!(artifact_label(&r["artifact"]));
            }

            out::result(g.output, &v, Some(RELEASE_COLS));
            Ok(())
        }
        DeployCmd::Release { svc, number } => {
            let id = service_id(&mut api, &svc).await?;
            let v = api.get(&format!("/api/v1/services/{}/releases/{number}", enc(&id))).await?;

            if g.output == Format::Table {
                println!("Release {} — {}", v["number"], artifact_label(&v["artifact"]));

                if let Some(src) = v["source"].as_str() {
                    println!("\n{src}");
                }

                return Ok(());
            }

            out::result(g.output, &v, None);
            Ok(())
        }
        DeployCmd::Slots { svc } => {
            let id = service_id(&mut api, &svc).await?;
            let v = api.get(&format!("/api/v1/services/{}/slots", enc(&id))).await?;

            if g.output == Format::Table {
                println!("Current release {}", v["current_release"]);
                out::result(g.output, &v["slots"], Some(SLOT_COLS));

                return Ok(());
            }

            out::result(g.output, &v, None);
            Ok(())
        }
        DeployCmd::Redeploy { svc, number, wait } => {
            let id = service_id(&mut api, &svc).await?;
            let v =
                api.post(&format!("/api/v1/services/{}/deployments", enc(&id)), &json!({ "release": number })).await?;
            started(g, &mut api, &v, &wait).await
        }
        DeployCmd::Rollback { svc, to, wait } => {
            let id = service_id(&mut api, &svc).await?;
            let v = api.post(&format!("/api/v1/services/{}/rollback", enc(&id)), &json!({ "release": to })).await?;
            started(g, &mut api, &v, &wait).await
        }
        DeployCmd::Pause { deployment } => request(g, &mut api, &deployment, "pause").await,
        DeployCmd::Resume { deployment } => request(g, &mut api, &deployment, "resume").await,
        DeployCmd::Abort { deployment } => request(g, &mut api, &deployment, "abort").await,
        DeployCmd::Approve { deployment } => request(g, &mut api, &deployment, "approve").await,
        DeployCmd::Reject { deployment } => request(g, &mut api, &deployment, "reject").await,
    }
}

async fn request(g: &Global, api: &mut crate::api::Api, id: &str, what: &str) -> Result<()> {
    let v = api.post(&format!("/api/v1/deployments/{}/{what}", enc(id)), &json!({})).await?;
    out::result(g.output, &v, Some(DEPLOYMENT_COLS));

    Ok(())
}

/// One line for an artifact: image (with a short digest), git commit or archive.
fn artifact_label(a: &Value) -> String {
    let s = |k: &str| a[k].as_str().unwrap_or_default();
    match s("kind") {
        "image" => match a["digest"].as_str() {
            Some(d) => format!("{}@{}", s("image"), &d[..d.len().min(19)]),
            None => s("image").to_owned(),
        },
        "git" => format!("{}#{}", s("repo"), &s("commit")[..s("commit").len().min(12)]),
        "archive" => s("url").to_owned(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifacts_read_in_one_line() {
        let d = format!("sha256:{}", "a".repeat(64));
        assert_eq!(
            artifact_label(&json!({ "kind": "image", "image": "ghcr.io/x/web:1", "digest": d })),
            "ghcr.io/x/web:1@sha256:aaaaaaaaaaaa"
        );
        assert_eq!(
            artifact_label(&json!({ "kind": "git", "repo": "https://g/x.git", "commit": "b".repeat(40) })),
            "https://g/x.git#bbbbbbbbbbbb"
        );
        assert!(terminal("rolled_back") && !terminal("awaiting_approval"));
    }
}
