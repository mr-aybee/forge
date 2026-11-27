//! Administration and remaining console areas for `forge` (issue #5, phase 4):
//! approvals, people and access, API keys, connectors, settings, tenancy,
//! sites and certificates, the vault, system
//! update, packages and telemetry. Every command calls the same API as the
//! console; the server authorizes, asks for MFA and audits.

use std::io::{IsTerminal, Read as _};

use clap::Subcommand;
use reqwest::Method;
use serde_json::{Value, json};

use crate::api::Api;
use crate::cmds::{confirm, resolve_node};
use crate::error::{CliError, Exit, Result};
use crate::out::{self, Col};
use crate::{Global, connect, target};

// ------------------------------------------------------------------ helpers

fn show(g: &Global, v: &Value, cols: Option<&[Col]>) -> Result<()> {
    out::result(g.output, v, cols);
    Ok(())
}

pub(crate) fn enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

pub(crate) fn looks_like_id(s: &str) -> bool {
    s.len() == 36
        && s.bytes().filter(|&b| b == b'-').count() == 4
        && s.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
}

/// The array at `pointer` (or the value itself when it is an array).
fn rows(v: &Value, pointer: &str) -> Value {
    if pointer.is_empty() { v.clone() } else { v.pointer(pointer).cloned().unwrap_or_else(|| json!([])) }
}

/// One item of a list by id or by one of `fields`: exactly one match, never the first.
async fn resolve(api: &mut Api, list: &str, pointer: &str, fields: &[&str], what: &str, needle: &str) -> Result<Value> {
    let all = rows(&api.get(list).await?, pointer);
    pick(&all, fields, what, needle)
}

/// The one item of `all` whose id, or one of `fields` (case-insensitive), is `needle`.
pub(crate) fn pick(all: &Value, fields: &[&str], what: &str, needle: &str) -> Result<Value> {
    let items: Vec<&Value> = all.as_array().into_iter().flatten().collect();
    if let Some(hit) = items.iter().find(|i| i["id"].as_str() == Some(needle)) {
        return Ok((*hit).clone());
    }

    let m: Vec<&&Value> = items
        .iter()
        .filter(|i| fields.iter().any(|f| i[*f].as_str().is_some_and(|s| s.eq_ignore_ascii_case(needle))))
        .collect();

    match m.as_slice() {
        [one] => Ok((**one).clone()),
        [] => Err(CliError::new(Exit::NotFound, format!("no {what} `{needle}` that you can see"))),
        many => Err(CliError::usage(format!(
            "`{needle}` matches {} {what}s; use an id: {}",
            many.len(),
            many.iter().filter_map(|i| i["id"].as_str()).collect::<Vec<_>>().join(", ")
        ))
        .with_code("ambiguous")),
    }
}

fn id_of(v: &Value) -> String {
    v["id"].as_str().unwrap_or_default().to_owned()
}

pub(crate) async fn cluster_id(api: &mut Api, c: &str) -> Result<String> {
    if looks_like_id(c) {
        return Ok(c.to_owned());
    }

    Ok(id_of(&resolve(api, "/api/v1/clusters", "", &["name", "abbr"], "cluster", c).await?))
}

/// A project (the console's *Service*, D-102) by id or name.
async fn project_id(api: &mut Api, g: &str) -> Result<String> {
    if looks_like_id(g) {
        return Ok(g.to_owned());
    }

    Ok(id_of(&resolve(api, "/api/v1/projects", "", &["name"], "project", g).await?))
}

pub(crate) async fn node_id(api: &mut Api, n: &str) -> Result<String> {
    Ok(id_of(&resolve_node(api, n).await?))
}

/// Adds `node` (hostname, else display name, else the id) to each row from
/// its `field` server id. Best effort: without access to the node list the
/// ids stay.
pub(crate) async fn add_node_names(api: &mut Api, rows: &mut [Value], field: &str) {
    let names = node_names(api).await;

    for r in rows {
        let id = r[field].as_str().unwrap_or_default().to_owned();
        r["node"] = json!(names.get(&id).cloned().unwrap_or(id));
    }
}

/// Server id → node name, for display.
pub(crate) async fn node_names(api: &mut Api) -> std::collections::HashMap<String, String> {
    let all = api.get("/api/v1/servers").await.unwrap_or(Value::Null);
    all.as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| {
            let name =
                s["hostname"].as_str().filter(|d| !d.is_empty()).or_else(|| s["display_name"].as_str())?.to_owned();
            Some((s["id"].as_str()?.to_owned(), name))
        })
        .collect()
}

/// A JSON document from `@file.json`, `@-` (stdin) or inline text.
pub(crate) fn read_json(spec: &str) -> Result<Value> {
    let text = match spec {
        "@-" => std::io::read_to_string(std::io::stdin())?,
        s if s.starts_with('@') => {
            std::fs::read_to_string(&s[1..]).map_err(|e| CliError::usage(format!("{}: {e}", &s[1..])))?
        }
        s => std::fs::read_to_string(s).map_err(|e| CliError::usage(format!("{s}: {e}")))?,
    };

    serde_json::from_str(&text).map_err(|e| CliError::usage(format!("{spec} is not valid JSON: {e}")))
}

/// Reads `--file` and checks it against the server's own schema for
/// `method path` before anything is sent (D-077). A server without the
/// schema, or an untyped operation, is not checked.
pub(crate) async fn file_body(api: &mut Api, method: &str, path: &str, file: &str) -> Result<Value> {
    let body = read_json(file)?;
    check_body(api, method, path, &body).await?;

    Ok(body)
}

/// Checks `body` against the server's schema for `method path` (D-077), so a
/// body built from flags fails as early and as precisely as one from a file.
pub(crate) async fn check_body(api: &mut Api, method: &str, path: &str, body: &Value) -> Result<()> {
    if let Ok(doc) = api.get("/api/openapi.json").await {
        crate::schemacheck::validate(&doc, method, path, body)?;
    }

    Ok(())
}

/// A secret from stdin (never from a flag, so it stays out of shell history
/// and process lists). One trailing newline is dropped.
fn secret_from_stdin(what: &str) -> Result<String> {
    if std::io::stdin().is_terminal() {
        return Err(CliError::usage(format!(
            "pipe the {what} on stdin (it is never taken from a flag), e.g. `… --password-stdin < file`"
        )));
    }

    let mut s = String::new();
    std::io::stdin().read_to_string(&mut s)?;
    let s = s.strip_suffix('\n').map(|t| t.strip_suffix('\r').unwrap_or(t)).unwrap_or(&s).to_owned();
    if s.is_empty() {
        return Err(CliError::usage(format!("the {what} on stdin is empty")));
    }

    Ok(s)
}

// ------------------------------------------------------------------ approvals

const APPROVAL_COLS: &[Col] = &[
    ("ID", "/id"),
    ("STATUS", "/status"),
    ("ACTION", "/action"),
    ("SUMMARY", "/summary"),
    ("REQUESTER", "/requester"),
    ("EXPIRES", "/expires_at"),
];

#[derive(Subcommand, Debug)]
pub enum ApprovalsCmd {
    /// Changes your AI connectors asked for (Ask mode).
    List {
        /// Only this status: pending, approved, denied, executed, failed, expired, cancelled.
        #[arg(long)]
        status: Option<String>,
        /// Everyone's requests (needs user.manage).
        #[arg(long)]
        all: bool,
        #[command(flatten)]
        paging: out::PageArgs,
    },
    /// One request, with the exact call it would make.
    Get { id: String },
    /// Refuse a pending request; nothing runs.
    Deny {
        id: String,
        #[arg(long)]
        yes: bool,
    },
    /// Approving happens in the console only, so an agent that runs this CLI
    /// cannot approve its own request. Prints where to go.
    Approve { id: String },
}

pub async fn approvals(g: &Global, cmd: ApprovalsCmd) -> Result<()> {
    let t = target(g)?;
    if let ApprovalsCmd::Approve { id } = &cmd {
        return Err(CliError::new(
            Exit::MfaRequired,
            format!(
                "approve requests in the Forge console: {}/approvals?id={} (the CLI can list and deny only, so an agent cannot approve its own request)",
                t.server,
                enc(id)
            ),
        )
        .with_code("approval_in_console"));
    }
    let mut api = connect(g, &t).await?;

    match cmd {
        ApprovalsCmd::List { status, all, paging } => {
            let mut q = Vec::new();

            if let Some(s) = status {
                q.push(format!("status={}", enc(&s)));
            }

            if all {
                q.push("all=true".to_owned());
            }

            let path = if q.is_empty() {
                "/api/v1/approvals".to_owned()
            } else {
                format!("/api/v1/approvals?{}", q.join("&"))
            };

            let v = api.get(&path).await?;
            out::list_value(g.output, v, "/approvals", Some(APPROVAL_COLS), paging);
            Ok(())
        }
        ApprovalsCmd::Get { id } => {
            let v = api.get(&format!("/api/v1/approvals/{}", enc(&id))).await?;
            show(g, &v, None)
        }
        ApprovalsCmd::Deny { id, yes } => {
            let a = api.get(&format!("/api/v1/approvals/{}", enc(&id))).await?;
            confirm(g, yes, &format!("Deny request {id} ({})?", a["summary"].as_str().unwrap_or("?")))?;
            let v = api.post(&format!("/api/v1/approvals/{}/deny", enc(&id)), &json!({})).await?;
            show(g, &v, None)
        }
        ApprovalsCmd::Approve { .. } => Ok(()),
    }
}

