//! `forge services plan / apply / scale / export`: Compose / Swarm format
//! services files run by Forge (D-178).
//!
//! The CLI reads the file and sends its text; the server never reads
//! files. `env_file` entries are read here (relative to the file) and
//! inlined into `environment` first, with `environment` winning.
//!
//! Exit status: `0` applied (or nothing to do), `1` the file or plan was
//! refused, `10` a job failed with `--wait`, `8` `--wait` timed out.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Map, Value, json};

use crate::Global;
use crate::api::Api;
use crate::cmds::{confirm, wait_jobs};
use crate::error::{CliError, Exit, Result};
use crate::out::{self, Format};
use crate::rescmd::{resolve_app, resolve_environment};

fn refused(message: impl Into<String>) -> CliError {
    CliError::new(Exit::General, message).with_code("plan_refused")
}

/// A refused request (400/422) exits 1; other errors keep their class.
fn plan_error(e: CliError) -> CliError {
    if e.exit == Exit::Usage { CliError { exit: Exit::General, ..e } } else { e }
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

/// Reads the file and inlines every service's `env_file` (paths relative
/// to `base`). A file without `env_file` is sent exactly as written.
pub fn load(text: &str, base: &Path) -> Result<String> {
    let mut tree = forge_catalog::yaml::parse_document(text).map_err(refused)?;
    let mut changed = false;

    for (name, svc) in tree.get_mut("services").and_then(Value::as_object_mut).into_iter().flatten() {
        let Some(m) = svc.as_object_mut() else { continue };
        let Some(files) = m.remove("env_file") else { continue };
        let files: Vec<String> = match files {
            Value::String(s) => vec![s],
            Value::Array(a) => {
                a.iter().filter_map(|f| f.as_str().or_else(|| f["path"].as_str()).map(str::to_owned)).collect()
            }
            _ => return Err(refused(format!("services.{name}.env_file: a path or a list of paths"))),
        };

        let mut vars = Map::new();

        for f in &files {
            let path = base.join(f);
            let text = std::fs::read_to_string(&path)
                .map_err(|e| refused(format!("services.{name}.env_file: {}: {e}", path.display())))?;
            let parsed = forge_catalog::stacks::parse_env(&text)
                .map_err(|e| refused(format!("services.{name}.env_file: {f}: {e}")))?;
            vars.extend(parsed.into_iter().map(|(k, v)| (k, Value::String(v))));
        }

        // `environment` wins over the files, as in Compose.
        match m.get("environment") {
            Some(Value::Object(own)) => vars.extend(own.clone()),
            Some(Value::Array(own)) => {
                for e in own.iter().filter_map(Value::as_str) {
                    let (k, v) = e.split_once('=').unwrap_or((e, ""));
                    vars.insert(k.to_owned(), Value::String(v.to_owned()));
                }
            }
            _ => {}
        }

        m.insert("environment".into(), Value::Object(vars));
        changed = true;
    }

    Ok(if changed { forge_catalog::stacks::to_yaml(&tree) } else { text.to_owned() })
}

/// Prints a plan for people.
fn print_plan(fmt: Format, plan: &Value) {
    if fmt != Format::Table {
        out::result(fmt, plan, None);
        return;
    }

    let s = |v: &Value, k: &str| v[k].as_str().unwrap_or_default().to_owned();
    let list = |v: &Value, k: &str| {
        v[k].as_array().into_iter().flatten().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")
    };
    println!("Environment  {} (file `{}`)", s(plan, "environment"), s(plan, "source"));

    for c in plan["changes"].as_array().into_iter().flatten() {
        let mut line = format!("  {:<10} {:<24} on {}", s(c, "action"), s(c, "service"), list(c, "nodes"));

        for (k, label) in [
            ("added_nodes", "adds"),
            ("removed_nodes", "takes off"),
            ("fields", "changes"),
            ("privileged", "privileged"),
        ] {
            if c[k].as_array().is_some_and(|a| !a.is_empty()) {
                line.push_str(&format!("; {label} {}", list(c, k)));
            }
        }

        println!("{line}");
    }
}

fn is_noop(plan: &Value) -> bool {
    plan["changes"].as_array().into_iter().flatten().all(|c| c["action"] == "unchanged")
}

/// A services file and where it goes.
pub struct FileArgs<'a> {
    pub file: &'a str,
    pub environment: &'a str,
    pub prune: bool,
}

