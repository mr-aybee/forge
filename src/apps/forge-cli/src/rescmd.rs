//! `forge stacks` and `forge services`: compose stacks (#7) and services,
//! through the same API as the console.

use std::time::Duration;

use clap::Subcommand;
use serde_json::{Value, json};

use crate::api::Api;
use crate::appspec::{self, AppFlags, Changes, Role};
use crate::cmds::{confirm, resolve_node, wait_jobs};
use crate::error::{CliError, Exit, Result};
use crate::out::{self, Col};
use crate::{Global, connect, target};

fn show(g: &Global, v: &Value, cols: Option<&[Col]>) -> Result<()> {
    out::result(g.output, v, cols);
    Ok(())
}

fn enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

fn looks_like_id(s: &str) -> bool {
    s.len() == 36
        && s.bytes().filter(|&b| b == b'-').count() == 4
        && s.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
}

/// An environment id from an id or a name.
pub(crate) async fn resolve_environment(api: &mut Api, ns: &str) -> Result<String> {
    if looks_like_id(ns) {
        return Ok(ns.to_owned());
    }

    let all = api.get("/api/v1/environments").await?;
    let m: Vec<&Value> = all.as_array().into_iter().flatten().filter(|n| n["name"].as_str() == Some(ns)).collect();
    match m.as_slice() {
        [one] => Ok(one["id"].as_str().unwrap_or_default().to_owned()),
        [] => Err(CliError::new(Exit::NotFound, format!("no environment named `{ns}`"))),
        _ => Err(CliError::usage(format!("`{ns}` is ambiguous; use the environment id")).with_code("ambiguous")),
    }
}

// ------------------------------------------------------------------ stacks

const STACK_COLS: &[Col] = &[
    ("ID", "/id"),
    ("NAME", "/name"),
    ("KIND", "/kind"),
    ("REVISION", "/revision"),
    ("DEPLOYED", "/deployed_revision"),
    ("NODES", "/node_count"),
    ("STATUS", "/node_status"),
];

#[derive(Subcommand, Debug)]
pub enum StacksCmd {
    #[command(flatten)]
    Templates(crate::containercmd::TemplatesCmd),
    /// Compose stacks you can see.
    List {
        /// Only this environment (id or name).
        #[arg(long)]
        environment: Option<String>,
        /// Only stacks with services on this node (id, hostname or display name).
        #[arg(long)]
        node: Option<String>,
        #[command(flatten)]
        paging: out::Paging,
    },
    /// One stack: its compose file, policy preview and service state per node.
    #[command(alias = "get")]
    Show {
        stack: String,
        #[arg(long)]
        environment: Option<String>,
    },
    /// Saved revisions of a stack.
    Revisions {
        stack: String,
        #[arg(long)]
        environment: Option<String>,
    },
    /// Deploy the latest (or a given) revision on its nodes.
    Deploy {
        stack: String,
        #[arg(long)]
        environment: Option<String>,
        /// Revision to deploy; default: the latest.
        #[arg(long)]
        revision: Option<i64>,
        /// Services to start (comma-separated; their dependencies too); default: all.
        #[arg(long, value_delimiter = ',')]
        services: Vec<String>,
        /// Pull images again even when present.
        #[arg(long)]
        pull: bool,
        /// Wait for the deploy jobs (exit 10 if one fails, 8 on timeout).
        #[arg(long)]
        wait: bool,
        #[arg(long, default_value_t = 1800)]
        wait_timeout_secs: u64,
        #[arg(long)]
        yes: bool,
    },
    /// Deploy an older revision (default: the one before the deployed one).
    Rollback {
        stack: String,
        #[arg(long)]
        environment: Option<String>,
        /// Revision to go back to.
        #[arg(long)]
        to: Option<i64>,
        #[arg(long)]
        wait: bool,
        #[arg(long, default_value_t = 1800)]
        wait_timeout_secs: u64,
        #[arg(long)]
        yes: bool,
    },
    /// Stop and remove the stack's containers (files in a folder you chose stay).
    Down {
        stack: String,
        #[arg(long)]
        environment: Option<String>,
        /// Only on this node.
        #[arg(long)]
        node: Option<String>,
        /// Also remove the stack's named volumes: their data is lost.
        #[arg(long)]
        volumes: bool,
        #[arg(long)]
        wait: bool,
        #[arg(long, default_value_t = 1800)]
        wait_timeout_secs: u64,
        #[arg(long)]
        yes: bool,
    },
    /// Start, stop, restart or pull services on one node.
    Service {
        stack: String,
        /// start, stop, restart or pull.
        action: String,
        /// The node (id, hostname or display name).
        #[arg(long)]
        node: String,
        /// Services (comma-separated); default: all on that node.
        #[arg(long, value_delimiter = ',')]
        services: Vec<String>,
        #[arg(long)]
        environment: Option<String>,
        #[arg(long)]
        wait: bool,
    },
    /// The last log lines of one service on one node.
    Logs {
        stack: String,
        service: String,
        #[arg(long)]
        node: String,
        #[arg(long, default_value_t = 200)]
        tail: u32,
        #[arg(long)]
        environment: Option<String>,
    },
    #[command(flatten)]
    Edit(StackEditCmd),
    #[command(flatten)]
    Doc(crate::stackdoccmd::StackDocCmd),
}