// ------------------------------------------------------------------ users and access

#[derive(Subcommand, Debug)]
pub enum UsersCmd {
    /// People with a Forge account (needs user.manage).
    List {
        #[command(flatten)]
        paging: out::Paging,
    },
    /// Create a local account. The password is read from stdin.
    Create {
        username: String,
        #[arg(long)]
        email: Option<String>,
        #[arg(long, default_value = "")]
        display_name: String,
        /// Read the initial password from stdin (required).
        #[arg(long, required = true)]
        password_stdin: bool,
    },
    /// Remove all passkeys of a user (lost device). Needs MFA.
    ResetPasskeys {
        user: String,
        #[arg(long)]
        yes: bool,
    },
}

pub(crate) async fn user_id(api: &mut Api, u: &str) -> Result<String> {
    Ok(id_of(&resolve(api, "/api/v1/users", "", &["username", "email"], "user", u).await?))
}

pub async fn users(g: &Global, cmd: UsersCmd) -> Result<()> {
    let t = target(g)?;
    let mut api = connect(g, &t).await?;

    match cmd {
        UsersCmd::List { paging } => {
            let v = api.get("/api/v1/users").await?;
            out::list_value(
                g.output,
                v,
                "",
                Some(&[
                    ("ID", "/id"),
                    ("USERNAME", "/username"),
                    ("EMAIL", "/email"),
                    ("NAME", "/display_name"),
                    ("DISABLED", "/disabled"),
                ]),
                paging,
            );
            Ok(())
        }
        UsersCmd::Create { username, email, display_name, password_stdin: _ } => {
            let password = secret_from_stdin("password")?;
            let body =
                json!({ "username": username, "email": email, "display_name": display_name, "password": password });
            let v = api.post("/api/v1/users", &body).await?;
            show(g, &v, None)
        }
        UsersCmd::ResetPasskeys { user, yes } => {
            let id = user_id(&mut api, &user).await?;
            confirm(g, yes, &format!("Remove every passkey of {user}?"))?;
            let v = api.call(Method::DELETE, &format!("/api/v1/users/{id}/passkeys"), None).await?;
            show(g, &v, None)
        }
    }
}

#[derive(Subcommand, Debug)]
pub enum AccessCmd {
    /// Role bindings: yours, plus those on scopes where you hold user.manage.
    List {
        #[command(flatten)]
        paging: out::Paging,
    },
    /// Give a user a role on a scope.
    Grant {
        /// Username, email or id.
        user: String,
        /// platform_admin, workspace_admin, operator, developer, dba, auditor or customer.
        role: String,
        /// `platform:root`, `workspace:<id>`, `tenant:<id>`, `cluster:<id|name>` or `environment:<id>`.
        scope: String,
        #[arg(long)]
        yes: bool,
    },
    /// Remove a role binding by id.
    Revoke {
        binding: String,
        #[arg(long)]
        yes: bool,
    },
    /// Every action an access profile can hold, whether it needs MFA and which roles hold it.
    Catalog,
    /// Access profiles: named, versioned sets of actions (D-086).
    #[command(subcommand)]
    Profiles(crate::profilescmd::ProfilesCmd),
    /// Profiles given to users, service accounts or groups at a scope.
    #[command(subcommand)]
    Assignments(crate::profilescmd::AssignmentsCmd),
    /// User groups: assign a profile to a group to grant it to every member.
    #[command(subcommand)]
    Groups(crate::profilescmd::GroupsCmd),
    /// Every action a user may perform at a scope, and the assignments behind it.
    Effective {
        /// Username, email or id.
        user: String,
        /// `workspace:<id>`, `cluster:<id|name>`, `project:<id>`, `environment:<id>`, `server:<id>` or `service:<id>`.
        scope: String,
        /// List only the allowed actions.
        #[arg(long)]
        allowed: bool,
    },
}

pub async fn access(g: &Global, cmd: AccessCmd) -> Result<()> {
    let t = target(g)?;
    let mut api = connect(g, &t).await?;

    match cmd {
        AccessCmd::List { paging } => {
            let v = api.get("/api/v1/role-bindings").await?;
            out::list_value(
                g.output,
                v,
                "",
                Some(&[
                    ("ID", "/id"),
                    ("USER", "/user_id"),
                    ("ROLE", "/role"),
                    ("KIND", "/scope/kind"),
                    ("SCOPE", "/scope/id"),
                ]),
                paging,
            );
            Ok(())
        }
        AccessCmd::Grant { user, role, scope, yes } => {
            let uid = user_id(&mut api, &user).await?;

            let scope = match scope.split_once(':') {
                Some(("cluster", c)) => format!("cluster:{}", cluster_id(&mut api, c).await?),
                _ => scope,
            };

            confirm(g, yes, &format!("Give {user} the role {role} on {scope} at {}?", t.server))?;
            let v = api.post("/api/v1/role-bindings", &json!({ "user_id": uid, "role": role, "scope": scope })).await?;
            show(g, &v, None)
        }
        AccessCmd::Revoke { binding, yes } => {
            confirm(g, yes, &format!("Remove role binding {binding}?"))?;
            let v = api.call(Method::DELETE, &format!("/api/v1/role-bindings/{}", enc(&binding)), None).await?;
            show(g, &v, None)
        }
        AccessCmd::Catalog => crate::profilescmd::catalog(g, &mut api).await,
        AccessCmd::Profiles(c) => crate::profilescmd::profiles(g, &mut api, c).await,
        AccessCmd::Assignments(c) => crate::profilescmd::assignments(g, &mut api, c).await,
        AccessCmd::Groups(c) => crate::profilescmd::groups(g, &mut api, c).await,
        AccessCmd::Effective { user, scope, allowed } => {
            crate::profilescmd::effective(g, &mut api, &user, scope, allowed).await
        }
    }
}

// ------------------------------------------------------------------ API keys and connectors

#[derive(Subcommand, Debug)]
pub enum ApiKeysCmd {
    /// Your personal API keys, or another user's (platform admins).
    List {
        /// Another user's keys (user id; needs Platform admin).
        #[arg(long)]
        user: Option<String>,
        #[command(flatten)]
        paging: out::Paging,
    },
    /// Create a key; the secret is printed once.
    Create {
        name: String,
        /// Days until it expires (1-3650; default 90).
        #[arg(long, conflicts_with = "never_expires")]
        expires_days: Option<u32>,
        /// A key that never expires (flagged in lists).
        #[arg(long)]
        never_expires: bool,
        /// May run MFA-guarded actions without a prompt (CI). Needs an
        /// MFA-verified session and `[security] allow_elevated_tokens`.
        #[arg(long)]
        elevated: bool,
        /// Restrict the key to these actions at a scope:
        /// `<kind>:<id>=<action>[,<action>…]`, for example
        /// `environment:<id>=view,health.view`. Repeat for more scopes. Without
        /// it the key acts with all of your access; with it, only with what
        /// is listed and never more than you hold.
        #[arg(long = "allow", value_name = "SCOPE=ACTIONS")]
        allow: Vec<String>,
    },
    /// Delete a key (id).
    Delete {
        id: String,
        /// Another user's key (user id; needs Platform admin).
        #[arg(long)]
        user: Option<String>,
        #[arg(long)]
        yes: bool,
    },
    /// Revoke every API key of a user (platform admins).
    RevokeAll {
        /// The user id.
        #[arg(long)]
        user: String,
        #[arg(long)]
        yes: bool,
    },
}

pub async fn api_keys(g: &Global, cmd: ApiKeysCmd) -> Result<()> {
    let t = target(g)?;
    let mut api = connect(g, &t).await?;

    match cmd {
        ApiKeysCmd::List { user, paging } => {
            let path =
                user.map_or_else(|| "/api/v1/api-keys".to_owned(), |u| format!("/api/v1/users/{}/api-keys", enc(&u)));
            let v = api.get(&path).await?;
            out::list_value(
                g.output,
                v,
                "",
                Some(&[
                    ("ID", "/id"),
                    ("NAME", "/name"),
                    ("PREFIX", "/prefix"),
                    ("ELEVATED", "/elevated"),
                    ("RESTRICTED", "/restrictions"),
                    ("EXPIRES", "/expires_at"),
                    ("USED", "/last_used_at"),
                ]),
                paging,
            );
            Ok(())
        }
        ApiKeysCmd::Create { name, expires_days, never_expires, elevated, allow } => {
            let mut body = json!({ "name": name, "expires_days": expires_days, "never_expires": never_expires });

            if elevated {
                body["elevated"] = json!(true);
            }

            if let Some(r) = restrictions(&mut api, &allow).await? {
                body["restrictions"] = r;
            }

            let v = api.post("/api/v1/api-keys", &body).await?;

            if g.output == out::Format::Table {
                eprintln!("Store this secret now; Forge shows it only once.");
            }

            show(g, &v, None)
        }
        ApiKeysCmd::Delete { id, user, yes } => {
            confirm(g, yes, &format!("Delete API key {id}? Scripts using it stop working."))?;

            let path = match user {
                Some(u) => format!("/api/v1/users/{}/api-keys/{}", enc(&u), enc(&id)),
                None => format!("/api/v1/api-keys/{}", enc(&id)),
            };

            let v = api.call(Method::DELETE, &path, None).await?;
            show(g, &v, None)
        }
        ApiKeysCmd::RevokeAll { user, yes } => {
            confirm(g, yes, &format!("Revoke every API key of user {user}? Their scripts stop working."))?;
            let v = api.call(Method::DELETE, &format!("/api/v1/users/{}/api-keys", enc(&user)), None).await?;
            show(g, &v, None)
        }
    }
}

