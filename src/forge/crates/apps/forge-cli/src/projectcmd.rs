//! `forge projects`: the console's *Projects* (D-102).
//!
//! A project groups environments; the services that run in them are
//! `forge services`. The API serves projects at `/api/v1/projects`.

use clap::Subcommand;
use reqwest::Method;
use serde_json::{Value, json};

use crate::admincmd::{enc, pick};
use crate::api::Api;
use crate::cmds::confirm;
use crate::error::{CliError, Result};
use crate::out::{self, Col};
use crate::{Global, connect, target};

const BASE: &str = "/api/v1/projects";

const COLS: &[Col] = &[
    ("ID", "/id"),
    ("NAME", "/name"),
    ("ENVIRONMENTS", "/environment_names"),
    ("SERVICES", "/service_count"),
    ("DESCRIPTION", "/description"),
];

#[derive(Subcommand, Debug)]
pub enum ProjectsCmd {
    /// Projects you can see, with their environments and service counts.
    List {
        #[command(flatten)]
        paging: out::Paging,
    },
    /// One project with its environments.
    #[command(alias = "get")]
    Show {
        /// The project (id or name).
        project: String,
    },
    /// Create a project. Add environments with `forge environments create
    /// --project <project>`, then services with `forge services create`.
    Create {
        /// Name: 1-63 lower-case letters, digits and dashes; unique in the workspace.
        name: String,
        /// What the project is, for people (at most 500 characters).
        #[arg(long, default_value = "")]
        description: String,
    },
    /// Rename a project or change its description.
    Update {
        /// The project (id or name).
        project: String,
        /// New name: 1-63 lower-case letters, digits and dashes.
        #[arg(long)]
        name: Option<String>,
        /// New description (an empty string clears it).
        #[arg(long)]
        description: Option<String>,
        /// Do not ask first (required with --non-interactive).
        #[arg(long)]
        yes: bool,
    },
    /// Delete a project that has no environments left.
    Delete {
        /// The project (id or name).
        project: String,
        /// Do not ask first (required with --non-interactive).
        #[arg(long)]
        yes: bool,
    },
}

/// The project `needle` names (id or name) among `all`.
fn find(all: &Value, needle: &str) -> Result<Value> {
    pick(all, &["name"], "project", needle)
}

/// A list row: the group plus its environment names as one cell.
fn row(group: &Value) -> Value {
    let mut r = group.clone();
    let names: Vec<&str> =
        group["environments"].as_array().into_iter().flatten().filter_map(|e| e["name"].as_str()).collect();
    r["environment_names"] = json!(if names.is_empty() { "-".to_owned() } else { names.join(", ") });

    r
}

/// The body of `forge projects update`: only the fields that were given.
fn update_body(name: Option<&str>, description: Option<&str>) -> Result<Value> {
    if name.is_none() && description.is_none() {
        return Err(CliError::usage("pass --name and/or --description"));
    }

    let mut body = serde_json::Map::new();

    if let Some(n) = name {
        body.insert("name".into(), json!(n));
    }

    if let Some(d) = description {
        body.insert("description".into(), json!(d));
    }

    Ok(Value::Object(body))
}

async fn resolve(api: &mut Api, needle: &str) -> Result<Value> {
    find(&api.get(BASE).await?, needle)
}

pub async fn run(g: &Global, cmd: ProjectsCmd) -> Result<()> {
    let t = target(g)?;
    let mut api = connect(g, &t).await?;

    match cmd {
        ProjectsCmd::List { paging } => {
            let v = api.get(BASE).await?;
            let rows: Vec<Value> = v.as_array().into_iter().flatten().map(row).collect();
            out::list(g.output, rows, Some(COLS), paging.into(), out::Total::Rows);
            Ok(())
        }
        ProjectsCmd::Show { project } => {
            let a = resolve(&mut api, &project).await?;
            out::result(g.output, &a, None);
            Ok(())
        }
        ProjectsCmd::Create { name, description } => {
            let v = api.post(BASE, &json!({ "name": name, "description": description })).await?;
            out::result(g.output, &v, None);
            Ok(())
        }
        ProjectsCmd::Update { project, name, description, yes } => {
            let body = update_body(name.as_deref(), description.as_deref())?;
            let a = resolve(&mut api, &project).await?;
            let id = a["id"].as_str().unwrap_or_default();
            confirm(g, yes, &format!("Update project {} on {}?", a["name"].as_str().unwrap_or(&project), t.server))?;
            let v = api.call(Method::PUT, &format!("{BASE}/{}", enc(id)), Some(&body)).await?;
            out::result(g.output, &v, None);
            Ok(())
        }
        ProjectsCmd::Delete { project, yes } => {
            let a = resolve(&mut api, &project).await?;
            let id = a["id"].as_str().unwrap_or_default();
            let name = a["name"].as_str().unwrap_or(&project).to_owned();
            confirm(g, yes, &format!("Delete project {name} on {}?", t.server))?;
            api.call(Method::DELETE, &format!("{BASE}/{}", enc(id)), None).await?;
            out::result(g.output, &json!({ "deleted": id, "name": name }), None);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;
    use crate::error::Exit;
    use crate::{Cli, Cmd};

    fn parse(args: &[&str]) -> ProjectsCmd {
        match Cli::try_parse_from(args).map(|c| c.cmd) {
            Ok(Cmd::Projects(c)) => c,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn create_and_update_parse_their_flags() {
        let ProjectsCmd::Create { name, description } =
            parse(&["forge", "projects", "create", "shop", "--description", "Web shop"])
        else {
            panic!("create")
        };

        assert_eq!((name.as_str(), description.as_str()), ("shop", "Web shop"));

        let ProjectsCmd::Update { project, name, description, yes } =
            parse(&["forge", "projects", "update", "shop", "--name", "store", "--yes"])
        else {
            panic!("update")
        };

        assert_eq!((project.as_str(), name.as_deref(), description, yes), ("shop", Some("store"), None, true));

        // `get` stays an alias of `show`.
        assert!(matches!(parse(&["forge", "projects", "get", "shop"]), ProjectsCmd::Show { .. }));
    }

    #[test]
    fn update_sends_only_what_was_given() {
        assert_eq!(update_body(Some("store"), None).ok(), Some(json!({ "name": "store" })));
        assert_eq!(update_body(None, Some("")).ok(), Some(json!({ "description": "" })));
        assert_eq!(update_body(None, None).map_err(|e| e.exit).err(), Some(Exit::Usage));
    }

    #[test]
    fn projects_resolve_by_id_or_name_and_rows_name_environments() {
        let all = json!([
            { "id": "g1", "name": "shop", "environments": [{ "name": "shop-prod" }, { "name": "shop-uat" }] },
            { "id": "g2", "name": "billing", "environments": [] },
        ]);
        assert_eq!(find(&all, "shop").ok().map(|a| a["id"].clone()), Some(json!("g1")));
        assert_eq!(find(&all, "g2").ok().map(|a| a["name"].clone()), Some(json!("billing")));
        assert_eq!(find(&all, "nope").map_err(|e| e.exit).err(), Some(Exit::NotFound));
        assert_eq!(row(&all[0])["environment_names"], "shop-prod, shop-uat");
        assert_eq!(row(&all[1])["environment_names"], "-");
    }
}
