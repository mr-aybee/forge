//! `forge access catalog|profiles|assignments|groups|effective` and `forge service-accounts`
//! (D-086): access profiles, their assignments and service accounts. Every
//! command calls the same API as the console; the server authorizes, asks
//! for MFA and audits.

use clap::Subcommand;
use reqwest::Method;
use serde_json::{Value, json};

use crate::Global;
use crate::admincmd::{cluster_id, enc};
use crate::api::Api;
use crate::cmds::confirm;
use crate::error::Result;
use crate::out::{self, Col};

fn show(g: &Global, v: &Value, cols: Option<&[Col]>) -> Result<()> {
    out::result(g.output, v, cols);
    Ok(())
}

/// A profile id, or a profile name looked up in the list.
async fn profile_id(api: &mut Api, p: &str) -> Result<String> {
    let list = api.get("/api/v1/access/profiles").await?;
    let found = list.as_array().into_iter().flatten().find(|x| x["id"] == p || x["name"].as_str() == Some(p));
    match found.and_then(|x| x["id"].as_str()) {
        Some(id) => Ok(id.to_owned()),
        None => Err(crate::error::CliError::new(crate::error::Exit::NotFound, format!("no access profile `{p}`"))),
    }
}

/// `cluster:<name>` becomes `cluster:<id>`; other scopes pass through.
async fn scope(api: &mut Api, s: String) -> Result<String> {
    Ok(match s.split_once(':') {
        Some(("cluster", c)) => format!("cluster:{}", cluster_id(api, c).await?),
        _ => s,
    })
}