#[derive(Subcommand, Debug)]
pub enum ConnectorsCmd {
    /// AI apps connected through MCP: yours, or everyone's with --all (user.manage).
    List {
        /// Everyone's connections (needs user.manage).
        #[arg(long)]
        all: bool,
        #[command(flatten)]
        paging: out::PageArgs,
    },
    /// Disconnect one app connection; its tokens stop working.
    Revoke {
        id: String,
        #[arg(long)]
        yes: bool,
    },
    /// Registered OAuth clients (dynamic client registration).
    Clients,
    /// Revoke a registered client and every connection made with it.
    RevokeClient {
        id: String,
        #[arg(long)]
        yes: bool,
    },
}

pub async fn connectors(g: &Global, cmd: ConnectorsCmd) -> Result<()> {
    let t = target(g)?;
    let mut api = connect(g, &t).await?;

    match cmd {
        ConnectorsCmd::List { all, paging } => {
            let v = api.get(if all { "/api/v1/connected-apps?all=true" } else { "/api/v1/connected-apps" }).await?;
            out::list_value(
                g.output,
                v,
                "/connections",
                Some(&[
                    ("ID", "/id"),
                    ("APP", "/client_name"),
                    ("MODE", "/mode"),
                    ("REDIRECT", "/redirect_host"),
                    ("USED", "/last_used_at"),
                    ("REVOKED", "/revoked_at"),
                ]),
                paging,
            );
            Ok(())
        }
        ConnectorsCmd::Revoke { id, yes } => {
            confirm(g, yes, &format!("Disconnect app connection {id}?"))?;
            let v = api.post(&format!("/api/v1/connected-apps/{}/revoke", enc(&id)), &json!({})).await?;
            show(g, &v, None)
        }
        ConnectorsCmd::Clients => {
            let v = api.get("/api/v1/oauth/clients").await?;
            show(
                g,
                &rows(&v, "/clients"),
                Some(&[
                    ("CLIENT", "/client_id"),
                    ("NAME", "/name"),
                    ("USED", "/last_used_at"),
                    ("REVOKED", "/revoked_at"),
                ]),
            )
        }
        ConnectorsCmd::RevokeClient { id, yes } => {
            confirm(g, yes, &format!("Revoke OAuth client {id} and all its connections?"))?;
            let v = api.post(&format!("/api/v1/oauth/clients/{}/revoke", enc(&id)), &json!({})).await?;
            show(g, &v, None)
        }
    }
}

// ------------------------------------------------------------------ settings and tenancy

#[derive(Subcommand, Debug)]
pub enum SettingsCmd {
    /// Workspace settings and instance facts.
    List,
    /// Set a setting.
    Set {
        key: String,
        value: String,
        #[arg(long)]
        yes: bool,
    },
    /// Reset a setting to its default.
    Unset {
        key: String,
        #[arg(long)]
        yes: bool,
    },
    /// Server settings: the public address and the effective forge.toml
    /// values (`system.update` on the platform).
    Server,
    /// The public address links use (alert emails, webhooks, approvals).
    #[command(subcommand)]
    PublicAddress(PublicAddressCmd),
}

#[derive(Subcommand, Debug)]
pub enum PublicAddressCmd {
    /// Shows the address links use, the setting and the forge.toml fallback.
    Get,
    /// Sets it: an http(s) origin such as `https://forge.example.com` (no path).
    /// Needs an MFA-verified session.
    Set {
        url: String,
        #[arg(long)]
        yes: bool,
    },
    /// Clears the setting; links use `public_url` from forge.toml.
    Clear {
        #[arg(long)]
        yes: bool,
    },
}

const ADDRESS_COLUMNS: &[(&str, &str)] =
    &[("EFFECTIVE", "/effective"), ("SOURCE", "/source"), ("SETTING", "/setting"), ("CONFIGURED", "/configured")];

pub async fn settings(g: &Global, cmd: SettingsCmd) -> Result<()> {
    let t = target(g)?;
    let mut api = connect(g, &t).await?;

    match cmd {
        SettingsCmd::List => {
            let v = api.get("/api/v1/settings").await?;

            match g.output {
                out::Format::Table => show(g, &rows(&v, "/values"), Some(&[("KEY", "/key"), ("VALUE", "/value")])),
                _ => show(g, &v, None),
            }
        }
        SettingsCmd::Set { key, value, yes } => {
            confirm(g, yes, &format!("Set {key} = {value} on {}?", t.server))?;
            let v = api
                .call(Method::PUT, &format!("/api/v1/settings/{}", enc(&key)), Some(&json!({ "value": value })))
                .await?;
            show(g, &v, None)
        }
        SettingsCmd::Unset { key, yes } => {
            confirm(g, yes, &format!("Reset {key} to its default on {}?", t.server))?;
            let v = api.call(Method::DELETE, &format!("/api/v1/settings/{}", enc(&key)), None).await?;
            show(g, &v, None)
        }
        SettingsCmd::Server => {
            let v = api.get("/api/v1/server/settings").await?;
            show(g, &v, None)
        }
        SettingsCmd::PublicAddress(PublicAddressCmd::Get) => {
            let v = api.get("/api/v1/server/settings").await?;
            show(g, &v["public_address"], Some(ADDRESS_COLUMNS))
        }
        SettingsCmd::PublicAddress(PublicAddressCmd::Set { url, yes }) => {
            confirm(g, yes, &format!("Set the public address of {} to {url}?", t.server))?;
            let v = api.call(Method::PUT, "/api/v1/server/public-address", Some(&json!({ "value": url }))).await?;
            show(g, &v, Some(ADDRESS_COLUMNS))
        }
        SettingsCmd::PublicAddress(PublicAddressCmd::Clear { yes }) => {
            confirm(g, yes, &format!("Clear the public address of {} (links use public_url)?", t.server))?;
            let v = api.call(Method::DELETE, "/api/v1/server/public-address", None).await?;
            show(g, &v, Some(ADDRESS_COLUMNS))
        }
    }
}

#[derive(Subcommand, Debug)]
pub enum TenantsCmd {
    /// Tenants in the workspace.
    List,
    /// Create a tenant.
    Create { name: String },
}

pub async fn tenants(g: &Global, cmd: TenantsCmd) -> Result<()> {
    let t = target(g)?;
    let mut api = connect(g, &t).await?;

    match cmd {
        TenantsCmd::List => {
            let v = api.get("/api/v1/tenants").await?;
            show(g, &v, Some(&[("ID", "/id"), ("NAME", "/name"), ("CREATED", "/created_at")]))
        }
        TenantsCmd::Create { name } => {
            let v = api.post("/api/v1/tenants", &json!({ "name": name })).await?;
            show(g, &v, None)
        }
    }
}

#[derive(Subcommand, Debug)]
pub enum EnvironmentsCmd {
    /// Environments you can see.
    List {
        /// Only environments in this cluster (id, name or abbreviation).
        #[arg(long)]
        cluster: Option<String>,
        /// Only the environments of this project (id or name).
        #[arg(long)]
        project: Option<String>,
        #[command(flatten)]
        paging: out::Paging,
    },
    /// Create an environment of a project in a cluster.
    Create {
        name: String,
        /// Cluster (id or name).
        #[arg(long)]
        cluster: String,
        /// Project the environment belongs to (id or name).
        #[arg(long)]
        project: String,
        /// Protect it from deletion.
        #[arg(long)]
        protected: bool,
    },
    /// Move an environment to another cluster.
    Move {
        environment: String,
        /// Target cluster (id or name).
        cluster: String,
        #[arg(long)]
        yes: bool,
    },
}