/// Creating and editing stacks from compose files (for CI pipelines).
#[derive(Subcommand, Debug)]
pub enum StackEditCmd {
    /// Create a stack from a compose file (`--dry-run` shows the plan only).
    Create {
        /// Stack name (a-z, 0-9, `-`, `_`).
        name: String,
        /// Environment (id or name).
        #[arg(long)]
        environment: String,
        /// Node the stack is created on (services without placement run here).
        #[arg(long)]
        node: String,
        /// Compose file (`-` for stdin).
        #[arg(long, short = 'f')]
        compose: String,
        /// `.env` file for the stack.
        #[arg(long)]
        env_file: Option<std::path::PathBuf>,
        /// Folder on the node (default: Forge's stack folder).
        #[arg(long)]
        dir: Option<String>,
        /// Revision note.
        #[arg(long)]
        note: Option<String>,
        /// Validate and show the placement and policy plan; save nothing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Save a new revision from a compose file and/or `.env` file (deploy it with `deploy`).
    Update {
        stack: String,
        #[arg(long)]
        environment: Option<String>,
        /// New compose file (`-` for stdin).
        #[arg(long, short = 'f')]
        compose: Option<String>,
        /// New `.env` file; omitted keeps the stored variables.
        #[arg(long)]
        env_file: Option<std::path::PathBuf>,
        #[arg(long)]
        note: Option<String>,
    },
    /// Delete a stack that is down on every node.
    Delete {
        stack: String,
        #[arg(long)]
        environment: Option<String>,
        #[arg(long)]
        yes: bool,
    },
    /// Read the service state from every node again.
    Refresh {
        stack: String,
        #[arg(long)]
        environment: Option<String>,
    },
    /// The compose file of one saved revision.
    Revision {
        stack: String,
        revision: i64,
        #[arg(long)]
        environment: Option<String>,
    },
}

pub(crate) fn read_text(spec: &str) -> Result<String> {
    if spec == "-" {
        return Ok(std::io::read_to_string(std::io::stdin())?);
    }

    std::fs::read_to_string(spec).map_err(|e| CliError::usage(format!("{spec}: {e}")))
}

fn read_env(path: Option<&std::path::Path>) -> Result<Option<String>> {
    path.map(|p| std::fs::read_to_string(p).map_err(|e| CliError::usage(format!("{}: {e}", p.display())))).transpose()
}

async fn stack_edit(g: &Global, api: &mut Api, server: &str, cmd: StackEditCmd) -> Result<()> {
    match cmd {
        StackEditCmd::Create { name, environment, node, compose, env_file, dir, note, dry_run } => {
            let mut body = json!({
                "environment_id": resolve_environment(api, &environment).await?,
                "server_id": resolve_node(api, &node).await?["id"].as_str().unwrap_or_default(),
                "name": name,
                "content": read_text(&compose)?,
                "env": read_env(env_file.as_deref())?,
                "dir": dir,
            });

            // The preview takes no revision note (it refuses unknown fields).
            if !dry_run {
                body["note"] = json!(note);
            }

            let path = if dry_run { "/api/v1/stacks/preview" } else { "/api/v1/stacks" };
            show(g, &api.post(path, &body).await?, None)
        }
        StackEditCmd::Update { stack, environment, compose, env_file, note } => {
            if compose.is_none() && env_file.is_none() {
                return Err(CliError::usage("pass --compose and/or --env-file"));
            }

            let s = resolve_stack(api, &stack, environment.as_deref()).await?;
            let id = s["stack"]["id"].as_str().unwrap_or_default().to_owned();
            let body = json!({
                "content": compose.as_deref().map(read_text).transpose()?,
                "env": read_env(env_file.as_deref())?,
                "note": note,
                // The version just read: a save by someone else in between
                // answers 409 `stale_version` (exit 7) instead of being lost.
                "version": s["stack"]["updated_at"],
            });
            let v = api.call(reqwest::Method::PUT, &format!("/api/v1/stacks/{id}"), Some(&body)).await?;
            show(g, &v, None)
        }
        StackEditCmd::Delete { stack, environment, yes } => {
            let s = resolve_stack(api, &stack, environment.as_deref()).await?;
            let id = s["stack"]["id"].as_str().unwrap_or_default().to_owned();
            confirm(g, yes, &format!("Delete stack {} and its revisions on {server}?", s["stack"]["name"]))?;
            show(g, &api.call(reqwest::Method::DELETE, &format!("/api/v1/stacks/{id}"), None).await?, None)
        }
        StackEditCmd::Refresh { stack, environment } => {
            let s = resolve_stack(api, &stack, environment.as_deref()).await?;
            let id = s["stack"]["id"].as_str().unwrap_or_default().to_owned();
            show(g, &api.post(&format!("/api/v1/stacks/{id}/refresh"), &json!({})).await?, None)
        }
        StackEditCmd::Revision { stack, revision, environment } => {
            let s = resolve_stack(api, &stack, environment.as_deref()).await?;
            let id = s["stack"]["id"].as_str().unwrap_or_default().to_owned();
            let v = api.get(&format!("/api/v1/stacks/{id}/revisions/{revision}")).await?;
            match (g.output, v["content"].as_str()) {
                (out::Format::Table, Some(text)) => {
                    print!("{text}");
                    Ok(())
                }
                _ => show(g, &v, None),
            }
        }
    }
}

/// A stack by id, or by name (unique in the environment if one is given).
pub(crate) async fn resolve_stack(api: &mut Api, stack: &str, environment: Option<&str>) -> Result<Value> {
    if looks_like_id(stack) {
        return api.get(&format!("/api/v1/stacks/{stack}")).await;
    }

    let path = match environment {
        Some(ns) => format!("/api/v1/stacks?environment_id={}", resolve_environment(api, ns).await?),
        None => "/api/v1/stacks".to_owned(),
    };

    let all = api.get(&path).await?;
    let m: Vec<&Value> =
        all.as_array().into_iter().flatten().filter(|s| s["stack"]["name"].as_str() == Some(stack)).collect();

    match m.as_slice() {
        [one] => api.get(&format!("/api/v1/stacks/{}", one["stack"]["id"].as_str().unwrap_or_default())).await,
        [] => Err(CliError::new(Exit::NotFound, format!("no stack named `{stack}` that you can see"))),
        many => Err(CliError::usage(format!(
            "`{stack}` is in {} environments; pass --environment or use the id: {}",
            many.len(),
            many.iter().filter_map(|s| s["stack"]["id"].as_str()).collect::<Vec<_>>().join(", ")
        ))
        .with_code("ambiguous")),
    }
}

fn job_ids(v: &Value) -> Vec<String> {
    match v {
        Value::Array(a) => a.iter().filter_map(|j| j["id"].as_str().map(str::to_owned)).collect(),
        Value::Object(_) => v["id"].as_str().map(str::to_owned).into_iter().collect(),
        _ => Vec::new(),
    }
}

/// Shows the jobs a mutation started, or waits for them first.
async fn jobs_result(g: &Global, api: &mut Api, started: Value, wait: bool, timeout: u64) -> Result<()> {
    if !wait {
        return show(g, &started, Some(&[("JOB", "/id"), ("KIND", "/kind"), ("STATUS", "/status")]));
    }

    let done = wait_jobs(api, &job_ids(&started), Duration::from_secs(timeout)).await?;

    show(g, &Value::Array(done), Some(&[("JOB", "/id"), ("KIND", "/kind"), ("STATUS", "/status")]))
}

pub async fn stacks(g: &Global, cmd: StacksCmd) -> Result<()> {
    if let StacksCmd::Templates(c) = cmd {
        return crate::containercmd::templates(g, c).await;
    }

    if let StacksCmd::Doc(c) = cmd {
        return crate::stackdoccmd::run(g, c).await;
    }

    let t = target(g)?;
    let mut api = connect(g, &t).await?;

    match cmd {
        StacksCmd::Templates(_) | StacksCmd::Doc(_) => Ok(()),
        StacksCmd::Edit(e) => stack_edit(g, &mut api, &t.server, e).await,
        StacksCmd::List { environment, node, paging } => {
            let mut q = Vec::new();

            if let Some(ns) = environment {
                q.push(format!("environment_id={}", resolve_environment(&mut api, &ns).await?));
            }

            if let Some(n) = node {
                q.push(format!("server_id={}", resolve_node(&mut api, &n).await?["id"].as_str().unwrap_or_default()));
            }

            let path =
                if q.is_empty() { "/api/v1/stacks".to_owned() } else { format!("/api/v1/stacks?{}", q.join("&")) };
            let v = api.get(&path).await?;
            let rows: Vec<Value> = v
                .as_array()
                .into_iter()
                .flatten()
                .map(|s| {
                    let mut row = s["stack"].clone();
                    let nodes = s["nodes"].as_array().cloned().unwrap_or_default();
                    row["node_count"] = json!(nodes.len());
                    let mut status: Vec<String> =
                        nodes.iter().filter_map(|n| n["status"].as_str().map(str::to_owned)).collect();
                    status.sort();
                    status.dedup();
                    row["node_status"] = json!(if status.is_empty() { "-".to_owned() } else { status.join(",") });
                    row["nodes"] = json!(nodes);
                    row
                })
                .collect();
            out::list(g.output, rows, Some(STACK_COLS), paging.into(), out::Total::Rows);
            Ok(())
        }
        StacksCmd::Show { stack, environment } => {
            let v = resolve_stack(&mut api, &stack, environment.as_deref()).await?;
            show(g, &v, None)
        }
        StacksCmd::Revisions { stack, environment } => {
            let s = resolve_stack(&mut api, &stack, environment.as_deref()).await?;
            let id = s["stack"]["id"].as_str().unwrap_or_default().to_owned();
            let v = api.get(&format!("/api/v1/stacks/{id}/revisions")).await?;
            show(
                g,
                &v,
                Some(&[
                    ("REVISION", "/revision"),
                    ("SHA256", "/sha256"),
                    ("AUTHOR", "/author"),
                    ("AT", "/created_at"),
                    ("NOTE", "/note"),
                ]),
            )
        }
        StacksCmd::Deploy { stack, environment, revision, services, pull, wait, wait_timeout_secs, yes } => {
            let s = resolve_stack(&mut api, &stack, environment.as_deref()).await?;
            let (id, name) = (s["stack"]["id"].as_str().unwrap_or_default().to_owned(), s["stack"]["name"].clone());
            let rev = revision.unwrap_or_else(|| s["stack"]["revision"].as_i64().unwrap_or(1));
            confirm(g, yes, &format!("Deploy revision {rev} of stack {name} on {}?", t.server))?;
            let body = json!({ "revision": revision, "services": services, "pull": pull });
            let started = api.post(&format!("/api/v1/stacks/{id}/deploy"), &body).await?;
            jobs_result(g, &mut api, started, wait, wait_timeout_secs).await
        }
        StacksCmd::Rollback { stack, environment, to, wait, wait_timeout_secs, yes } => {
            let s = resolve_stack(&mut api, &stack, environment.as_deref()).await?;
            let id = s["stack"]["id"].as_str().unwrap_or_default().to_owned();
            let deployed = s["stack"]["deployed_revision"].as_i64().unwrap_or(0);

            let to = match to {
                Some(r) => r,
                None if deployed > 1 => deployed - 1,
                None => {
                    return Err(CliError::usage("no earlier revision is deployed: pass --to <revision>")
                        .with_code("no_previous_revision"));
                }
            };

            confirm(
                g,
                yes,
                &format!(
                    "Roll stack {} back to revision {to} (deployed: {deployed}) on {}?",
                    s["stack"]["name"], t.server
                ),
            )?;
            let started = api.post(&format!("/api/v1/stacks/{id}/deploy"), &json!({ "revision": to })).await?;
            jobs_result(g, &mut api, started, wait, wait_timeout_secs).await
        }
        StacksCmd::Down { stack, environment, node, volumes, wait, wait_timeout_secs, yes } => {
            let s = resolve_stack(&mut api, &stack, environment.as_deref()).await?;
            let id = s["stack"]["id"].as_str().unwrap_or_default().to_owned();

            let server = match &node {
                Some(n) => Some(resolve_node(&mut api, n).await?["id"].as_str().unwrap_or_default().to_owned()),
                None => None,
            };

            let what = format!(
                "Take stack {} down{}{} on {}?",
                s["stack"]["name"],
                node.as_deref().map(|n| format!(" on node {n}")).unwrap_or_default(),
                if volumes { " AND DELETE ITS VOLUMES (data is lost)" } else { "" },
                t.server
            );
            confirm(g, yes, &what)?;
            let started = api
                .post(&format!("/api/v1/stacks/{id}/down"), &json!({ "volumes": volumes, "server_id": server }))
                .await?;
            jobs_result(g, &mut api, started, wait, wait_timeout_secs).await
        }
        StacksCmd::Service { stack, action, node, services, environment, wait } => {
            if !matches!(action.as_str(), "start" | "stop" | "restart" | "pull") {
                return Err(CliError::usage("action: start, stop, restart or pull"));
            }

            let s = resolve_stack(&mut api, &stack, environment.as_deref()).await?;
            let id = s["stack"]["id"].as_str().unwrap_or_default().to_owned();
            let server = resolve_node(&mut api, &node).await?["id"].as_str().unwrap_or_default().to_owned();
            let body = json!({ "server_id": server, "action": action, "services": services });
            let started = api.post(&format!("/api/v1/stacks/{id}/services"), &body).await?;
            jobs_result(g, &mut api, started, wait, 600).await
        }
        StacksCmd::Logs { stack, service, node, tail, environment } => {
            let s = resolve_stack(&mut api, &stack, environment.as_deref()).await?;
            let id = s["stack"]["id"].as_str().unwrap_or_default().to_owned();
            let server = resolve_node(&mut api, &node).await?["id"].as_str().unwrap_or_default().to_owned();
            let v = api
                .get(&format!("/api/v1/stacks/{id}/logs?server_id={server}&service={}&tail={tail}", enc(&service)))
                .await?;

            match (g.output, v["logs"].as_str()) {
                (out::Format::Table, Some(text)) => {
                    print!("{text}");
                    Ok(())
                }
                _ => show(g, &v, None),
            }
        }
    }
}

// ------------------------------------------------------------------ apps

#[derive(Subcommand, Debug)]
pub enum ServicesCmd {
    /// The service on Docker Swarm: status, scale, restart, rollback, logs.
    #[command(subcommand)]
    Swarm(crate::containercmd::ServiceSwarmCmd),
    /// Services you can see.
    List {
        /// Only this environment (id or name).
        #[arg(long)]
        environment: Option<String>,
        /// Only services in this cluster's environments (id or name).
        #[arg(long)]
        cluster: Option<String>,
        #[command(flatten)]
        paging: out::Paging,
    },
    /// One service: a readable summary with its instances per node
    /// (`-o json` for the full record).
    #[command(alias = "get")]
    Show {
        #[arg(id = "service", value_name = "SERVICE")]
        app: String,
        #[arg(long)]
        environment: Option<String>,
    },
    /// The service's health checks: one line per check.
    Health {
        #[arg(id = "service", value_name = "SERVICE")]
        app: String,
        #[arg(long)]
        environment: Option<String>,
    },
    /// Run an action on one instance: start, stop, restart, status or prepare (create the folder, user and app root before the first deploy).
    Action {
        #[arg(id = "service", value_name = "SERVICE")]
        app: String,
        /// start, stop, restart, status or prepare.
        action: String,
        /// The node (id, hostname or display name).
        #[arg(long)]
        node: String,
        #[arg(long)]
        environment: Option<String>,
        #[arg(long)]
        yes: bool,
    },
    /// Deploy the service on its nodes. The server first installs what a
    /// node lacks: the runtimes the definition lists and, for a container,
    /// Docker Engine on a Linux node that has none (needs package.manage, MFA).
    Deploy {
        #[arg(id = "service", value_name = "SERVICE")]
        app: String,
        #[arg(long)]
        environment: Option<String>,
        /// Wait for the deploy jobs (exit 10 if one fails, 8 on timeout).
        #[arg(long)]
        wait: bool,
        #[arg(long, default_value_t = 1800)]
        wait_timeout_secs: u64,
        #[arg(long)]
        yes: bool,
    },
    /// Delete a service that runs on no node.
    Delete {
        #[arg(id = "service", value_name = "SERVICE")]
        app: String,
        #[arg(long)]
        environment: Option<String>,
        #[arg(long)]
        yes: bool,
    },
    /// Create a service from flags, from a JSON definition (`--file app.json`,
    /// `@-` for stdin), or both (flags change the file's fields). Checked
    /// against the server's schema first.
    Create {
        /// The definition as JSON (`@-` reads stdin); optional with flags.
        #[arg(long)]
        file: Option<String>,
        /// The environment (id or name) the service is created in.
        #[arg(long)]
        environment: Option<String>,
        /// Name: lower-case letters, digits and dashes (at most 40).
        #[arg(long)]
        name: Option<String>,
        #[command(flatten)]
        flags: AppFlags,
    },
    /// Change a service's definition: single fields with flags, or all of it
    /// from a JSON file (same shape as create). Deploy to apply it.
    Update {
        #[arg(id = "service", value_name = "SERVICE")]
        app: String,
        /// A full definition as JSON (`@-` reads stdin). It replaces the
        /// stored one; without it the flags change the stored definition.
        #[arg(long)]
        file: Option<String>,
        #[command(flatten)]
        flags: AppFlags,
        /// Remove a variable (`KEY`); repeat for more.
        #[arg(long = "unset-env", value_name = "KEY")]
        unset_env: Vec<String>,
        /// Look the name up in this environment only (id or name).
        #[arg(long)]
        environment: Option<String>,
        #[arg(long)]
        yes: bool,
    },
    /// Set the nodes the service is placed on (repeat --node). The list
    /// replaces the placement: nodes left out are dropped if nothing was
    /// deployed there yet; take a deployed instance off with `forge services remove`.
    Place {
        #[arg(id = "service", value_name = "SERVICE")]
        app: String,
        /// A node (id, hostname or display name), optionally with its role:
        /// `<node>:active` or `<node>:standby`. Repeat for more.
        #[arg(long, required = true, value_name = "NODE[:ROLE]")]
        node: Vec<String>,
        /// The role of nodes given without one: standby instead of active.
        #[arg(long)]
        standby: bool,
        #[arg(long)]
        environment: Option<String>,
        #[arg(long)]
        yes: bool,
    },
    /// Buckets attached to a service: list, attach, detach.
    #[command(subcommand)]
    Storage(AppStorageCmd),
    /// Show an instance's recent log lines; --follow keeps streaming
    /// (Ctrl+C stops).
    Logs {
        #[arg(id = "service", value_name = "SERVICE")]
        app: String,
        /// The node (id, hostname or display name).
        #[arg(long)]
        node: String,
        /// Lines to show (0-5000).
        #[arg(long, default_value_t = 200)]
        tail: u32,
        /// Keep printing new lines.
        #[arg(long)]
        follow: bool,
        #[arg(long)]
        environment: Option<String>,
    },
    /// Take the service off one node (stops it and removes its unit or container).
    Remove {
        #[arg(id = "service", value_name = "SERVICE")]
        app: String,
        /// The node (id, hostname or display name).
        #[arg(long)]
        node: String,
        #[arg(long)]
        environment: Option<String>,
        #[arg(long)]
        yes: bool,
    },
    /// Files in the service's folder on every node it is placed on (or
    /// --node): the app root is looked up per node.
    Files {
        #[arg(id = "service", value_name = "SERVICE")]
        app: String,
        /// Only these nodes (repeat); default: every node the app is placed on.
        #[arg(long)]
        node: Vec<String>,
        #[arg(long)]
        environment: Option<String>,
        #[command(subcommand)]
        op: AppFilesCmd,
    },
}

/// Object storage attached to a service (D-106). An attachment issues an
/// access key limited to one bucket; the service gets the S3 settings as
/// variables with its next deploy.
#[derive(Subcommand, Debug)]
pub enum AppStorageCmd {
    /// The buckets attached to a service and the variables each one adds.
    List {
        #[arg(id = "service", value_name = "SERVICE")]
        app: String,
        /// Look the name up in this environment only (id or name).
        #[arg(long)]
        environment: Option<String>,
    },
    /// Attach a bucket of a managed store: issues a key for that bucket and
    /// injects the S3 settings at the next deploy.
    Attach {
        #[arg(id = "service", value_name = "SERVICE")]
        app: String,
        /// The object store (id, name or slug).
        #[arg(long)]
        store: String,
        /// The bucket to attach.
        #[arg(long)]
        bucket: String,
        /// Create the bucket when it does not exist yet.
        #[arg(long)]
        create_bucket: bool,
        /// What the service may do in the bucket.
        #[arg(long, default_value = "readwrite", value_parser = ["read", "readwrite"])]
        access: String,
        /// Prefix of the injected variables (`<PREFIX>_ENDPOINT`, `_REGION`,
        /// `_BUCKET`, `_ACCESS_KEY_ID`, `_SECRET_ACCESS_KEY`).
        #[arg(long, default_value = "S3")]
        prefix: String,
        /// Look the name up in this environment only (id or name).
        #[arg(long)]
        environment: Option<String>,
    },
    /// Detach a bucket: revokes its access key and removes the variables at
    /// the next deploy.
    Detach {
        #[arg(id = "service", value_name = "SERVICE")]
        app: String,
        /// The attachment's access key (from `forge services storage list`).
        access_key: String,
        /// Look the name up in this environment only (id or name).
        #[arg(long)]
        environment: Option<String>,
        /// Do not ask first (required with --non-interactive).
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum AppFilesCmd {
    /// List a folder of the app root.
    Ls {
        #[arg(default_value = "")]
        path: String,
        /// Include hidden entries.
        #[arg(long, short = 'a')]
        all: bool,
    },
    /// Upload a local file to a path in the app root.
    Upload {
        local: std::path::PathBuf,
        remote: String,
        /// Replace an existing file.
        #[arg(long)]
        overwrite: bool,
    },
    /// Unpack an archive that is already in the app root into a new folder.
    Extract {
        archive: String,
        dest: String,
        /// Replace an existing folder atomically.
        #[arg(long)]
        replace: bool,
    },
    /// Create a folder in the app root.
    Mkdir {
        path: String,
        /// Create missing parent folders too.
        #[arg(long, short = 'p')]
        parents: bool,
    },
}

pub(crate) async fn resolve_app(api: &mut Api, app: &str, environment: Option<&str>) -> Result<Value> {
    if looks_like_id(app) {
        return api.get(&format!("/api/v1/services/{app}")).await;
    }

    let path = match environment {
        Some(ns) => format!("/api/v1/services?environment_id={}", resolve_environment(api, ns).await?),
        None => "/api/v1/services".to_owned(),
    };

    let all = api.get(&path).await?;
    let m: Vec<&Value> = all.as_array().into_iter().flatten().filter(|a| a["name"].as_str() == Some(app)).collect();
    match m.as_slice() {
        [one] => api.get(&format!("/api/v1/services/{}", one["id"].as_str().unwrap_or_default())).await,
        [] => Err(CliError::new(Exit::NotFound, format!("no service named `{app}` that you can see"))),
        many => Err(CliError::usage(format!(
            "`{app}` matches {} services; pass --environment or use the id: {}",
            many.len(),
            many.iter().filter_map(|a| a["id"].as_str()).collect::<Vec<_>>().join(", ")
        ))
        .with_code("ambiguous")),
    }
}

pub async fn services(g: &Global, cmd: ServicesCmd) -> Result<()> {
    if let ServicesCmd::Swarm(c) = cmd {
        return crate::containercmd::service_swarm(g, c).await;
    }

    let t = target(g)?;
    let mut api = connect(g, &t).await?;

    match cmd {
        ServicesCmd::Swarm(_) => Ok(()),
        ServicesCmd::List { environment, cluster, paging } => {
            let mut q = Vec::new();

            if let Some(ns) = environment {
                q.push(format!("environment_id={}", resolve_environment(&mut api, &ns).await?));
            }

            if let Some(c) = cluster {
                q.push(format!("cluster_id={}", crate::admincmd::cluster_id(&mut api, &c).await?));
            }

            let path =
                if q.is_empty() { "/api/v1/services".to_owned() } else { format!("/api/v1/services?{}", q.join("&")) };
            let mut v = api.get(&path).await?;
            let ns_names = environment_names(&mut api).await;

            for a in v.as_array_mut().into_iter().flatten() {
                let inst = a["instances"].as_array().cloned().unwrap_or_default();
                a["instances_summary"] = json!(format!("{}/{}", running_count(&inst), inst.len()));
                let ns = a["environment_id"].as_str().unwrap_or_default().to_owned();
                a["environment"] = json!(ns_names.get(&ns).cloned().unwrap_or(ns));
            }

            out::list_value(
                g.output,
                v,
                "",
                Some(&[
                    ("ID", "/id"),
                    ("NAME", "/name"),
                    ("RUNTIME", "/runtime"),
                    ("ENVIRONMENT", "/environment"),
                    ("RUNNING", "/instances_summary"),
                ]),
                paging,
            );
            Ok(())
        }
        ServicesCmd::Show { app, environment } => {
            let v = resolve_app(&mut api, &app, environment.as_deref()).await?;

            if g.output.is_machine() {
                return show(g, &v, None);
            }

            let names = crate::admincmd::node_names(&mut api).await;
            let ns_names = environment_names(&mut api).await;
            show(g, &app_summary(&v, &names, &ns_names), None)
        }
        ServicesCmd::Create { file, environment, name, flags } => {
            if file.is_none() && !flags.any() {
                return Err(CliError::usage(
                    "pass --file, or --environment, --name and --runtime with the fields the service needs",
                ));
            }

            let secrets = secrets_for(&flags, file.as_deref())?;
            let base = file.as_deref().map(crate::admincmd::read_json).transpose()?;

            let ns = match &environment {
                Some(ns) => Some(resolve_environment(&mut api, ns).await?),
                None => None,
            };

            let changes = Changes { flags: &flags, secrets, unset_env: &[] };
            let body = appspec::create_body(base, ns, name.as_deref(), changes)?;
            crate::admincmd::check_body(&mut api, "POST", "/api/v1/services", &body).await?;
            show(g, &api.post("/api/v1/services", &body).await?, None)
        }
        ServicesCmd::Update { app, file, flags, unset_env, environment, yes } => {
            if file.is_none() && !flags.any() && unset_env.is_empty() {
                return Err(CliError::usage("pass --file, or the flags of the fields to change"));
            }

            let secrets = secrets_for(&flags, file.as_deref())?;
            let a = resolve_app(&mut api, &app, environment.as_deref()).await?;
            let id = service_id(&a);
            let path = format!("/api/v1/services/{id}");

            // A file replaces the stored definition; without one the flags
            // change the definition that was just read.
            let mut body = match &file {
                Some(f) => crate::admincmd::read_json(f)?,
                None => appspec::definition(&a),
            };

            appspec::apply(&mut body, Changes { flags: &flags, secrets, unset_env: &unset_env })?;
            crate::admincmd::check_body(&mut api, "PUT", &path, &body).await?;
            let what = if file.is_some() { "Replace" } else { "Change" };
            confirm(g, yes, &format!("{what} the definition of service {app} ({})? Deploy to apply it.", t.server))?;
            show(g, &api.call(reqwest::Method::PUT, &path, Some(&body)).await?, None)
        }
        ServicesCmd::Storage(op) => app_storage(g, &mut api, &t.server, op).await,
        ServicesCmd::Place { app, node, standby, environment, yes } => {
            let default = if standby { Role::Standby } else { Role::Active };
            let wanted: Vec<(String, Role)> =
                node.iter().map(|n| appspec::parse_placement(n, default)).collect::<Result<_>>()?;
            let a = resolve_app(&mut api, &app, environment.as_deref()).await?;
            let id = service_id(&a);
            let mut placements = Vec::new();

            for (n, role) in &wanted {
                let sid = resolve_node(&mut api, n).await?["id"].as_str().unwrap_or_default().to_owned();
                placements.push((sid, *role));
            }

            let listed: Vec<String> = wanted.iter().map(|(n, r)| format!("{n} ({})", r.as_str())).collect();
            confirm(g, yes, &format!("Place service {app} on {} ({})?", listed.join(", "), t.server))?;
            let v = api
                .call(
                    reqwest::Method::PUT,
                    &format!("/api/v1/services/{id}/instances"),
                    Some(&appspec::placement_body(&placements)),
                )
                .await?;

            if g.output.is_machine() {
                return show(g, &v, None);
            }

            let names = crate::admincmd::node_names(&mut api).await;
            show(g, &Value::Array(instance_rows(&v, &names)), Some(INSTANCE_COLS))
        }
        ServicesCmd::Logs { app, node, tail, follow, environment } => {
            let a = resolve_app(&mut api, &app, environment.as_deref()).await?;
            let id = service_id(&a);
            let server = resolve_node(&mut api, &node).await?["id"].as_str().unwrap_or_default().to_owned();
            let path = format!("/api/v1/services/{id}/instances/{server}/actions");

            if follow {
                return follow_logs(g, &mut api, &path, tail).await;
            }

            let v = api.post(&path, &json!({ "action": "logs", "tail": tail.min(5000) })).await?;
            if v["status"] == "failed" {
                return Err(CliError::new(
                    Exit::JobFailed,
                    format!("logs of {app} on {node}: {}", v["error"].as_str().unwrap_or("failed")),
                )
                .with_code("action_failed"));
            }

            if g.output.is_machine() {
                return show(g, &v, None);
            }

            let text = log_text(&v["result"]);
            out::emit(&if text.is_empty() || text.ends_with('\n') { text } else { format!("{text}\n") });
            Ok(())
        }
        ServicesCmd::Remove { app, node, environment, yes } => {
            let a = resolve_app(&mut api, &app, environment.as_deref()).await?;
            let id = service_id(&a);
            let server = resolve_node(&mut api, &node).await?["id"].as_str().unwrap_or_default().to_owned();
            confirm(
                g,
                yes,
                &format!(
                    "Remove {app} from {node} ({})? It stops there and its unit or container is removed.",
                    t.server
                ),
            )?;
            let v = api
                .post(&format!("/api/v1/services/{id}/instances/{server}/actions"), &json!({ "action": "remove" }))
                .await?;

            if v["status"] == "failed" {
                if !g.output.is_machine() {
                    show(g, &v, None)?;
                }

                return Err(CliError::new(
                    Exit::JobFailed,
                    format!("remove of {app} on {node} failed: {}", v["error"].as_str().unwrap_or("-")),
                )
                .with_code("action_failed"));
            }

            show(g, &v, None)
        }
        ServicesCmd::Files { app, node, environment, op } => {
            let a = resolve_app(&mut api, &app, environment.as_deref()).await?;
            app_files(g, &mut api, &a, &node, op).await
        }
        ServicesCmd::Health { app, environment } => {
            let a = resolve_app(&mut api, &app, environment.as_deref()).await?;
            let id = a["id"].as_str().unwrap_or_default().to_owned();
            let v = api.get(&format!("/api/v1/services/{id}/health")).await?;

            if g.output.is_machine() {
                return show(g, &v, None);
            }

            let rows: Vec<Value> = v["summaries"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|s| {
                    json!({
                        "purpose": s["purpose"],
                        "name": s["name"],
                        "enabled": if s["enabled"] == true { "yes" } else { "no" },
                        "summary": s["summary"],
                    })
                })
                .collect();
            show(
                g,
                &Value::Array(rows),
                Some(&[("PURPOSE", "/purpose"), ("NAME", "/name"), ("ENABLED", "/enabled"), ("SUMMARY", "/summary")]),
            )
        }
        ServicesCmd::Action { app, action, node, environment, yes } => {
            if !matches!(action.as_str(), "start" | "stop" | "restart" | "status" | "prepare") {
                return Err(CliError::usage("action: start, stop, restart, status or prepare"));
            }

            let a = resolve_app(&mut api, &app, environment.as_deref()).await?;
            let id = a["id"].as_str().or_else(|| a["app"]["id"].as_str()).unwrap_or_default().to_owned();
            let server = resolve_node(&mut api, &node).await?["id"].as_str().unwrap_or_default().to_owned();

            if action != "status" {
                confirm(g, yes, &format!("{action} {app} on {node} ({})?", t.server))?;
            }

            let v = api
                .post(&format!("/api/v1/services/{id}/instances/{server}/actions"), &json!({ "action": action }))
                .await?;

            // A node-side failure is not a success (exit 10, like a failed job).
            if v["status"] == "failed" {
                let why = v["error"].as_str().or_else(|| v["last_error"].as_str()).unwrap_or("-");

                if g.output == out::Format::Table {
                    show(g, &v, None)?;
                }

                return Err(CliError::new(Exit::JobFailed, format!("{action} of {app} on {node} failed: {why}"))
                    .with_code("action_failed"));
            }

            show(g, &v, None)
        }
        ServicesCmd::Deploy { app, environment, wait, wait_timeout_secs, yes } => {
            let a = resolve_app(&mut api, &app, environment.as_deref()).await?;
            let id = a["id"].as_str().or_else(|| a["app"]["id"].as_str()).unwrap_or_default().to_owned();
            confirm(g, yes, &format!("Deploy service {app} on its nodes ({})?", t.server))?;
            let started = api.post(&format!("/api/v1/services/{id}/deploy"), &json!({})).await?;
            if let Some(note) = install_note(&started).filter(|_| !g.output.is_machine()) {
                eprintln!("{note}");
            }

            jobs_result(g, &mut api, started, wait, wait_timeout_secs).await
        }
        ServicesCmd::Delete { app, environment, yes } => {
            let a = resolve_app(&mut api, &app, environment.as_deref()).await?;
            let id = a["id"].as_str().or_else(|| a["app"]["id"].as_str()).unwrap_or_default().to_owned();
            confirm(g, yes, &format!("Delete service {app} ({})?", t.server))?;
            show(g, &api.call(reqwest::Method::DELETE, &format!("/api/v1/services/{id}"), None).await?, None)
        }
    }
}

fn service_id(a: &Value) -> String {
    a["id"].as_str().or_else(|| a["app"]["id"].as_str()).unwrap_or_default().to_owned()
}

/// Instances that run now. `state` is what the node last reported (`active`
/// for a systemd unit or a running container); `status` is Forge's
/// lifecycle (`pending`, `deployed`, `failed`, …) and says nothing about it.
fn running_count(instances: &[Value]) -> usize {
    instances.iter().filter(|i| matches!(i["state"].as_str(), Some("running" | "active"))).count()
}

/// What a deploy installs first, from the jobs it started: one line naming
/// the packages, or `None` when every node had what the service needs.
fn install_note(started: &Value) -> Option<String> {
    let installs: Vec<&Value> =
        started.as_array().into_iter().flatten().filter(|j| j["kind"] == "package.action").collect();

    if installs.is_empty() {
        return None;
    }

    let mut names: Vec<&str> = installs.iter().filter_map(|j| j["payload"]["package"].as_str()).collect();
    names.sort_unstable();
    names.dedup();

    Some(format!(
        "Installing what the nodes lack first ({}): {} install job(s). The deploy jobs wait for them.",
        if names.is_empty() { "runtimes".to_owned() } else { names.join(", ") },
        installs.len()
    ))
}

/// Secret values of `--secret-env`. A bare `KEY` reads stdin, which
/// `--file @-` already uses for the definition.
fn secrets_for(flags: &AppFlags, file: Option<&str>) -> Result<Vec<(String, String)>> {
    use std::io::IsTerminal as _;
    let specs = appspec::parse_secret_env(&flags.secret_env)?;

    appspec::read_secrets(&specs, || {
        if file == Some("@-") {
            return Err(CliError::usage(
                "--file @- and --secret-env KEY both read stdin; give the secret as `KEY=@<file>`",
            ));
        }

        if std::io::stdin().is_terminal() {
            return Err(CliError::usage(
                "pipe the secret value on stdin (it is never taken from a flag), or pass `--secret-env KEY=@<file>`",
            ));
        }

        Ok(std::io::read_to_string(std::io::stdin())?)
    })
}

const ATTACHMENT_COLS: &[Col] = &[
    ("ACCESS KEY", "/access_key"),
    ("STORE", "/store"),
    ("BUCKET", "/bucket"),
    ("ACCESS", "/access"),
    ("PREFIX", "/prefix"),
    ("ENDPOINT", "/endpoint"),
];

/// The attachments of a storage answer as table rows (`store`: the store's
/// name, or its id when the store is gone).
fn attachment_rows(v: &Value) -> Value {
    let rows: Vec<Value> = v["attachments"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|a| {
            let mut r = a.clone();
            r["store"] = if a["store_name"].is_string() { a["store_name"].clone() } else { a["store_id"].clone() };
            r
        })
        .collect();
    Value::Array(rows)
}

/// The body of `POST /services/{id}/storage`.
fn attach_body(store_id: &str, bucket: &str, create_bucket: bool, access: &str, prefix: &str) -> Value {
    json!({ "store_id": store_id, "bucket": bucket, "create_bucket": create_bucket, "access": access, "prefix": prefix })
}

fn show_attachments(g: &Global, v: &Value) -> Result<()> {
    if g.output.is_machine() {
        return show(g, v, None);
    }

    show(g, &attachment_rows(v), Some(ATTACHMENT_COLS))
}

/// `forge services storage`: the buckets attached to a service.
async fn app_storage(g: &Global, api: &mut Api, server: &str, op: AppStorageCmd) -> Result<()> {
    match op {
        AppStorageCmd::List { app, environment } => {
            let a = resolve_app(api, &app, environment.as_deref()).await?;
            let v = api.get(&format!("/api/v1/services/{}/storage", service_id(&a))).await?;
            show_attachments(g, &v)
        }
        AppStorageCmd::Attach { app, store, bucket, create_bucket, access, prefix, environment } => {
            let a = resolve_app(api, &app, environment.as_deref()).await?;
            let sid = crate::admincmd::store_id(api, &store).await?;
            let body = attach_body(&sid, &bucket, create_bucket, &access, &prefix);
            let v = api.post(&format!("/api/v1/services/{}/storage", service_id(&a)), &body).await?;

            if !g.output.is_machine() {
                eprintln!(
                    "Bucket {bucket} of {store} is attached to {app} ({access}). Deploy {app} to give it the {prefix}_* variables."
                );
            }

            show_attachments(g, &v)
        }
        AppStorageCmd::Detach { app, access_key, environment, yes } => {
            let a = resolve_app(api, &app, environment.as_deref()).await?;
            confirm(
                g,
                yes,
                &format!(
                    "Detach the bucket of key {access_key} from {app} ({server})? The key is revoked at once; the variables go with the next deploy."
                ),
            )?;
            let path = format!("/api/v1/services/{}/storage/{}", service_id(&a), enc(&access_key));
            let v = api.call(reqwest::Method::DELETE, &path, None).await?;
            show_attachments(g, &v)
        }
    }
}

/// Environment id → name, for display (best effort).
async fn environment_names(api: &mut Api) -> std::collections::HashMap<String, String> {
    let all = api.get("/api/v1/environments").await.unwrap_or(Value::Null);
    all.as_array()
        .into_iter()
        .flatten()
        .filter_map(|n| Some((n["id"].as_str()?.to_owned(), n["name"].as_str()?.to_owned())))
        .collect()
}

const INSTANCE_COLS: &[Col] =
    &[("NODE", "/node"), ("ROLE", "/role"), ("STATUS", "/status"), ("STATE", "/state"), ("ERROR", "/last_error")];

/// Instances with node names instead of server ids.
fn instance_rows(v: &Value, names: &std::collections::HashMap<String, String>) -> Vec<Value> {
    v.as_array()
        .into_iter()
        .flatten()
        .map(|i| {
            let sid = i["server_id"].as_str().unwrap_or_default();
            json!({
                "node": names.get(sid).cloned().unwrap_or_else(|| sid.to_owned()),
                "role": i["role"],
                "status": i["status"],
                "state": i["state"],
                "last_error": i["last_error"],
            })
        })
        .collect()
}

/// Spec fields that only containers use (the server refuses them for
/// services and jobs).
const CONTAINER_ONLY: &[&str] = &[
    "image",
    "entrypoint",
    "volumes",
    "ports",
    "labels",
    "hostname",
    "user",
    "extra_hosts",
    "read_only",
    "healthcheck",
    "log_max_mb",
    "stop_timeout_secs",
];

/// `forge services show` for people: what runs, where it lives on the node and
/// one line per instance with the node's name.
fn app_summary(
    v: &Value,
    names: &std::collections::HashMap<String, String>,
    ns_names: &std::collections::HashMap<String, String>,
) -> Value {
    let runtime = v["runtime"].as_str().unwrap_or_default();
    let name = v["name"].as_str().unwrap_or_default();
    let ns_id = v["environment_id"].as_str().unwrap_or_default();
    let ns = v["spec"]["environment"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .or_else(|| ns_names.get(ns_id).cloned())
        .unwrap_or_else(|| ns_id.to_owned());
    let mut s = serde_json::Map::new();
    s.insert("name".into(), json!(name));
    s.insert("id".into(), v["id"].clone());
    s.insert("runtime".into(), json!(runtime));
    s.insert("environment".into(), json!(ns));

    if runtime != "container" {
        s.insert("unit".into(), json!(format!("forge-app-{ns}-{name}.service")));
        s.insert("folder".into(), json!(forge_catalog::apps::app_home(&ns, name)));
        s.insert(
            "user".into(),
            json!(forge_catalog::apps::app_user_candidates(&ns, name).into_iter().next().unwrap_or_default()),
        );
    }

    let mut spec = serde_json::Map::new();

    for (k, x) in v["spec"].as_object().into_iter().flatten() {
        let hidden = matches!(k.as_str(), "name" | "runtime" | "environment" | "env")
            || (runtime != "container" && CONTAINER_ONLY.contains(&k.as_str()));

        let empty = match x {
            Value::Null => true,
            Value::String(t) => t.is_empty(),
            Value::Array(a) => a.is_empty(),
            Value::Object(o) => o.is_empty(),
            Value::Number(n) => n.as_u64() == Some(0),
            Value::Bool(b) => !b,
        };

        if !hidden && !empty {
            spec.insert(k.clone(), x.clone());
        }
    }

    s.insert("spec".into(), Value::Object(spec));
    let env: Vec<String> = v["env"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|e| {
            let key = e["key"].as_str().unwrap_or("?");

            if e["secret"] == true {
                format!("{key}=(secret)")
            } else {
                format!("{key}={}", e["value"].as_str().unwrap_or_default())
            }
        })
        .collect();
    s.insert("env".into(), json!(env));
    s.insert("instances".into(), Value::Array(instance_rows(&v["instances"], names)));
    s.insert("updated_at".into(), v["updated_at"].clone());

    Value::Object(s)
}

/// Log lines from a `logs` or `follow` result (`{lines}` or text).
fn log_text(r: &Value) -> String {
    match r {
        Value::String(s) => s.clone(),
        Value::Object(_) => r["lines"].as_str().or_else(|| r["output"].as_str()).unwrap_or_default().to_owned(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// `forge services logs --follow`: one `follow` segment after another (each
/// about 50 s on the node), printing the live output and continuing from
/// the cursor each segment returns.
async fn follow_logs(g: &Global, api: &mut Api, path: &str, tail: u32) -> Result<()> {
    use std::io::Write as _;
    let mut cursor = String::new();
    let mut first = true;

    loop {
        let body = if first {
            json!({ "action": "follow", "cursor": cursor, "tail": tail.min(5000) })
        } else {
            json!({ "action": "follow", "cursor": cursor })
        };

        first = false;
        let seg = api.post(path, &body).await?;
        let job = seg["job_id"].as_str().unwrap_or_default().to_owned();
        if job.is_empty() {
            return Err(CliError::new(Exit::General, "the server did not start a follow segment"));
        }

        let mut printed = 0usize;

        loop {
            let live = api.get(&format!("/api/v1/jobs/{job}/live")).await.unwrap_or(Value::Null);
            if let Some(text) = live["output"].as_str()
                && text.len() > printed
            {
                print!("{}", text.get(printed..).unwrap_or_default());
                let _ = std::io::stdout().flush();
                printed = text.len();
            }

            let j = api.get(&format!("/api/v1/jobs/{job}")).await?;
            match j["status"].as_str().unwrap_or_default() {
                "succeeded" => {
                    let o = &j["state"]["output"];

                    // The final result is authoritative: print what the live view missed.
                    let all = log_text(o);
                    if all.len() > printed {
                        print!("{}", all.get(printed..).unwrap_or_default());
                        let _ = std::io::stdout().flush();
                    }

                    cursor = o["cursor"].as_str().unwrap_or(&cursor).to_owned();

                    break;
                }
                "failed" | "cancelled" => {
                    return Err(CliError::new(
                        Exit::JobFailed,
                        format!("following failed: {}", j["last_error"].as_str().unwrap_or("-")),
                    )
                    .with_code("action_failed"));
                }
                _ => tokio::time::sleep(Duration::from_secs(1)).await,
            }
        }

        if g.output.is_machine() {
            // Machine formats get one segment, then stop.
            return Ok(());
        }
    }
}

/// The app root of `app` on node `server` (by service id, else by its folder).
async fn app_root(api: &mut Api, server: &str, app: &Value) -> Result<Option<String>> {
    let v = api.get(&format!("/api/v1/servers/{server}/files/roots")).await?;
    let id = service_id(app);
    let roots: Vec<&Value> = v["roots"].as_array().into_iter().flatten().collect();
    let by_id = roots.iter().find(|r| r["service_id"].as_str() == Some(id.as_str()));
    let suffix = format!("/{}", app["name"].as_str().unwrap_or_default());
    let by_path = || {
        roots.iter().find(|r| {
            r["kind"] == "service"
                && [&r["path"], &r["base"], &r["spec"]["base"]]
                    .iter()
                    .any(|p| p.as_str().is_some_and(|p| p.starts_with("/srv/forge/apps/") && p.ends_with(&suffix)))
        })
    };

    Ok(by_id.or_else(by_path).and_then(|r| r["id"].as_str().map(str::to_owned)))
}

/// `forge services files`: runs the file operation on each node with that
/// node's app root.
async fn app_files(g: &Global, api: &mut Api, app: &Value, nodes: &[String], op: AppFilesCmd) -> Result<()> {
    use crate::filescmd::FilesCmd;
    let name = app["name"].as_str().unwrap_or("?").to_owned();
    let mut targets: Vec<(String, String)> = Vec::new();

    if nodes.is_empty() {
        let names = crate::admincmd::node_names(api).await;

        for i in app["instances"].as_array().into_iter().flatten() {
            let sid = i["server_id"].as_str().unwrap_or_default().to_owned();
            targets.push((names.get(&sid).cloned().unwrap_or_else(|| sid.clone()), sid));
        }
    } else {
        for n in nodes {
            let s = resolve_node(api, n).await?;
            targets.push((
                s["hostname"].as_str().unwrap_or(n).to_owned(),
                s["id"].as_str().unwrap_or_default().to_owned(),
            ));
        }
    }

    if targets.is_empty() {
        return Err(CliError::usage(format!(
            "{name} is placed on no node: run `forge services place {name} --node <node>` first"
        )));
    }

    let mut failed = Vec::new();

    for (label, sid) in &targets {
        let Some(root) = app_root(api, sid, app).await? else {
            failed.push(format!("{label}: no app root for {name} (deploy it there first)"));
            continue;
        };

        if !g.output.is_machine() && targets.len() > 1 {
            eprintln!("== {label}");
        }

        let root = Some(root);
        let node = sid.clone();

        let cmd = match &op {
            AppFilesCmd::Ls { path, all } => {
                FilesCmd::Ls { node, path: path.clone(), root, all: *all, hidden: false, limit: 1000, offset: 0 }
            }
            AppFilesCmd::Upload { local, remote, overwrite } => FilesCmd::Upload {
                node,
                local: local.clone(),
                remote: remote.clone(),
                root,
                overwrite: *overwrite,
                if_version: None,
            },
            AppFilesCmd::Extract { archive, dest, replace } => {
                FilesCmd::Extract { node, archive: archive.clone(), dest: dest.clone(), root, replace: *replace }
            }
            AppFilesCmd::Mkdir { path, parents } => {
                FilesCmd::Mkdir { node, path: path.clone(), root, parents: *parents }
            }
        };

        if let Err(e) = Box::pin(crate::filescmd::run(g, cmd)).await {
            failed.push(format!("{label}: {}", e.message));
        }
    }

    if failed.is_empty() {
        Ok(())
    } else {
        Err(CliError::new(Exit::General, failed.join("; ")).with_code("partial_failure"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_summary_names_nodes_and_hides_container_fields() {
        let v = json!({
            "id": "a1", "name": "xcweb", "runtime": "systemd", "environment_id": "n1",
            "spec": { "command": "python3 -m http.server 8080", "image": "", "volumes": [], "user": "", "listen_port": 8080 },
            "env": [{ "key": "A", "value": "1" }, { "key": "S", "secret": true }],
            "instances": [{ "server_id": "s1", "role": "active", "status": "deployed", "state": "active" }],
        });
        let names = [("s1".to_owned(), "lab-b".to_owned())].into_iter().collect();
        let ns = [("n1".to_owned(), "lab-test".to_owned())].into_iter().collect();
        let s = app_summary(&v, &names, &ns);
        assert_eq!(s["unit"], "forge-app-lab-test-xcweb.service");
        assert_eq!(s["folder"], "/srv/forge/apps/lab-test/xcweb");
        assert_eq!(s["user"], "lab-test-xcweb");
        assert_eq!(s["instances"][0]["node"], "lab-b");
        assert!(s["spec"].get("image").is_none() && s["spec"].get("user").is_none());
        assert_eq!(s["spec"]["listen_port"], 8080);
        assert_eq!(s["env"], json!(["A=1", "S=(secret)"]));
    }

    fn parse(args: &[&str]) -> ServicesCmd {
        use clap::Parser as _;

        match crate::Cli::try_parse_from(args).map(|c| c.cmd) {
            Ok(crate::Cmd::Services(c)) => c,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn create_takes_flags_a_file_or_both() {
        let ServicesCmd::Create { file, environment, name, flags } = parse(&[
            "forge",
            "services",
            "create",
            "--environment",
            "shop-prod",
            "--name",
            "web",
            "--runtime",
            "container",
            "--image",
            "nginx:1.27",
            "--port",
            "8080:80",
            "--port",
            "8443:443",
            "--env",
            "MODE=prod",
            "--secret-env",
            "TOKEN",
            "--volume",
            "/srv/forge/apps/shop-prod/web/html:/usr/share/nginx/html:ro",
            "--restart",
            "false",
        ]) else {
            panic!("create")
        };

        assert_eq!((file, environment.as_deref(), name.as_deref()), (None, Some("shop-prod"), Some("web")));
        assert_eq!(flags.runtime, Some(appspec::Runtime::Container));
        assert_eq!(flags.ports, ["8080:80", "8443:443"]);
        assert_eq!((flags.env.len(), flags.secret_env.len(), flags.volumes.len()), (1, 1, 1));
        assert_eq!(flags.restart, Some(false));
        assert!(flags.any());

        // A file alone still works, and an unknown runtime is refused by the parser.
        let ServicesCmd::Create { file, flags, .. } = parse(&["forge", "services", "create", "--file", "app.json"])
        else {
            panic!("create --file")
        };

        assert!(file.as_deref() == Some("app.json") && !flags.any());
        use clap::Parser as _;
        assert!(crate::Cli::try_parse_from(["forge", "services", "create", "--runtime", "vm"]).is_err());
    }

    #[test]
    fn update_and_place_parse_their_flags() {
        let ServicesCmd::Update { app, file, flags, unset_env, .. } = parse(&[
            "forge",
            "services",
            "update",
            "web",
            "--image",
            "nginx:1.28",
            "--listen-port",
            "8080",
            "--unset-env",
            "OLD",
            "--yes",
        ]) else {
            panic!("update")
        };

        assert_eq!(
            (app.as_str(), file, flags.image.as_deref(), flags.listen_port),
            ("web", None, Some("nginx:1.28"), Some(8080))
        );
        assert_eq!(unset_env, ["OLD"]);

        let ServicesCmd::Place { node, standby, .. } =
            parse(&["forge", "services", "place", "web", "--node", "lab-b:active", "--node", "lab-c:standby", "--yes"])
        else {
            panic!("place")
        };

        assert_eq!((node, standby), (vec!["lab-b:active".to_owned(), "lab-c:standby".to_owned()], false));
    }

    #[test]
    fn storage_commands_parse_and_build_the_attach_request() {
        let ServicesCmd::Storage(AppStorageCmd::Attach { app, store, bucket, create_bucket, access, prefix, .. }) =
            parse(&[
                "forge",
                "services",
                "storage",
                "attach",
                "web",
                "--store",
                "media",
                "--bucket",
                "uploads",
                "--create-bucket",
                "--access",
                "read",
                "--prefix",
                "MEDIA",
            ])
        else {
            panic!("attach")
        };

        assert_eq!(app, "web");
        assert_eq!(
            attach_body(&store, &bucket, create_bucket, &access, &prefix),
            json!({ "store_id": "media", "bucket": "uploads", "create_bucket": true, "access": "read", "prefix": "MEDIA" })
        );

        // Defaults: read-write, the S3 prefix, an existing bucket.
        let ServicesCmd::Storage(AppStorageCmd::Attach { create_bucket, access, prefix, .. }) =
            parse(&["forge", "services", "storage", "attach", "web", "--store", "media", "--bucket", "uploads"])
        else {
            panic!("attach defaults")
        };

        assert_eq!((create_bucket, access.as_str(), prefix.as_str()), (false, "readwrite", "S3"));
        assert!(matches!(
            parse(&["forge", "services", "storage", "detach", "web", "AKIAEXAMPLE", "--yes"]),
            ServicesCmd::Storage(AppStorageCmd::Detach { yes: true, .. })
        ));
        use clap::Parser as _;
        assert!(
            crate::Cli::try_parse_from([
                "forge", "services", "storage", "attach", "web", "--store", "m", "--bucket", "b", "--access", "admin"
            ])
            .is_err()
        );
    }

    #[test]
    fn attachments_show_the_store_name_or_its_id() {
        let v = json!({ "attachments": [
            { "access_key": "AK1", "store_id": "s1", "store_name": "media", "bucket": "uploads", "access": "readwrite", "prefix": "S3" },
            { "access_key": "AK2", "store_id": "s2", "store_name": null, "bucket": "old", "access": "read", "prefix": "OLD" },
        ], "can_change": true });
        let rows = attachment_rows(&v);
        assert_eq!((&rows[0]["store"], &rows[1]["store"]), (&json!("media"), &json!("s2")));
    }

    /// Issue #64: RUNNING counts what the node last reported, never the
    /// lifecycle status, and an instance without a reported state is not running.
    #[test]
    fn running_counts_the_state_the_node_reported() {
        let instances = [
            json!({ "status": "deployed", "state": "active" }),
            json!({ "status": "deployed", "state": "running" }),
            json!({ "status": "deployed", "state": null }),
            json!({ "status": "deployed", "state": "exited" }),
            json!({ "status": "pending" }),
        ];
        assert_eq!(running_count(&instances), 2);
        assert_eq!(running_count(&[]), 0);
    }

    #[test]
    fn a_deploy_says_what_it_installs_first() {
        let started = json!([
            { "id": "j1", "kind": "package.action", "payload": { "package": "docker", "action": "install" } },
            { "id": "j2", "kind": "package.action", "payload": { "package": "docker", "action": "install" } },
            { "id": "j3", "kind": "app.action" },
        ]);
        let note = install_note(&started).unwrap_or_default();
        assert!(note.contains("(docker)") && note.contains("2 install job(s)"), "{note}");
        assert!(install_note(&json!([{ "id": "j3", "kind": "app.action" }])).is_none());
    }

    #[test]
    fn job_ids_from_lists_and_single_jobs() {
        assert_eq!(job_ids(&json!([{ "id": "a" }, { "id": "b" }])), ["a", "b"]);
        assert_eq!(job_ids(&json!({ "id": "c" })), ["c"]);
        assert!(job_ids(&json!(null)).is_empty());
    }
}