async fn body(api: &mut Api, a: &FileArgs<'_>) -> Result<Value> {
    let text = load(&read_text(a.file)?, &base_dir(a.file))?;
    let env = resolve_environment(api, a.environment).await?;

    Ok(json!({ "environment_id": env, "file": text, "prune": a.prune }))
}

/// `forge services plan`.
pub async fn plan(g: &Global, api: &mut Api, a: &FileArgs<'_>) -> Result<()> {
    let b = body(api, a).await?;
    let plan = api.post("/api/v1/services/plan", &b).await.map_err(plan_error)?;
    print_plan(g.output, &plan);

    Ok(())
}

/// How `apply` waits.
pub struct Wait {
    pub yes: bool,
    pub wait: bool,
    pub timeout_secs: u64,
}

/// `forge services apply`.
pub async fn apply(g: &Global, api: &mut Api, a: &FileArgs<'_>, w: &Wait) -> Result<()> {
    let b = body(api, a).await?;
    let plan = api.post("/api/v1/services/plan", &b).await.map_err(plan_error)?;
    print_plan(g.output, &plan);

    if is_noop(&plan) {
        if g.output == Format::Table {
            println!("Nothing to do.");
        }

        return Ok(());
    }

    confirm(g, w.yes || g.non_interactive, &format!("Apply this to {}?", a.environment))?;
    let applied = api.post("/api/v1/services/apply", &b).await.map_err(plan_error)?;

    finish(g, api, &applied, w).await
}

/// `forge services scale`.
pub async fn scale(
    g: &Global,
    api: &mut Api,
    service: &str,
    environment: Option<&str>,
    replicas: u32,
    w: &Wait,
) -> Result<()> {
    let app = resolve_app(api, service, environment).await?;
    let id = app["id"].as_str().or_else(|| app["app"]["id"].as_str()).unwrap_or_default().to_owned();
    confirm(g, w.yes || g.non_interactive, &format!("Run {service} on {replicas} node(s)?"))?;
    let applied = api
        .post(&format!("/api/v1/services/{id}/scale"), &json!({ "replicas": replicas }))
        .await
        .map_err(plan_error)?;
    print_plan(g.output, &applied);

    finish(g, api, &applied, w).await
}

async fn finish(g: &Global, api: &mut Api, applied: &Value, w: &Wait) -> Result<()> {
    let ids: Vec<String> =
        applied["jobs"].as_array().into_iter().flatten().filter_map(|j| j["id"].as_str().map(str::to_owned)).collect();
    let cols: &[out::Col] = &[("JOB", "/id"), ("KIND", "/kind"), ("STATUS", "/status")];

    if !w.wait || ids.is_empty() {
        out::result(g.output, &applied["jobs"], Some(cols));
        return Ok(());
    }

    let done = wait_jobs(api, &ids, Duration::from_secs(w.timeout_secs)).await?;
    out::result(g.output, &Value::Array(done), Some(cols));

    Ok(())
}

/// `forge services export`: one service, or with `--environment` and no
/// name every service of the environment.
pub async fn export(api: &mut Api, service: Option<&str>, environment: Option<&str>) -> Result<()> {
    let v = match (service, environment) {
        (Some(s), env) => {
            let app = resolve_app(api, s, env).await?;
            let id = app["id"].as_str().or_else(|| app["app"]["id"].as_str()).unwrap_or_default().to_owned();
            api.get(&format!("/api/v1/services/{id}/export")).await?
        }
        (None, Some(env)) => {
            let id = resolve_environment(api, env).await?;
            api.get(&format!("/api/v1/environments/{id}/services/export")).await?
        }
        (None, None) => return Err(CliError::usage("name a service, or pass --environment for all of its services")),
    };

    print!("{}", v["yaml"].as_str().unwrap_or_default());

    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // reason: tests
mod tests {
    use super::*;

    #[test]
    fn env_files_are_inlined_and_environment_wins() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("app.env"), "A=from-file\nB=2\n").unwrap();
        let text = "services:\n  web:\n    image: nginx\n    env_file: app.env\n    environment:\n      A: own\n";
        let out = load(text, dir.path()).unwrap();
        let tree = forge_catalog::yaml::parse_document(&out).unwrap();

        assert_eq!(tree["services"]["web"]["environment"]["A"], "own");
        assert_eq!(tree["services"]["web"]["environment"]["B"], "2");
        assert!(tree["services"]["web"].get("env_file").is_none());

        let plain = "services:\n  web:\n    image: nginx\n";
        assert_eq!(load(plain, dir.path()).unwrap(), plain);
    }
}