pub async fn environments(g: &Global, cmd: EnvironmentsCmd) -> Result<()> {
    let t = target(g)?;
    let mut api = connect(g, &t).await?;

    match cmd {
        EnvironmentsCmd::List { cluster, project, paging } => {
            let mut query = Vec::new();

            if let Some(c) = cluster {
                query.push(format!("cluster_id={}", cluster_id(&mut api, &c).await?));
            }

            if let Some(a) = project {
                query.push(format!("project_id={}", project_id(&mut api, &a).await?));
            }

            let path = if query.is_empty() {
                "/api/v1/environments".to_owned()
            } else {
                format!("/api/v1/environments?{}", query.join("&"))
            };

            let v = api.get(&path).await?;
            out::list_value(
                g.output,
                v,
                "",
                Some(&[
                    ("ID", "/id"),
                    ("NAME", "/name"),
                    ("PROJECT", "/project_id"),
                    ("CLUSTER", "/cluster_id"),
                    ("PROTECTED", "/protected"),
                ]),
                paging,
            );
            Ok(())
        }
        EnvironmentsCmd::Create { name, cluster, project, protected } => {
            let cid = cluster_id(&mut api, &cluster).await?;
            let gid = project_id(&mut api, &project).await?;
            let v = api
                .post(
                    "/api/v1/environments",
                    &json!({ "name": name, "cluster_id": cid, "project_id": gid, "protected": protected }),
                )
                .await?;
            show(g, &v, None)
        }
        EnvironmentsCmd::Move { environment, cluster, yes } => {
            let ns =
                id_of(&resolve(&mut api, "/api/v1/environments", "", &["name"], "environment", &environment).await?);
            let cid = cluster_id(&mut api, &cluster).await?;
            confirm(g, yes, &format!("Move environment {environment} to cluster {cluster}?"))?;
            let v = api
                .call(Method::PUT, &format!("/api/v1/environments/{ns}/cluster"), Some(&json!({ "cluster_id": cid })))
                .await?;
            show(g, &v, None)
        }
    }
}

// ------------------------------------------------------------------ clusters (writes)

#[derive(Subcommand, Debug)]
pub enum ClusterAdminCmd {
    /// Create a cluster from a JSON spec (`--file spec.json`, `@-` for stdin).
    Create {
        #[arg(long)]
        file: String,
    },
    /// Update a cluster from a JSON spec.
    Update {
        cluster: String,
        #[arg(long)]
        file: String,
        #[arg(long)]
        yes: bool,
    },
    /// Delete a cluster.
    Delete {
        cluster: String,
        #[arg(long)]
        yes: bool,
    },
    /// Live state: nodes, ingress, fabric.
    Live { cluster: String },
    /// Routing table (ingress and fabric).
    Routing { cluster: String },
}

pub async fn cluster_admin(g: &Global, cmd: ClusterAdminCmd) -> Result<()> {
    let t = target(g)?;
    let mut api = connect(g, &t).await?;

    match cmd {
        ClusterAdminCmd::Create { file } => {
            let body = file_body(&mut api, "POST", "/api/v1/clusters", &file).await?;
            let v = api.post("/api/v1/clusters", &body).await?;
            show(g, &v, None)
        }
        ClusterAdminCmd::Update { cluster, file, yes } => {
            let id = cluster_id(&mut api, &cluster).await?;
            let body = file_body(&mut api, "PUT", &format!("/api/v1/clusters/{id}"), &file).await?;
            confirm(g, yes, &format!("Update cluster {cluster}?"))?;
            let v = api.call(Method::PUT, &format!("/api/v1/clusters/{id}"), Some(&body)).await?;
            show(g, &v, None)
        }
        ClusterAdminCmd::Delete { cluster, yes } => {
            let id = cluster_id(&mut api, &cluster).await?;
            confirm(g, yes, &format!("Delete cluster {cluster} on {}?", t.server))?;
            let v = api.call(Method::DELETE, &format!("/api/v1/clusters/{id}"), None).await?;
            show(g, &v, None)
        }
        ClusterAdminCmd::Live { cluster } => {
            let id = cluster_id(&mut api, &cluster).await?;
            let v = api.get(&format!("/api/v1/clusters/{id}/live")).await?;
            show(g, &v, None)
        }
        ClusterAdminCmd::Routing { cluster } => {
            let id = cluster_id(&mut api, &cluster).await?;
            let v = api.get(&format!("/api/v1/clusters/{id}/routing")).await?;

            if g.output.is_machine() {
                return show(g, &v, None);
            }

            let rows: Vec<Value> = v["endpoints"].as_array().into_iter().flatten().map(routing_row).collect();
            let mut text = out::render_human(
                &Value::Array(rows),
                Some(&[
                    ("SITE", "/name"),
                    ("DOMAINS", "/domains"),
                    ("TLS", "/tls"),
                    ("SOURCE", "/source"),
                    ("EDGES", "/edges"),
                    ("UPSTREAMS", "/upstreams"),
                ]),
            );

            if v["profile"] == false {
                text.push_str(
                    "no ingress profile on this cluster: only sites with explicit edges on its nodes are listed\n",
                );
            }

            if text.lines().count() <= 1 {
                text = "no sites route through this cluster\n".to_owned();
            }

            out::emit(&text);
            Ok(())
        }
    }
}

/// One routing endpoint as a table row. `SOURCE` says `profile` (linked to
/// the cluster's ingress profile) or `edges (not linked)` for sites with
/// explicit edge nodes.
fn routing_row(e: &Value) -> Value {
    let names = |list: &Value, f: &dyn Fn(&Value) -> String| -> String {
        list.as_array().into_iter().flatten().map(f).collect::<Vec<_>>().join(", ")
    };
    let edges = names(&e["edges"], &|x| {
        x["name"].as_str().or_else(|| x["server_id"].as_str()).or_else(|| x.as_str()).unwrap_or("?").to_owned()
    });
    let ups = names(&e["targets"], &|t| {
        names(&t["upstreams"], &|u| {
            let who = u["node"].as_str().map(|n| format!(" ({n})")).unwrap_or_default();
            format!("{}{who}", u["address"].as_str().unwrap_or("?"))
        })
    });

    let source = match (e["source"].as_str(), e["linked"].as_bool()) {
        (Some("edges"), _) | (_, Some(false)) => "edges (not linked)".to_owned(),
        (Some(s), _) => s.to_owned(),
        (None, _) => "profile".to_owned(),
    };

    json!({
        "name": e["name"],
        "domains": e["domains"],
        "tls": e["tls"],
        "source": source,
        "edges": edges,
        "upstreams": ups,
    })
}

// ------------------------------------------------------------------ sites and certificates

#[derive(Subcommand, Debug)]
pub enum SitesCmd {
    /// Sites (domains routed through the cluster ingress).
    List {
        #[command(flatten)]
        paging: out::Paging,
    },
    /// One site.
    Get { site: String },
    /// Create a site from a JSON spec (`--file site.json`, `@-` for stdin).
    Create {
        #[arg(long)]
        file: String,
    },
    /// Replace a site's spec.
    Update {
        site: String,
        #[arg(long)]
        file: String,
        #[arg(long)]
        yes: bool,
    },
    /// Render a spec without saving it: the Nginx config the edges would
    /// run (only the variant in use: `http` when `tls` is off) and the
    /// resolved upstreams. `-o json` gives the full answer.
    Preview {
        #[arg(long)]
        file: String,
    },
    /// Push the site to the ingress nodes (reloads Nginx on each edge).
    Apply {
        site: String,
        #[arg(long)]
        yes: bool,
    },
    /// Remove the site from the ingress nodes (keeps the definition).
    Remove {
        site: String,
        #[arg(long)]
        yes: bool,
    },
    /// Renew the site's certificate now.
    Renew {
        site: String,
        #[arg(long)]
        yes: bool,
    },
    /// Delete a site.
    Delete {
        site: String,
        #[arg(long)]
        yes: bool,
    },
}

/// Shows a site operation's per-edge result; exit 10 when an edge failed.
fn site_result(g: &Global, what: &str, site: &str, v: &Value) -> Result<()> {
    let failed: Vec<String> = v["edges"]
        .as_array()
        .into_iter()
        .flatten()
        // Each edge is `{server_id, result: {job_id, status, error}}`.
        .filter(|e| e["result"]["status"] == "failed" || e["status"] == "failed")
        .map(|e| {
            format!(
                "{}: {}",
                e["server_id"].as_str().unwrap_or("?"),
                e["result"]["error"].as_str().or_else(|| e["error"].as_str()).unwrap_or("failed")
            )
        })
        .collect();
    let not_ok = v["ok"] == false;
    if failed.is_empty() && !not_ok {
        return show(g, v, None);
    }

    if g.output == out::Format::Table {
        show(g, v, None)?;
    }

    let why = if failed.is_empty() { v["error"].as_str().unwrap_or("failed").to_owned() } else { failed.join("; ") };

    Err(CliError::new(Exit::JobFailed, format!("{what} of site {site} failed: {why}")).with_code("site_failed"))
}

