//! `forge stacks validate / plan / apply -f forge.stack.yaml`
//! (D-174).
//!
//! The CLI reads the document, the compose file and the `.env` file it
//! names (paths relative to the document) and sends their text; the server
//! never reads files. `validate` works offline. `apply --wait` follows the
//! `stack.rollout` job and prints the stack's names when it is live.
//!
//! Exit status: `0` live (or started, without `--wait`), `1` the document or
//! plan was refused, `7` a rollout of the stack is already running, `2` a
//! node failed.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use clap::{Args, Subcommand};
use forge_catalog::stack_doc::{self, StackDoc};
use serde_json::{Value, json};

use crate::admincmd::enc;
use crate::cmds::confirm;
use crate::error::{CliError, Exit, Result};
use crate::out::{self, Format};
use crate::{Global, connect, target};

#[derive(Args, Debug)]
pub struct StackDocArgs {
    /// The stack document (`-` reads stdin; paths are then relative to the
    /// current folder).
    #[arg(long, short = 'f', value_name = "FILE", default_value = "forge.stack.yaml")]
    pub file: String,
    /// Apply to this environment instead of `metadata.environment`; its
    /// `overrides` entry is merged in.
    #[arg(long, value_name = "SLUG")]
    pub environment: Option<String>,
}

/// `forge.stack.yaml` commands (flattened into `forge stacks`).
#[derive(Subcommand, Debug)]
pub enum StackDocCmd {
    /// Check a stack document and the files it names, offline.
    Validate(StackDocArgs),
    /// Show what applying a stack document would do: nodes, network, findings.
    Plan(StackDocArgs),
    /// Create or update the stack from a document and roll it out.
    Apply {
        #[command(flatten)]
        doc: StackDocArgs,
        /// Follow the rollout until it ends; exit 2 if a node failed.
        #[arg(long)]
        wait: bool,
        /// Longest wait with `--wait`, in minutes.
        #[arg(long, default_value_t = 60, value_name = "MINUTES")]
        timeout: u64,
        /// Start without asking.
        #[arg(long)]
        yes: bool,
    },
}

/// A document with the texts it names.
struct Loaded {
    doc: StackDoc,
    document: String,
    compose: String,
    env: Option<String>,
}

fn refused(message: impl Into<String>) -> CliError {
    CliError::new(Exit::General, message).with_code("plan_refused")
}

fn read_text(file: &str) -> Result<String> {
    let mut text = String::new();

    if file == "-" {
        std::io::stdin().read_to_string(&mut text).map_err(|e| CliError::usage(format!("stdin: {e}")))?;
    } else {
        text = std::fs::read_to_string(file).map_err(|e| CliError::usage(format!("{file}: {e}")))?;
    }

    Ok(text)
}