#[derive(Subcommand, Debug)]
pub enum ProfilesCmd {
    /// Every access profile with its actions (built-ins first).
    List,
    /// One profile and every revision.
    Show {
        /// Profile id or name.
        profile: String,
    },
    /// Create a profile from a list of actions (`forge access catalog`).
    Create {
        name: String,
        /// An action; repeat for each (`--action view --action audit.read`).
        #[arg(long = "action", required = true)]
        actions: Vec<String>,
        #[arg(long, default_value = "")]
        description: String,
    },
    /// Publish a new revision with these actions; every assignment moves to it.
    Publish {
        /// Profile id or name.
        profile: String,
        /// The full new action list; repeat for each.
        #[arg(long = "action", required = true)]
        actions: Vec<String>,
        /// Show the impact only: added and removed actions, assignments, users,
        /// and scopes where you lack an added action.
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        yes: bool,
    },
    /// Copy a profile (built-ins too) into a new, editable one.
    Duplicate {
        /// Profile id or name.
        profile: String,
        /// Name of the copy.
        name: String,
    },
    /// Archive a profile: its assignments stop granting at once.
    Archive {
        /// Profile id or name.
        profile: String,
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum AssignmentsCmd {
    /// Assignments at scopes you manage, and your own.
    List,
    /// Give a user, service account or group a profile at a scope.
    Add {
        /// Username, email or id (`svc-<name>` for a service account); with
        /// `--group`, a group name or id.
        user: String,
        /// Profile id or name.
        profile: String,
        /// `workspace:<id>`, `cluster:<id|name>`, `project:<id>`, `environment:<id>`, `server:<id>` or `service:<id>`.
        scope: String,
        /// Stop granting after this many minutes (1–525600); default never.
        #[arg(long)]
        expires_in_minutes: Option<u32>,
        /// The first argument names a group: every member gets the profile.
        #[arg(long)]
        group: bool,
        #[arg(long)]
        yes: bool,
    },
    /// Remove an assignment (no MFA needed).
    Remove {
        /// Assignment id.
        assignment: String,
        #[arg(long)]
        yes: bool,
    },
}

pub async fn catalog(g: &Global, api: &mut Api) -> Result<()> {
    let v = api.get("/api/v1/access/catalog").await?;
    show(g, &v["actions"], Some(&[("ACTION", "/action"), ("MFA", "/mfa"), ("ROLES", "/roles")]))
}

pub async fn profiles(g: &Global, api: &mut Api, cmd: ProfilesCmd) -> Result<()> {
    match cmd {
        ProfilesCmd::List => {
            let v = api.get("/api/v1/access/profiles").await?;
            show(
                g,
                &v,
                Some(&[
                    ("ID", "/id"),
                    ("NAME", "/name"),
                    ("BUILTIN", "/builtin"),
                    ("REV", "/revision"),
                    ("ARCHIVED", "/archived_at"),
                    ("ACTIONS", "/actions"),
                ]),
            )
        }
        ProfilesCmd::Show { profile } => {
            let id = profile_id(api, &profile).await?;
            let v = api.get(&format!("/api/v1/access/profiles/{}", enc(&id))).await?;
            show(g, &v, None)
        }
        ProfilesCmd::Create { name, actions, description } => {
            let v = api
                .post(
                    "/api/v1/access/profiles",
                    &json!({ "name": name, "description": description, "actions": actions }),
                )
                .await?;
            show(g, &v, None)
        }
        ProfilesCmd::Publish { profile, actions, dry_run, yes } => {
            let id = profile_id(api, &profile).await?;
            let current = api.get(&format!("/api/v1/access/profiles/{}", enc(&id))).await?;
            let body = json!({ "actions": actions, "expected_revision": current["revision"] });
            let url = format!("/api/v1/access/profiles/{}", enc(&id));
            if dry_run {
                let v = api.call(Method::PUT, &format!("{url}?dry_run=true"), Some(&body)).await?;
                return show(g, &v, None);
            }
            confirm(g, yes, &format!("Publish a new revision of {profile}? Every assignment of it changes at once."))?;
            let v = api.call(Method::PUT, &url, Some(&body)).await?;
            show(g, &v, None)
        }
        ProfilesCmd::Duplicate { profile, name } => {
            let id = profile_id(api, &profile).await?;
            let v =
                api.post(&format!("/api/v1/access/profiles/{}/duplicate", enc(&id)), &json!({ "name": name })).await?;
            show(g, &v, None)
        }
        ProfilesCmd::Archive { profile, yes } => {
            let id = profile_id(api, &profile).await?;
            confirm(g, yes, &format!("Archive {profile}? Its assignments stop granting at once."))?;
            let v = api.call(Method::DELETE, &format!("/api/v1/access/profiles/{}", enc(&id)), None).await?;
            show(g, &v, None)
        }
    }
}

pub async fn assignments(g: &Global, api: &mut Api, cmd: AssignmentsCmd) -> Result<()> {
    match cmd {
        AssignmentsCmd::List => {
            let v = api.get("/api/v1/access/assignments").await?;
            show(
                g,
                &v,
                Some(&[
                    ("ID", "/id"),
                    ("USER", "/user_id"),
                    ("GROUP", "/group_id"),
                    ("PROFILE", "/profile_id"),
                    ("KIND", "/scope/kind"),
                    ("SCOPE", "/scope/id"),
                    ("EXPIRES", "/expires_at"),
                ]),
            )
        }
        AssignmentsCmd::Add { user, profile, scope: s, expires_in_minutes, group, yes } => {
            let pid = profile_id(api, &profile).await?;
            let s = scope(api, s).await?;
            let who = if group { format!("every member of group {user}") } else { user.clone() };
            confirm(g, yes, &format!("Give {who} the profile {profile} at {s}?"))?;
            let mut body = json!({ "profile_id": pid, "scope": s, "expires_in_minutes": expires_in_minutes });
            if group {
                body["group_id"] = json!(group_id(api, &user).await?);
            } else if crate::admincmd::looks_like_id(&user) {
                body["user_id"] = json!(user);
            } else {
                body["login"] = json!(user);
            }
            let v = api.post("/api/v1/access/assignments", &body).await?;
            show(g, &v, None)
        }
        AssignmentsCmd::Remove { assignment, yes } => {
            confirm(g, yes, &format!("Remove assignment {assignment}?"))?;
            let v = api.call(Method::DELETE, &format!("/api/v1/access/assignments/{}", enc(&assignment)), None).await?;
            show(g, &v, None)
        }
    }
}

// ------------------------------------------------------------------ groups

/// A group id, or a group name looked up in the list.
async fn group_id(api: &mut Api, g: &str) -> Result<String> {
    let list = api.get("/api/v1/access/groups").await?;
    let found = list
        .as_array()
        .into_iter()
        .flatten()
        .find(|x| x["id"] == g || x["name"].as_str().is_some_and(|n| n.eq_ignore_ascii_case(g)));
    match found.and_then(|x| x["id"].as_str()) {
        Some(id) => Ok(id.to_owned()),
        None => Err(crate::error::CliError::new(crate::error::Exit::NotFound, format!("no group `{g}`"))),
    }
}

#[derive(Subcommand, Debug)]
pub enum GroupsCmd {
    /// Every group with its member ids.
    List,
    /// Create an empty group (`access.manage`).
    Create {
        name: String,
        #[arg(long, default_value = "")]
        description: String,
    },
    /// Delete a group: its members lose every grant through it at once.
    Delete {
        /// Group id or name.
        group: String,
        #[arg(long)]
        yes: bool,
    },
    /// Add a user (needs MFA and every action the group holds at its scopes).
    AddMember {
        /// Group id or name.
        group: String,
        /// Username, email or id.
        user: String,
    },
    /// Remove a user; their grants through the group end at once.
    RemoveMember {
        /// Group id or name.
        group: String,
        /// Username, email or id.
        user: String,
    },
}

pub async fn groups(g: &Global, api: &mut Api, cmd: GroupsCmd) -> Result<()> {
    match cmd {
        GroupsCmd::List => {
            let v = api.get("/api/v1/access/groups").await?;
            show(
                g,
                &v,
                Some(&[("ID", "/id"), ("NAME", "/name"), ("MEMBERS", "/members"), ("DESCRIPTION", "/description")]),
            )
        }
        GroupsCmd::Create { name, description } => {
            let v = api.post("/api/v1/access/groups", &json!({ "name": name, "description": description })).await?;
            show(g, &v, None)
        }
        GroupsCmd::Delete { group, yes } => {
            let id = group_id(api, &group).await?;
            confirm(g, yes, &format!("Delete group {group}? Its members lose every grant through it at once."))?;
            let v = api.call(Method::DELETE, &format!("/api/v1/access/groups/{}", enc(&id)), None).await?;
            show(g, &v, None)
        }
        GroupsCmd::AddMember { group, user } => {
            let id = group_id(api, &group).await?;
            let uid = crate::admincmd::user_id(api, &user).await?;
            let url = format!("/api/v1/access/groups/{}/members/{}", enc(&id), enc(&uid));
            let v = api.call(Method::PUT, &url, None).await?;
            show(g, &v, None)
        }
        GroupsCmd::RemoveMember { group, user } => {
            let id = group_id(api, &group).await?;
            let uid = crate::admincmd::user_id(api, &user).await?;
            let url = format!("/api/v1/access/groups/{}/members/{}", enc(&id), enc(&uid));
            let v = api.call(Method::DELETE, &url, None).await?;
            show(g, &v, None)
        }
    }
}

pub async fn effective(g: &Global, api: &mut Api, user: &str, s: String, allowed: bool) -> Result<()> {
    let uid = crate::admincmd::user_id(api, user).await?;
    let s = scope(api, s).await?;
    let mut v = api.get(&format!("/api/v1/access/effective?user_id={}&scope={}", enc(&uid), enc(&s))).await?;
    if allowed && let Some(list) = v["actions"].as_array_mut() {
        list.retain(|a| a["allowed"] == true);
    }
    show(g, &v, None)
}

// ------------------------------------------------------------ service accounts

#[derive(Subcommand, Debug)]
pub enum ServiceAccountsCmd {
    /// Service accounts, with owner, token count and last use.
    List,
    /// Create a service account (`svc-<name>`); give it access with `forge access assignments add`.
    Create {
        /// 3–40 of a-z, 0-9 and -.
        name: String,
        #[arg(long, default_value = "")]
        description: String,
        /// The accountable person (username or email); default you.
        #[arg(long)]
        owner: Option<String>,
    },
    /// Stop the account: its tokens fail at once and open sessions end.
    Disable {
        /// Service account id or `svc-<name>`.
        account: String,
        #[arg(long)]
        yes: bool,
    },
    /// Allow a disabled account again.
    Enable {
        /// Service account id or `svc-<name>`.
        account: String,
    },
    /// The account's tokens (no secrets).
    Tokens {
        /// Service account id or `svc-<name>`.
        account: String,
    },
    /// Issue a token; the secret is printed once.
    TokenCreate {
        /// Service account id or `svc-<name>`.
        account: String,
        /// A label for the token, 1–80 characters.
        name: String,
        /// Days until it expires (1–3650; default 90).
        #[arg(long, conflicts_with = "never_expires")]
        expires_days: Option<u32>,
        /// A token that never expires (flagged in lists).
        #[arg(long)]
        never_expires: bool,
        /// May run MFA-guarded actions without a prompt (CI). Needs an
        /// MFA-verified session and `[security] allow_elevated_tokens`.
        #[arg(long)]
        elevated: bool,
        /// Restrict the token to these actions at a scope:
        /// `<kind>:<id>=<action>[,<action>…]`, for example
        /// `service:<id>=view,deploy`. Repeat for more scopes. Without it
        /// the token acts with all of the account's access; with it, only
        /// with what is listed and never more than the account holds.
        #[arg(long = "allow", value_name = "SCOPE=ACTIONS")]
        allow: Vec<String>,
    },
    /// Revoke a token (no MFA needed).
    TokenRevoke {
        /// Service account id or `svc-<name>`.
        account: String,
        /// Token id from `forge service-accounts tokens`.
        token: String,
        #[arg(long)]
        yes: bool,
    },
}

async fn account_id(api: &mut Api, a: &str) -> Result<String> {
    let list = api.get("/api/v1/service-accounts").await?;
    let want =
        if a.starts_with("svc-") || crate::admincmd::looks_like_id(a) { a.to_owned() } else { format!("svc-{a}") };
    let found = list.as_array().into_iter().flatten().find(|x| x["id"] == want.as_str() || x["name"] == want.as_str());
    match found.and_then(|x| x["id"].as_str()) {
        Some(id) => Ok(id.to_owned()),
        None => Err(crate::error::CliError::new(crate::error::Exit::NotFound, format!("no service account `{a}`"))),
    }
}

pub async fn service_accounts(g: &Global, cmd: ServiceAccountsCmd) -> Result<()> {
    let t = crate::target(g)?;
    let mut api = crate::connect(g, &t).await?;
    let api = &mut api;
    match cmd {
        ServiceAccountsCmd::List => {
            let v = api.get("/api/v1/service-accounts").await?;
            show(
                g,
                &v,
                Some(&[
                    ("ID", "/id"),
                    ("NAME", "/name"),
                    ("OWNER", "/owner"),
                    ("DISABLED", "/disabled"),
                    ("TOKENS", "/tokens"),
                    ("USED", "/last_used_at"),
                ]),
            )
        }
        ServiceAccountsCmd::Create { name, description, owner } => {
            let v = api
                .post("/api/v1/service-accounts", &json!({ "name": name, "description": description, "owner": owner }))
                .await?;
            show(g, &v, None)
        }
        ServiceAccountsCmd::Disable { account, yes } => {
            let id = account_id(api, &account).await?;
            confirm(g, yes, &format!("Disable {account}? Its tokens stop working at once."))?;
            let v = api
                .call(
                    Method::PATCH,
                    &format!("/api/v1/service-accounts/{}", enc(&id)),
                    Some(&json!({ "disabled": true })),
                )
                .await?;
            show(g, &v, None)
        }
        ServiceAccountsCmd::Enable { account } => {
            let id = account_id(api, &account).await?;
            let v = api
                .call(
                    Method::PATCH,
                    &format!("/api/v1/service-accounts/{}", enc(&id)),
                    Some(&json!({ "disabled": false })),
                )
                .await?;
            show(g, &v, None)
        }
        ServiceAccountsCmd::Tokens { account } => {
            let id = account_id(api, &account).await?;
            let v = api.get(&format!("/api/v1/service-accounts/{}/tokens", enc(&id))).await?;
            show(
                g,
                &v,
                Some(&[
                    ("ID", "/id"),
                    ("NAME", "/name"),
                    ("PREFIX", "/prefix"),
                    ("ELEVATED", "/elevated"),
                    ("EXPIRES", "/expires_at"),
                    ("USED", "/last_used_at"),
                ]),
            )
        }
        ServiceAccountsCmd::TokenCreate { account, name, expires_days, never_expires, elevated, allow } => {
            let id = account_id(api, &account).await?;
            let mut body = json!({ "name": name, "expires_days": expires_days, "never_expires": never_expires });
            if elevated {
                body["elevated"] = json!(true);
            }
            if let Some(r) = crate::admincmd::restrictions(api, &allow).await? {
                body["restrictions"] = r;
            }
            let v = api.post(&format!("/api/v1/service-accounts/{}/tokens", enc(&id)), &body).await?;
            if g.output == out::Format::Table {
                eprintln!("Store this secret now; Forge shows it only once.");
            }
            show(g, &v, None)
        }
        ServiceAccountsCmd::TokenRevoke { account, token, yes } => {
            let id = account_id(api, &account).await?;
            confirm(g, yes, &format!("Revoke token {token} of {account}? Anything using it stops working."))?;
            let v = api
                .call(Method::DELETE, &format!("/api/v1/service-accounts/{}/tokens/{}", enc(&id), enc(&token)), None)
                .await?;
            show(g, &v, None)
        }
    }
}