/// `forge sites preview` for people: the config variant the edges will run,
/// as plain text, then the resolved upstreams.
fn preview_text(input: &Value, v: &Value) -> String {
    let tls_off =
        input.pointer("/spec/tls").or_else(|| input.get("tls")).and_then(Value::as_str).is_none_or(|t| t == "off");
    let variant =
        v["variant"].as_str().map_or_else(|| if tls_off { "http" } else { "https" }.to_owned(), str::to_owned);
    let config = v["config"].as_str().or_else(|| v[variant.as_str()].as_str()).unwrap_or_default();
    let mut out = format!("# Nginx config ({variant}{})\n", if variant == "http" { ", tls off" } else { "" });
    out.push_str(config.trim_end());
    out.push('\n');

    if let Some(custom) = v["custom"].as_str().filter(|c| !c.trim().is_empty()) {
        out.push_str("\n# Custom config\n");
        out.push_str(custom.trim_end());
        out.push('\n');
    }

    let mut lines = Vec::new();

    for t in v["targets"].as_array().into_iter().flatten() {
        let target = match &t["target"] {
            Value::Object(m) => m
                .iter()
                .map(|(k, x)| format!("{k}={}", x.as_str().map_or_else(|| x.to_string(), str::to_owned)))
                .collect::<Vec<_>>()
                .join(" "),
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };

        let ups: Vec<String> = t["upstreams"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|u| {
                let mut s = u["address"].as_str().unwrap_or("?").to_owned();
                if let Some(st) = u["state"].as_str().filter(|s| !s.is_empty()) {
                    s.push_str(&format!(" ({st})"));
                }

                s
            })
            .collect();
        let standby = if t["standby"] == true { " [standby]" } else { "" };
        lines.push(format!(
            "  {target}{standby} -> {}",
            if ups.is_empty() { "(none)".to_owned() } else { ups.join(", ") }
        ));
    }

    if lines.is_empty() {
        for (list, standby) in [(&v["upstreams"], ""), (&v["standby"], " [standby]")] {
            for u in list.as_array().into_iter().flatten() {
                let a = u
                    .as_str()
                    .map(str::to_owned)
                    .or_else(|| u["address"].as_str().map(str::to_owned))
                    .unwrap_or_else(|| u.to_string());
                lines.push(format!("  {a}{standby}"));
            }
        }
    }

    out.push_str("\n# Upstreams\n");

    if lines.is_empty() {
        out.push_str("  (none)\n");
    }

    for l in lines {
        out.push_str(&l);
        out.push('\n');
    }

    out
}

async fn site_id(api: &mut Api, s: &str) -> Result<String> {
    Ok(id_of(&resolve(api, "/api/v1/sites", "", &["name"], "site", s).await?))
}

pub async fn sites(g: &Global, cmd: SitesCmd) -> Result<()> {
    let t = target(g)?;
    let mut api = connect(g, &t).await?;

    match cmd {
        SitesCmd::List { paging } => {
            let v = api.get("/api/v1/sites").await?;
            out::list_value(
                g.output,
                v,
                "",
                Some(&[("ID", "/id"), ("NAME", "/name"), ("UPDATED", "/updated_at")]),
                paging,
            );
            Ok(())
        }
        SitesCmd::Get { site } => {
            let id = site_id(&mut api, &site).await?;
            show(g, &api.get(&format!("/api/v1/sites/{id}")).await?, None)
        }
        SitesCmd::Create { file } => {
            let body = file_body(&mut api, "POST", "/api/v1/sites", &file).await?;
            show(g, &api.post("/api/v1/sites", &body).await?, None)
        }
        SitesCmd::Update { site, file, yes } => {
            let id = site_id(&mut api, &site).await?;
            let body = file_body(&mut api, "PUT", &format!("/api/v1/sites/{id}"), &file).await?;
            confirm(g, yes, &format!("Replace the spec of site {site}?"))?;
            show(g, &api.call(Method::PUT, &format!("/api/v1/sites/{id}"), Some(&body)).await?, None)
        }
        SitesCmd::Preview { file } => {
            let body = file_body(&mut api, "POST", "/api/v1/sites/preview", &file).await?;
            let v = api.post("/api/v1/sites/preview", &body).await?;

            if g.output.is_machine() {
                return show(g, &v, None);
            }

            out::emit(&preview_text(&body, &v));
            Ok(())
        }
        SitesCmd::Apply { site, yes } => {
            let id = site_id(&mut api, &site).await?;
            confirm(
                g,
                yes,
                &format!("Push site {site} to its ingress nodes ({})? Nginx reloads on each edge.", t.server),
            )?;
            let v = api.post(&format!("/api/v1/sites/{id}/apply"), &json!({})).await?;
            site_result(g, "apply", &site, &v)
        }
        SitesCmd::Remove { site, yes } => {
            let id = site_id(&mut api, &site).await?;
            confirm(g, yes, &format!("Take site {site} off the ingress nodes?"))?;
            let v = api.post(&format!("/api/v1/sites/{id}/remove"), &json!({})).await?;
            site_result(g, "remove", &site, &v)
        }
        SitesCmd::Renew { site, yes } => {
            let id = site_id(&mut api, &site).await?;
            confirm(g, yes, &format!("Renew the certificate of site {site} now ({})?", t.server))?;
            let v = api.post(&format!("/api/v1/sites/{id}/renew"), &json!({})).await?;
            site_result(g, "renew", &site, &v)
        }
        SitesCmd::Delete { site, yes } => {
            let id = site_id(&mut api, &site).await?;
            confirm(g, yes, &format!("Delete site {site}?"))?;
            show(g, &api.call(Method::DELETE, &format!("/api/v1/sites/{id}"), None).await?, None)
        }
    }
}

#[derive(Subcommand, Debug)]
pub enum CertificatesCmd {
    /// Uploaded and issued certificates.
    List {
        #[command(flatten)]
        paging: out::Paging,
    },
    /// Upload a certificate from a JSON spec (`--file cert.json`; holds the PEMs).
    Create {
        #[arg(long)]
        file: String,
    },
    /// Delete a certificate.
    Delete {
        id: String,
        #[arg(long)]
        yes: bool,
    },
}

pub async fn certificates(g: &Global, cmd: CertificatesCmd) -> Result<()> {
    let t = target(g)?;
    let mut api = connect(g, &t).await?;

    match cmd {
        CertificatesCmd::List { paging } => {
            let v = api.get("/api/v1/certificates").await?;
            out::list_value(
                g.output,
                v,
                "",
                Some(&[
                    ("ID", "/id"),
                    ("NAME", "/name"),
                    ("DOMAINS", "/domains"),
                    ("EXPIRES", "/not_after"),
                    ("ISSUER", "/issuer"),
                ]),
                paging,
            );
            Ok(())
        }
        CertificatesCmd::Create { file } => {
            let body = file_body(&mut api, "POST", "/api/v1/certificates", &file).await?;
            show(g, &api.post("/api/v1/certificates", &body).await?, None)
        }
        CertificatesCmd::Delete { id, yes } => {
            confirm(g, yes, &format!("Delete certificate {id}?"))?;
            show(g, &api.call(Method::DELETE, &format!("/api/v1/certificates/{}", enc(&id)), None).await?, None)
        }
    }
}

// ------------------------------------------------------------------ vault

#[derive(Subcommand, Debug)]
pub enum VaultCmd {
    /// Vault items (names and usernames; never passwords).
    List {
        #[command(flatten)]
        paging: out::Paging,
    },
    /// Store a password, read from stdin.
    Add {
        name: String,
        #[arg(long, default_value = "")]
        username: String,
        #[arg(long, default_value = "")]
        url: String,
        #[arg(long, default_value = "")]
        notes: String,
        /// Read the password from stdin (required).
        #[arg(long, required = true)]
        password_stdin: bool,
    },
    /// Change an item; with --password-stdin the password is rotated.
    Update {
        item: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        username: Option<String>,
        #[arg(long)]
        url: Option<String>,
        #[arg(long)]
        notes: Option<String>,
        #[arg(long)]
        password_stdin: bool,
    },
    /// Print an item's password (platform admin, MFA; never with an API key).
    Reveal { item: String },
    /// Delete an item.
    Delete {
        item: String,
        #[arg(long)]
        yes: bool,
    },
}

pub async fn vault(g: &Global, cmd: VaultCmd) -> Result<()> {
    let t = target(g)?;
    let mut api = connect(g, &t).await?;
    let base = "/api/v1/vault/items";

    match cmd {
        VaultCmd::List { paging } => {
            let v = api.get(base).await?;
            out::list_value(
                g.output,
                v,
                "",
                Some(&[
                    ("ID", "/id"),
                    ("NAME", "/name"),
                    ("USERNAME", "/username"),
                    ("URL", "/url"),
                    ("UPDATED", "/updated_at"),
                ]),
                paging,
            );
            Ok(())
        }
        VaultCmd::Add { name, username, url, notes, password_stdin: _ } => {
            let password = secret_from_stdin("password")?;
            let body = json!({ "name": name, "username": username, "url": url, "notes": notes, "password": password });
            show(g, &api.post(base, &body).await?, None)
        }
        VaultCmd::Update { item, name, username, url, notes, password_stdin } => {
            let cur = resolve(&mut api, base, "", &["name"], "vault item", &item).await?;
            let password = if password_stdin { Some(secret_from_stdin("password")?) } else { None };
            let pick = |new: Option<String>, key: &str| {
                new.unwrap_or_else(|| cur[key].as_str().unwrap_or_default().to_owned())
            };
            let body = json!({
                "name": pick(name, "name"),
                "username": pick(username, "username"),
                "url": pick(url, "url"),
                "notes": pick(notes, "notes"),
                "password": password,
                // Unchanged: an update without them would clear them (object
                // store keys find their vault item by its tags).
                "tags": cur["tags"].clone(),
                "links": cur["links"].as_array().into_iter().flatten()
                    .map(|l| json!({ "kind": l["kind"], "id": l["id"] }))
                    .collect::<Vec<_>>(),
            });
            show(g, &api.call(Method::PUT, &format!("{base}/{}", id_of(&cur)), Some(&body)).await?, None)
        }
        VaultCmd::Reveal { item } => {
            let id = id_of(&resolve(&mut api, base, "", &["name"], "vault item", &item).await?);
            let v = api.post(&format!("{base}/{id}/reveal"), &json!({})).await?;
            match (g.output, v["password"].as_str()) {
                (out::Format::Table, Some(p)) => {
                    println!("{p}");
                    Ok(())
                }
                _ => show(g, &v, None),
            }
        }
        VaultCmd::Delete { item, yes } => {
            let id = id_of(&resolve(&mut api, base, "", &["name"], "vault item", &item).await?);
            confirm(g, yes, &format!("Delete vault item {item}?"))?;
            show(g, &api.call(Method::DELETE, &format!("{base}/{id}"), None).await?, None)
        }
    }
}