fn base_dir(file: &str) -> PathBuf {
    if file == "-" {
        return PathBuf::from(".");
    }

    Path::new(file).parent().map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

/// Reads the document and the compose and `.env` files it names.
fn load(args: &StackDocArgs) -> Result<Loaded> {
    let document = read_text(&args.file)?;
    let yaml = forge_catalog::yaml::parse_document(&document).map_err(refused)?;
    let doc = stack_doc::parse(yaml, args.environment.as_deref()).map_err(|e| refused(e.to_string()))?;
    let base = base_dir(&args.file);
    let read = |rel: &str, field: &str| {
        let path = base.join(rel);
        std::fs::read_to_string(&path).map_err(|e| refused(format!("{field}: {}: {e}", path.display())))
    };

    let compose = match (&doc.spec.compose.file, &doc.spec.compose.inline) {
        (Some(f), _) => read(f, "spec.compose.file")?,
        (None, Some(i)) => i.clone(),
        (None, None) => return Err(refused("spec.compose: set exactly one of file and inline")),
    };

    let env = doc.spec.compose.env_file.as_deref().map(|e| read(e, "spec.compose.envFile")).transpose()?;

    // The compose file's own checks need the server; these need only the document.
    let tree = forge_catalog::yaml::parse_document(&compose).map_err(|e| refused(format!("compose: {e}")))?;
    doc.check_compose(&tree).map_err(|e| refused(e.to_string()))?;
    doc.compose_overlay(&tree).map_err(|e| refused(e.to_string()))?;

    Ok(Loaded { doc, document, compose, env })
}

fn body(l: &Loaded, args: &StackDocArgs) -> Value {
    json!({ "document": l.document, "compose": l.compose, "env": l.env, "environment": args.environment })
}

/// A refused request (400) exits 1; other errors keep their class.
fn plan_error(e: CliError) -> CliError {
    if e.exit == Exit::Usage { CliError { exit: Exit::General, ..e } } else { e }
}

fn duration(secs: u64) -> String {
    match secs {
        s if s % 3600 == 0 => format!("{}h", s / 3600),
        s if s % 60 == 0 => format!("{}m", s / 60),
        s => format!("{s}s"),
    }
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect()
}

/// Prints a plan for people.
fn print_plan(fmt: Format, plan: &Value) {
    if fmt != Format::Table {
        out::result(fmt, plan, None);
        return;
    }

    let s = |p: &str| plan.pointer(p).and_then(Value::as_str).unwrap_or_default().to_owned();
    let action = if s("/action") == "create" { "new stack" } else { "update" };
    println!("Stack        {} in {} ({action}, revision {})", s("/stack"), s("/environment"), plan["revision_next"]);
    println!(
        "Rollout      {}, health timeout {}",
        s("/order"),
        duration(plan["health_timeout_s"].as_u64().unwrap_or_default())
    );
    println!("Nodes");

    for n in plan["nodes"].as_array().into_iter().flatten() {
        println!("  {:<20} {}", n["hostname"].as_str().unwrap_or_default(), strings(&n["services"]).join(", "));
    }

    let removes = strings(&plan["removes"]);

    if !removes.is_empty() {
        println!("Takes down   {} node(s) of the current revision", removes.len());
    }

    let mesh = &plan["mesh"];

    if mesh["enabled"] == false {
        println!("Network      cluster network off (spec.network.mesh)");
    } else {
        let (on, off) = (strings(&mesh["nodes_in_sync"]), strings(&mesh["nodes_not_in_sync"]));
        let on = if on.is_empty() { "-".to_owned() } else { on.join(", ") };
        let off = if off.is_empty() { String::new() } else { format!("; not in sync: {}", off.join(", ")) };
        println!("Network      cluster network on {on}{off}");
    }

    for f in plan["findings"].as_array().into_iter().flatten() {
        let svc = f["service"].as_str().filter(|x| !x.is_empty()).map(|x| format!("{x}: ")).unwrap_or_default();
        println!("{:<12} {svc}{}", f["severity"].as_str().unwrap_or("info"), f["message"].as_str().unwrap_or_default());
    }
}

const fn finished(status: &str) -> bool {
    matches!(status.as_bytes(), b"succeeded" | b"failed" | b"cancelled")
}

/// Follows the rollout job until it ends, like `forge deploy apply --wait`.
async fn follow(g: &Global, api: &mut crate::api::Api, id: &str, timeout: Duration) -> Result<Value> {
    let deadline = Instant::now() + timeout;
    let mut shown = 0usize;

    loop {
        let j = api.get(&format!("/api/v1/jobs/{}", enc(id))).await?;
        let nodes = j.pointer("/state/nodes").and_then(Value::as_array).cloned().unwrap_or_default();

        if g.output == Format::Table {
            for n in nodes.iter().skip(shown) {
                eprintln!("· node {} {}", n["server_id"].as_str().unwrap_or("?"), n["outcome"].as_str().unwrap_or(""));
            }
        }

        shown = shown.max(nodes.len());
        let status = j["status"].as_str().unwrap_or_default().to_owned();

        if finished(&status) {
            if status == "succeeded" {
                return Ok(j);
            }

            let why = j["last_error"].as_str().unwrap_or(&status).to_owned();

            return Err(
                CliError::new(Exit::Usage, format!("the rollout ended {status}: {why}")).with_code("node_failed")
            );
        }

        if Instant::now() >= deadline {
            return Err(CliError::new(
                Exit::Timeout,
                format!("the rollout is still {status}; follow it with `forge jobs get {id}`"),
            ));
        }

        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

/// The stack's service names with their addresses (`stack-service` rows).
fn names_of(names: &Value) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();

    for r in names["names"].as_array().into_iter().flatten().filter(|r| r["kind"] == "stack-service") {
        let addr = r["mesh_address"].as_str().or_else(|| r["address"].as_str()).unwrap_or_default().to_owned();
        let e = out.entry(r["name"].as_str().unwrap_or_default().to_owned()).or_default();

        if !addr.is_empty() && !e.contains(&addr) {
            e.push(addr);
        }
    }

    out
}

fn print_live(stack: &str, revision: &Value, nodes: usize, names: &BTreeMap<String, Vec<String>>) {
    println!("stack {stack} revision {revision} is live on {nodes} node{}", if nodes == 1 { "" } else { "s" });

    if names.is_empty() {
        return;
    }

    println!("names:");
    let width = names.keys().map(String::len).max().unwrap_or(0);

    for (name, addrs) in names {
        println!("  {name:<width$}   {}", addrs.join(", "));
    }
}

pub async fn run(g: &Global, cmd: StackDocCmd) -> Result<()> {
    match cmd {
        StackDocCmd::Validate(args) => {
            let l = load(&args)?;
            let v = json!({
                "valid": true, "stack": l.doc.metadata.name, "environment": l.doc.metadata.environment,
                "order": l.doc.spec.rollout.order, "health_timeout_s": l.doc.health_timeout_s(),
                "placement": l.doc.spec.placement, "mesh": l.doc.spec.network.mesh, "env_file": l.env.is_some(),
            });
            out::result(g.output, &v, None);
            Ok(())
        }
        StackDocCmd::Plan(args) => {
            let l = load(&args)?;
            let mut api = connect(g, &target(g)?).await?;
            let plan = api.post("/api/v1/stacks/plan", &body(&l, &args)).await.map_err(plan_error)?;
            print_plan(g.output, &plan);

            if plan["blocked"] == true {
                return Err(refused("the plan has blocked findings"));
            }

            Ok(())
        }
        StackDocCmd::Apply { doc: args, wait, timeout, yes } => {
            let l = load(&args)?;
            let mut api = connect(g, &target(g)?).await?;
            let b = body(&l, &args);
            let plan = api.post("/api/v1/stacks/plan", &b).await.map_err(plan_error)?;
            print_plan(g.output, &plan);

            if plan["blocked"] == true {
                return Err(refused("the plan has blocked findings"));
            }

            let stack = l.doc.metadata.name.clone();
            confirm(g, yes || g.non_interactive, &format!("Apply stack {stack} to {}?", l.doc.metadata.environment))?;
            let started = api.post("/api/v1/stacks/apply", &b).await.map_err(plan_error)?;
            let job = started["job_id"].as_str().unwrap_or_default().to_owned();

            if !wait {
                out::result(
                    g.output,
                    &started,
                    Some(&[("JOB", "/job_id"), ("STACK", "/stack_id"), ("REVISION", "/revision")]),
                );
                return Ok(());
            }

            follow(g, &mut api, &job, Duration::from_secs(timeout.max(1) * 60)).await?;
            let id = started["stack_id"].as_str().unwrap_or_default();
            let names = api.get(&format!("/api/v1/names?stack={}", enc(id))).await?;
            let nodes = started.pointer("/plan/nodes").and_then(Value::as_array).map_or(0, Vec::len);

            if g.output == Format::Table {
                print_live(&stack, &started["revision"], nodes, &names_of(&names));
            } else {
                let v = json!({ "job_id": job, "stack_id": id, "revision": started["revision"], "nodes": nodes, "names": names_of(&names) });
                out::result(g.output, &v, None);
            }

            Ok(())
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // reason: tests
mod tests {
    use super::*;

    #[test]
    fn names_group_addresses_per_service() {
        let v = json!({ "names": [
            { "kind": "stack-service", "name": "api.orders.prod.c1.local", "mesh_address": "10.64.10.2" },
            { "kind": "stack-service", "name": "api.orders.prod.c1.local", "mesh_address": "10.64.12.4" },
            { "kind": "replica", "name": "api-1.orders.prod.c1.local", "mesh_address": "10.64.10.2" },
            { "kind": "stack-service", "name": "worker.orders.prod.c1.local", "mesh_address": "10.64.10.3" },
        ]});
        let n = names_of(&v);
        assert_eq!(n.len(), 2);
        assert_eq!(n["api.orders.prod.c1.local"], ["10.64.10.2", "10.64.12.4"]);
        assert_eq!(duration(300), "5m");
        assert_eq!(duration(3600), "1h");
        assert_eq!(duration(90), "90s");
        assert_eq!(plan_error(CliError::usage("x")).exit, Exit::General);
    }

    #[test]
    fn a_document_reads_its_files_relative_to_itself() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("compose.yaml"), "services:\n  web:\n    image: nginx\n").unwrap();
        std::fs::write(dir.path().join("prod.env"), "A=1\n").unwrap();
        let doc = dir.path().join("forge.stack.yaml");
        std::fs::write(
            &doc,
            "apiVersion: forge/v1\nkind: Stack\nmetadata: { name: orders, environment: prod }\nspec:\n  compose: { file: compose.yaml, envFile: prod.env }\n",
        )
        .unwrap();
        let l = load(&StackDocArgs { file: doc.display().to_string(), environment: None }).unwrap();
        assert!(l.compose.contains("nginx"));
        assert_eq!(l.env.as_deref(), Some("A=1\n"));
        std::fs::write(&doc, "apiVersion: forge/v1\nkind: Stack\nmetadata: { name: orders, environment: prod }\nspec:\n  compose: { file: missing.yaml }\n").unwrap();
        let e = load(&StackDocArgs { file: doc.display().to_string(), environment: None }).err().unwrap();
        assert_eq!(e.exit, Exit::General);
        assert!(e.message.starts_with("spec.compose.file:"), "{}", e.message);
    }
}