// ------------------------------------------------------------------ system update

#[derive(Subcommand, Debug)]
pub enum SystemCmd {
    /// Server version and whether an update is available.
    Update,
    /// Ask the release feed for a newer server version now.
    Check,
    /// Releases you can upgrade to, newest first (the running one marked).
    Releases,
    /// Install a server update (backs up the database first). Upgrades only.
    #[command(disable_version_flag = true)]
    Apply {
        /// The version to install: any listed release newer than the
        /// running one (default: the latest).
        #[arg(long, alias = "to")]
        version: Option<String>,
        /// After the server restarts, update the agents to it in canary
        /// batches (`rollout`), or leave them (`none`).
        #[arg(long, value_parser = ["rollout", "none"], default_value = "rollout")]
        agents: String,
        /// Nodes per batch for the agent rollout (1-100).
        #[arg(long, default_value_t = 1)]
        batch: usize,
        #[arg(long)]
        yes: bool,
    },
}

pub async fn system(g: &Global, cmd: SystemCmd) -> Result<()> {
    let t = target(g)?;
    let mut api = connect(g, &t).await?;

    match cmd {
        SystemCmd::Update => show(g, &api.get("/api/v1/system/update").await?, None),
        SystemCmd::Check => show(g, &api.post("/api/v1/system/update/check", &json!({})).await?, None),
        SystemCmd::Releases => {
            let v = api.get("/api/v1/system/update/releases").await?;
            show(
                g,
                &v["releases"],
                Some(&[("VERSION", "/version"), ("PUBLISHED", "/published_at"), ("CURRENT", "/current")]),
            )
        }
        SystemCmd::Apply { version, agents, batch, yes } => {
            let version = match version {
                Some(v) => v,
                None => {
                    let r = api.get("/api/v1/system/update/releases").await?;
                    r["releases"]
                        .as_array()
                        .and_then(|l| l.first())
                        .filter(|x| x["current"].as_bool() != Some(true))
                        .and_then(|x| x["version"].as_str())
                        .map(str::to_owned)
                        .ok_or_else(|| {
                            CliError::new(Exit::Conflict, "no server update is available").with_code("up_to_date")
                        })?
                }
            };

            let then = if agents == "rollout" { ", then its agents follow" } else { "" };
            confirm(g, yes, &format!("Update the Forge server at {} to {version}{then}? It restarts.", t.server))?;
            let body = json!({ "version": version, "agents": agents, "batch": batch });
            show(g, &api.post("/api/v1/system/update/apply", &body).await?, None)
        }
    }
}

// ------------------------------------------------------------------ packages

#[derive(Subcommand, Debug)]
pub enum PackagesCmd {
    /// The App Store catalog.
    Catalog {
        #[command(flatten)]
        paging: out::Paging,
    },
    /// Installed packages on nodes.
    Installed {
        /// Only this node's installations (id, hostname or display name).
        #[arg(long)]
        node: Option<String>,
        /// Only this package (catalog id, such as `postgres`).
        #[arg(long)]
        package: Option<String>,
        #[command(flatten)]
        paging: out::Paging,
    },
    /// Install a package from a JSON spec (`--file install.json`).
    Install {
        #[arg(long)]
        file: String,
    },
    /// Updates available for installed packages.
    Updates {
        #[command(flatten)]
        paging: out::Paging,
    },
    /// Check nodes for package updates now.
    Check,
    /// Update one installation to its candidate build (`forge packages updates`).
    Update {
        installation: String,
        /// The build to install (default: the candidate the node reported).
        #[arg(long)]
        build: Option<String>,
        #[arg(long)]
        yes: bool,
    },
    /// Set an installation's update policy: notify or auto_patch.
    Policy {
        installation: String,
        policy: String,
        #[arg(long)]
        yes: bool,
    },
    /// Run an action on an installation: start, stop, restart, uninstall, …
    Action {
        installation: String,
        action: String,
        #[arg(long)]
        yes: bool,
    },
    /// Upstream release lines and end-of-life dates.
    Releases,
}

/// `forge packages installed`. `STATUS` is the lifecycle (`installed`,
/// `failed`, `installing`, ...); `STATE` is the service state the last job
/// reported (`running`, `stopped`; `-` until a job reports one).
const INSTALLED_COLS: &[Col] = &[
    ("ID", "/id"),
    ("PACKAGE", "/package"),
    ("NODE", "/node"),
    ("VERSION", "/version"),
    ("STATUS", "/status"),
    ("STATE", "/state"),
    ("POLICY", "/update_policy"),
];

pub async fn packages(g: &Global, cmd: PackagesCmd) -> Result<()> {
    let t = target(g)?;
    let mut api = connect(g, &t).await?;

    match cmd {
        PackagesCmd::Catalog { paging } => {
            let v = api.get("/api/v1/catalog").await?;
            out::list_value(
                g.output,
                v,
                "/packages",
                Some(&[
                    ("ID", "/id"),
                    ("NAME", "/name"),
                    ("CATEGORY", "/category"),
                    ("KIND", "/kind"),
                    ("OS", "/os"),
                    ("SUMMARY", "/summary"),
                ]),
                paging,
            );
            Ok(())
        }
        PackagesCmd::Installed { node, package, paging } => {
            let path = match &node {
                Some(n) => format!("/api/v1/installations?server_id={}", node_id(&mut api, n).await?),
                None => "/api/v1/installations".to_owned(),
            };

            let mut rows = crate::cmds::rows_of(api.get(&path).await?);

            if let Some(p) = &package {
                rows.retain(|r| r["package"].as_str().is_some_and(|x| x.eq_ignore_ascii_case(p)));
            }

            add_node_names(&mut api, &mut rows, "server_id").await;
            out::list(g.output, rows, Some(INSTALLED_COLS), paging.into(), out::Total::Rows);
            Ok(())
        }
        PackagesCmd::Install { file } => {
            let body = file_body(&mut api, "POST", "/api/v1/packages/install", &file).await?;
            show(g, &api.post("/api/v1/packages/install", &body).await?, None)
        }
        PackagesCmd::Updates { paging } => {
            let v = api.get("/api/v1/package-updates").await?;
            out::list_value(
                g.output,
                v,
                "",
                Some(&[
                    ("INSTALLATION", "/installation_id"),
                    ("PACKAGE", "/package"),
                    ("INSTALLED", "/installed"),
                    ("CANDIDATE", "/candidate"),
                    ("STATE", "/state"),
                ]),
                paging,
            );
            Ok(())
        }
        PackagesCmd::Check => show(g, &api.post("/api/v1/package-updates/check", &json!({})).await?, None),
        PackagesCmd::Update { installation, build, yes } => {
            // The server installs exactly the build the node reported as the
            // candidate (`forge packages updates`), unless --build names one.
            let build = match build {
                Some(b) => b,
                None => {
                    let ups = api.get("/api/v1/package-updates").await?;
                    ups.as_array()
                        .into_iter()
                        .flatten()
                        .find(|u| u["installation_id"].as_str() == Some(installation.as_str()))
                        .and_then(|u| u["candidate"].as_str().filter(|c| !c.is_empty()).map(str::to_owned))
                        .ok_or_else(|| {
                            CliError::new(
                                Exit::NotFound,
                                format!("no update candidate for installation {installation}: run `forge packages check` first"),
                            )
                            .with_code("no_candidate")
                        })?
                }
            };

            confirm(g, yes, &format!("Update installation {installation} to {build}?"))?;
            let v = api
                .post(&format!("/api/v1/installations/{}/update", enc(&installation)), &json!({ "build": build }))
                .await?;
            show(g, &v, None)
        }
        PackagesCmd::Policy { installation, policy, yes } => {
            confirm(g, yes, &format!("Set the update policy of installation {installation} to {policy}?"))?;
            show(
                g,
                &api.call(
                    Method::PUT,
                    &format!("/api/v1/installations/{}/policy", enc(&installation)),
                    Some(&json!({ "update_policy": policy })),
                )
                .await?,
                None,
            )
        }
        PackagesCmd::Action { installation, action, yes } => {
            confirm(g, yes, &format!("Run `{action}` on installation {installation}?"))?;
            let v = api
                .post(&format!("/api/v1/installations/{}/actions", enc(&installation)), &json!({ "action": action }))
                .await?;
            show(g, &v, None)
        }
        PackagesCmd::Releases => {
            let v = api.get("/api/v1/package-releases").await?;
            show(
                g,
                &v,
                Some(&[
                    ("PRODUCT", "/product"),
                    ("LINE", "/line"),
                    ("LATEST", "/latest"),
                    ("LTS", "/lts"),
                    ("EOL", "/eol"),
                ]),
            )
        }
    }
}

// ------------------------------------------------------------------ telemetry

/// Latest metrics of every node, one row each.
pub async fn metrics(g: &Global, node: Option<String>) -> Result<()> {
    let t = target(g)?;
    let mut api = connect(g, &t).await?;

    if let Some(n) = node {
        let id = node_id(&mut api, &n).await?;
        return show(g, &api.get(&format!("/api/v1/servers/{id}/metrics")).await?, None);
    }

    let v = api.get("/api/v1/metrics/latest").await?;

    if g.output.is_machine() {
        return show(g, &v, None);
    }

    let names: std::collections::HashMap<String, String> = api
        .get("/api/v1/servers")
        .await?
        .as_array()
        .into_iter()
        .flatten()
        .map(|s| (id_of(s), s["hostname"].as_str().unwrap_or("?").to_owned()))
        .collect();

    let pct = |used: &Value, total: &Value| match (used.as_f64(), total.as_f64()) {
        (Some(u), Some(t)) if t > 0.0 => json!(format!("{:.0}%", u * 100.0 / t)),
        _ => Value::Null,
    };

    let list: Vec<Value> = v
        .as_object()
        .into_iter()
        .flatten()
        .map(|(id, m)| {
            json!({
                "node": names.get(id).cloned().unwrap_or_else(|| id.clone()),
                "cpu": m["cpu"].as_f64().map(|c| format!("{c:.0}%")),
                "load1": m["load1"],
                "mem": pct(&m["mem_used"], &m["mem_total"]),
                "disk": pct(&m["disk_used"], &m["disk_total"]),
            })
        })
        .collect();

    show(
        g,
        &Value::Array(list),
        Some(&[("NODE", "/node"), ("CPU", "/cpu"), ("LOAD", "/load1"), ("MEM", "/mem"), ("DISK", "/disk")]),
    )
}

/// Subjects and prefixes are lowercase dotted names (D-085); anything else
/// can't match, so it is refused here instead of being sent.
fn check_subject_text(s: &str, what: &str) -> Result<()> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b)) {
        return Err(CliError::usage(format!("{what} `{s}` must use lowercase letters, digits, `.`, `-` and `_`")));
    }

    Ok(())
}

/// Health observations of a node (`GET /api/v1/observations[/history]`).
pub async fn health(
    g: &Global,
    node: String,
    prefix: Option<String>,
    history: Option<String>,
    limit: u32,
) -> Result<()> {
    let t = target(g)?;
    let mut api = connect(g, &t).await?;
    let id = node_id(&mut api, &node).await?;

    if let Some(subject) = history {
        check_subject_text(&subject, "subject")?;
        let v = api
            .get(&format!("/api/v1/observations/history?resource=server:{id}&subject={subject}&limit={limit}"))
            .await?;

        if g.output.is_machine() {
            return show(g, &v, None);
        }

        let rows: Vec<Value> = v["entries"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|e| {
                json!({
                    "at": e["received_at"],
                    "state": value_text(&e["value"]),
                    "reason": e["reason"],
                    "detail": e["detail"],
                })
            })
            .collect();

        return show(
            g,
            &Value::Array(rows),
            Some(&[("RECEIVED (MS)", "/at"), ("STATE", "/state"), ("REASON", "/reason"), ("DETAIL", "/detail")]),
        );
    }

    let mut path = format!("/api/v1/observations?resource=server:{id}&limit={limit}");

    if let Some(p) = prefix {
        check_subject_text(&p, "prefix")?;
        path.push_str(&format!("&subject_prefix={p}"));
    }

    let v = api.get(&path).await?;

    if g.output.is_machine() {
        return show(g, &v, None);
    }

    let rows: Vec<Value> = v["observations"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|o| {
            json!({
                "subject": o["key"]["subject"],
                "state": value_text(&o["effective"]),
                "last": value_text(&o["value"]),
                "reason": o["reason"],
                "stale": if o["stale"] == true { "yes" } else { "no" },
            })
        })
        .collect();

    show(
        g,
        &Value::Array(rows),
        Some(&[
            ("SUBJECT", "/subject"),
            ("STATE", "/state"),
            ("LAST KNOWN", "/last"),
            ("REASON", "/reason"),
            ("STALE", "/stale"),
        ]),
    )
}

// ------------------------------------------------------------------ node extras

#[derive(Subcommand, Debug)]
pub enum NodeExtraCmd {
    /// Create a one-time enrollment token for a new node; printed once.
    EnrollToken {
        /// Cluster the node joins (id or name).
        #[arg(long)]
        cluster: Option<String>,
        /// Expected host name, shown until the node enrolls.
        #[arg(long)]
        hostname: Option<String>,
        /// Validity in minutes (1-1440).
        #[arg(long, default_value_t = 60)]
        ttl_minutes: u32,
    },
    /// Live metrics of one node, or the latest of every node.
    Metrics { node: Option<String> },
    /// A node's resource history from the analytics database.
    History {
        node: String,
        /// Hours back from now (1-9600). Up to 48 hours gives minute points,
        /// more gives hourly points.
        #[arg(long, default_value_t = 24)]
        hours: u32,
    },
    /// A node report: trends, peaks and the top processes by CPU and memory.
    Report {
        node: String,
        /// Hours back from now (1-9600).
        #[arg(long, default_value_t = 168)]
        hours: u32,
    },
    /// The analytics database: engine, readiness and buffered rows.
    Analytics,
    /// Health observations of a node: current values, or one check's history.
    Health {
        node: String,
        /// Only subjects starting with this, e.g. `health.readiness`.
        #[arg(long)]
        prefix: Option<String>,
        /// Show the history of this subject instead of current values.
        #[arg(long, value_name = "SUBJECT")]
        history: Option<String>,
        /// Most rows to show (1-1000).
        #[arg(long, default_value_t = 200)]
        limit: u32,
    },
    /// Top processes on a node.
    Processes {
        node: String,
        /// cpu, mem, pid, time or name (default cpu).
        #[arg(long)]
        sort: Option<String>,
        #[arg(long, default_value_t = 25)]
        limit: u32,
        /// Only processes whose command contains this text.
        #[arg(long)]
        filter: Option<String>,
    },
    /// End a process, or with --tree the process and its descendants (MFA).
    /// Linux sends a signal; Windows ends the task (term) or the process (kill).
    Kill {
        node: String,
        /// Process id, as `forge nodes processes` shows it.
        pid: u32,
        /// Linux: term, kill, hup or int. Windows: term (End task) or kill (End process).
        #[arg(long, default_value = "term")]
        signal: String,
        /// The process's descendants too, after the process itself.
        #[arg(long)]
        tree: bool,
        /// The process's start time (seconds since the epoch). Default: read
        /// from the node's process list.
        #[arg(long)]
        start_time: Option<u64>,
        /// Do not ask for confirmation.
        #[arg(long)]
        yes: bool,
    },
    /// Change a process's priority (MFA): --nice on Linux, --class on Windows.
    Priority {
        node: String,
        /// Process id, as `forge nodes processes` shows it.
        pid: u32,
        /// Linux: nice value, -20 (highest priority) to 19.
        #[arg(long, allow_hyphen_values = true, conflicts_with = "class")]
        nice: Option<i32>,
        /// Windows: idle, below_normal, normal, above_normal or high.
        #[arg(long)]
        class: Option<String>,
        /// The process's start time (seconds since the epoch). Default: read
        /// from the node's process list.
        #[arg(long)]
        start_time: Option<u64>,
    },
    /// Reachability between this node and the others.
    Connectivity {
        node: String,
        /// Probe again now instead of showing the last result.
        #[arg(long)]
        probe: bool,
    },
    /// OS logins Forge may use on the node (terminal, files).
    Logins { node: String },
    /// SSH settings of the node (host key, broker access).
    Ssh { node: String },
    /// An interactive shell on the node through Forge's SSH broker (MFA).
    Terminal {
        node: String,
        /// OS login (default: the node's first saved login, else root).
        #[arg(long, short = 'l')]
        login: Option<String>,
    },
    /// Run one command on the node without a terminal (MFA). Arguments
    /// after `--` are passed as they are; piped stdin is forwarded.
    Exec {
        node: String,
        /// OS login (default: the node's first saved login, else root).
        #[arg(long, short = 'l')]
        login: Option<String>,
        /// Run the words as one shell command line (pipes, `$VARS`) instead of an argument list.
        #[arg(long)]
        shell: bool,
        /// Exit with the remote command's status (default: 0 once it ran; the status is printed).
        #[arg(long)]
        propagate_exit_code: bool,
        /// The command and its arguments.
        #[arg(last = true, required = true)]
        argv: Vec<String>,
    },
}

/// The process's start time (given, or from the node's list) and its name.
async fn process_start(api: &mut Api, id: &str, pid: u32, given: Option<u64>) -> Result<(u64, String)> {
    if let Some(t) = given {
        return Ok((t, format!("pid {pid}")));
    }

    let v = api.get(&format!("/api/v1/servers/{id}/processes?sort=pid&limit=500")).await?;

    v["processes"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|p| p["pid"].as_u64() == Some(u64::from(pid)))
        .and_then(|p| Some((p["started"].as_u64()?, p["name"].as_str().unwrap_or("?").to_owned())))
        .ok_or_else(|| {
            CliError::new(
                Exit::NotFound,
                format!("process {pid} is not in the node's list; check the PID or pass --start-time"),
            )
            .with_code("not_found")
        })
}

/// `from=&to=` for the last `hours` hours (clamped to 1-9600).
fn window(hours: u32) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
    let from = now - i64::from(hours.clamp(1, 9600)) * 3_600_000;

    format!("from={from}&to={now}")
}

pub async fn node_extra(g: &Global, cmd: NodeExtraCmd) -> Result<()> {
    let cmd = match cmd {
        NodeExtraCmd::Metrics { node } => return metrics(g, node).await,
        NodeExtraCmd::Health { node, prefix, history, limit } => return health(g, node, prefix, history, limit).await,
        NodeExtraCmd::Terminal { node, login } => return crate::termcmd::terminal(g, &node, login).await,
        NodeExtraCmd::Exec { node, login, shell, propagate_exit_code, argv } => {
            return crate::termcmd::exec(g, &node, login, shell, propagate_exit_code, argv).await;
        }
        other => other,
    };

    let t = target(g)?;
    let mut api = connect(g, &t).await?;

    match cmd {
        NodeExtraCmd::EnrollToken { cluster, hostname, ttl_minutes } => {
            let cid = match cluster {
                Some(c) => Some(cluster_id(&mut api, &c).await?),
                None => None,
            };

            let body = json!({ "cluster_id": cid, "hostname": hostname, "ttl_minutes": ttl_minutes });
            let v = api.post("/api/v1/enrollment-tokens", &body).await?;

            if g.output == out::Format::Table {
                eprintln!("This token is shown once; it enrolls one node.");
            }

            show(g, &v, None)
        }
        NodeExtraCmd::Processes { node, sort, limit, filter } => {
            let id = node_id(&mut api, &node).await?;
            let mut q = vec![format!("limit={limit}")];

            if let Some(s) = sort {
                q.push(format!("sort={}", enc(&s)));
            }

            if let Some(f) = filter {
                q.push(format!("filter={}", enc(&f)));
            }

            let v = api.get(&format!("/api/v1/servers/{id}/processes?{}", q.join("&"))).await?;
            let list = if v.is_array() { v } else { rows(&v, "/processes") };
            show(
                g,
                &list,
                Some(&[
                    ("PID", "/pid"),
                    ("USER", "/user"),
                    ("CPU", "/cpu"),
                    ("MEM%", "/mem_pct"),
                    ("COMMAND", "/name"),
                ]),
            )
        }
        NodeExtraCmd::Kill { node, pid, signal, tree, start_time, yes } => {
            let id = node_id(&mut api, &node).await?;
            let (started, name) = process_start(&mut api, &id, pid, start_time).await?;
            let what = if tree { "and its descendants " } else { "" };
            confirm(g, yes, &format!("Send `{signal}` to process {pid} ({name}) {what}on {node}?"))?;
            let body = json!({ "start_time": started, "signal": signal, "tree": tree });
            show(g, &api.post(&format!("/api/v1/servers/{id}/processes/{pid}/signal"), &body).await?, None)
        }
        NodeExtraCmd::Priority { node, pid, nice, class, start_time } => {
            if nice.is_none() && class.is_none() {
                return Err(CliError::usage("pass --nice (Linux) or --class (Windows)"));
            }

            let id = node_id(&mut api, &node).await?;
            let (started, _) = process_start(&mut api, &id, pid, start_time).await?;
            let body = json!({ "start_time": started, "nice": nice, "class": class });
            show(g, &api.post(&format!("/api/v1/servers/{id}/processes/{pid}/priority"), &body).await?, None)
        }
        NodeExtraCmd::Connectivity { node, probe } => {
            let id = node_id(&mut api, &node).await?;
            let path = format!("/api/v1/servers/{id}/connectivity");
            let v = if probe { api.post(&path, &json!({})).await? } else { api.get(&path).await? };
            show(g, &v, None)
        }
        NodeExtraCmd::History { node, hours } => {
            let id = node_id(&mut api, &node).await?;
            show(g, &api.get(&format!("/api/v1/servers/{id}/history?{}", window(hours))).await?, None)
        }
        NodeExtraCmd::Report { node, hours } => {
            let id = node_id(&mut api, &node).await?;
            show(g, &api.get(&format!("/api/v1/servers/{id}/report?{}", window(hours))).await?, None)
        }
        NodeExtraCmd::Analytics => show(g, &api.get("/api/v1/analytics/status").await?, None),
        NodeExtraCmd::Logins { node } => {
            let id = node_id(&mut api, &node).await?;
            show(g, &api.get(&format!("/api/v1/servers/{id}/logins")).await?, None)
        }
        NodeExtraCmd::Ssh { node } => {
            let id = node_id(&mut api, &node).await?;
            show(g, &api.get(&format!("/api/v1/servers/{id}/ssh")).await?, None)
        }
        NodeExtraCmd::Metrics { .. }
        | NodeExtraCmd::Health { .. }
        | NodeExtraCmd::Terminal { .. }
        | NodeExtraCmd::Exec { .. } => Ok(()),
    }
}

/// An observation value for a table: a state as is, a measurement as a
/// number with its unit (`85.3 %`), `unknown` without a value.
fn value_text(v: &Value) -> Value {
    if v["type"] != "measure" {
        return v["state"].clone();
    }

    let Some(milli) = v["state"]["milli"].as_i64() else { return json!("unknown") };

    #[allow(clippy::cast_precision_loss)]
    let n = milli as f64 / 1000.0;
    let unit = match v["state"]["unit"].as_str() {
        Some("percent") => " %",
        Some("bytes") => " B",
        Some("seconds") => " s",
        _ => "",
    };

    json!(format!("{}{unit}", (n * 10.0).round() / 10.0))
}

#[cfg(test)]
mod value_text_tests {
    use super::*;

    #[test]
    fn measures_render_as_numbers() {
        assert_eq!(
            value_text(&json!({"type": "measure", "state": {"milli": 85_250, "unit": "percent"}})),
            json!("85.3 %")
        );
        assert_eq!(
            value_text(&json!({"type": "measure", "state": {"milli": null, "unit": "percent"}})),
            json!("unknown")
        );
        assert_eq!(value_text(&json!({"type": "health", "state": "failing"})), json!("failing"));
    }
}

#[cfg(test)]
mod package_tests {
    use super::*;

    /// Issue #64: the lifecycle status has its own column next to the service state.
    #[test]
    fn installed_packages_show_status_and_state() {
        let rows = vec![json!({ "id": "i1", "package": "docker", "node": "lab-c", "version": "stable",
                                "status": "installed", "state": "running", "update_policy": "notify" })];
        let text = out::table(&rows, INSTALLED_COLS);
        let mut lines = text.lines().map(|l| l.split_whitespace().collect::<Vec<_>>());
        assert_eq!(lines.next(), Some(vec!["ID", "PACKAGE", "NODE", "VERSION", "STATUS", "STATE", "POLICY"]));
        assert_eq!(lines.next(), Some(vec!["i1", "docker", "lab-c", "stable", "installed", "running", "notify"]));
    }
}

/// A token's `restrictions` from `--allow <kind>:<id>=<action>[,…]` flags
/// (personal and service-account tokens alike); `None` without any.
pub(crate) async fn restrictions(api: &mut Api, allow: &[String]) -> Result<Option<Value>> {
    if allow.is_empty() {
        return Ok(None);
    }

    let mut grants = Vec::new();

    for a in allow {
        let Some((scope, actions)) = a.split_once('=') else {
            return Err(crate::error::CliError::new(
                crate::error::Exit::Usage,
                format!("--allow `{a}`: expected <kind>:<id>=<action>[,<action>…]"),
            ));
        };

        let scope = match scope.split_once(':') {
            Some(("cluster", c)) => format!("cluster:{}", cluster_id(api, c).await?),
            _ => scope.to_owned(),
        };

        let actions: Vec<&str> = actions.split(',').map(str::trim).filter(|x| !x.is_empty()).collect();
        grants.push(json!({ "scope": scope, "actions": actions }));
    }

    Ok(Some(json!(grants)))
}
