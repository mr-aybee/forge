//! Command groups and their handlers.

use std::io::{BufRead, IsTerminal, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use clap::Subcommand;
use forge_sdk::manifest::Manifest;
use forge_sdk::signing::PublisherKey;
use serde_json::{Value, json};

use crate::api::Api;
use crate::error::{CliError, Exit, Result};
use crate::out::{self, Col, Format};
use crate::{Global, connect, creds, ctx, login, target};

// ------------------------------------------------------------------ helpers

fn show(g: &Global, v: &Value, cols: Option<&[Col]>) -> Result<()> {
    out::result(g.output, v, cols);
    Ok(())
}

fn enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

pub(crate) fn looks_like_id(s: &str) -> bool {
    s.len() == 36
        && s.bytes().filter(|&b| b == b'-').count() == 4
        && s.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
}

/// Resolves a node by id, hostname or display name: exactly one match, never the first.
pub(crate) async fn resolve_node(api: &mut Api, node: &str) -> Result<Value> {
    if looks_like_id(node) {
        return api.get(&format!("/api/v1/servers/{node}")).await;
    }

    let all = api.get("/api/v1/servers").await?;
    let matches: Vec<&Value> = all
        .as_array()
        .into_iter()
        .flatten()
        .filter(|s| {
            s["hostname"].as_str().is_some_and(|h| h.eq_ignore_ascii_case(node))
                || s["display_name"].as_str().is_some_and(|d| d.eq_ignore_ascii_case(node))
        })
        .collect();

    match matches.as_slice() {
        [one] => Ok((*one).clone()),
        [] => Err(CliError::new(Exit::NotFound, format!("no node named `{node}` that you can see"))),
        many => {
            let ids: Vec<String> = many
                .iter()
                .map(|s| format!("{} ({})", s["id"].as_str().unwrap_or("?"), s["hostname"].as_str().unwrap_or("?")))
                .collect();
            Err(CliError::usage(format!("`{node}` matches {} nodes; use an id: {}", many.len(), ids.join(", ")))
                .with_code("ambiguous"))
        }
    }
}

/// Asks for confirmation of a mutation unless `--yes`.
pub(crate) fn confirm(g: &Global, yes: bool, what: &str) -> Result<()> {
    if yes {
        return Ok(());
    }

    if g.non_interactive || !std::io::stdin().is_terminal() {
        return Err(CliError::usage(format!("{what}: confirmation needed; pass --yes in non-interactive use")));
    }

    eprint!("{what}\nType `yes` to continue: ");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;

    if line.trim() == "yes" { Ok(()) } else { Err(CliError::new(Exit::General, "cancelled").with_code("cancelled")) }
}

pub async fn simple_get(g: &Global, path: &str) -> Result<()> {
    let t = target(g)?;
    let mut api = match connect(g, &t).await {
        Ok(a) => a,
        // Public endpoints work without signing in.
        Err(e) if e.exit == Exit::Auth => Api::anonymous(t.server.clone(), t.ca_file.as_deref())?,
        Err(e) => return Err(e),
    };

    let v = api.get(path).await?;

    show(g, &v, None)
}

pub async fn version(g: &Global) -> Result<()> {
    let client = json!({ "version": crate::VERSION });

    let server = match target(g) {
        Ok(t) => Api::anonymous(t.server.clone(), t.ca_file.as_deref())?.get("/api/version").await.ok(),
        Err(_) => None,
    };

    let v = json!({ "client": client, "server": server });

    if g.output == Format::Table {
        println!("client  {}", crate::VERSION);

        match &server {
            Some(s) => println!(
                "server  {} (build {})",
                s["version"].as_str().unwrap_or("?"),
                s["build"].as_str().unwrap_or("?")
            ),
            None => println!("server  (not connected)"),
        }

        return Ok(());
    }

    show(g, &v, None)
}

// ------------------------------------------------------------------ contexts

#[derive(Subcommand, Debug)]
pub enum ContextsCmd {
    /// Add a context and make it current (unless --no-use).
    Add {
        name: String,
        /// Server URL (https; http only for localhost).
        #[arg(long)]
        server: String,
        /// Extra CA certificate (PEM) for an on-premises server.
        #[arg(long)]
        ca_file: Option<PathBuf>,
        #[arg(long)]
        no_use: bool,
    },
    /// List contexts.
    List,
    /// Switch the current context.
    Use { name: String },
    /// Show the current context.
    Current,
    /// Change a context's settings (changing the server clears its login).
    Update {
        name: String,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        ca_file: Option<PathBuf>,
        #[arg(long)]
        default_cluster: Option<String>,
    },
    /// Remove a context and its stored login (revoked on the server first).
    Remove { name: String },
}

pub async fn contexts(g: &Global, cmd: ContextsCmd) -> Result<()> {
    match cmd {
        ContextsCmd::Add { name, server, ca_file, no_use } => {
            ctx::valid_name(&name)?;
            let server = ctx::normalise_server(&server)?;
            let ca = ca_file.map(|p| p.to_string_lossy().into_owned());
            ctx::Config::update(|c| {
                if c.contexts.contains_key(&name) {
                    return Err(CliError::new(
                        Exit::Conflict,
                        format!("context `{name}` exists; use `forge contexts update`"),
                    ));
                }

                c.contexts
                    .insert(name.clone(), ctx::Context { server: server.clone(), ca_file: ca, ..Default::default() });

                if !no_use || c.current.is_none() {
                    c.current = Some(name.clone());
                }

                Ok(())
            })?;
            eprintln!("Added context `{name}` → {server}. Sign in with: forge auth login");
            show(g, &json!({ "name": name, "server": server }), None)
        }
        ContextsCmd::List => {
            let cfg = ctx::Config::load()?;
            let rows: Vec<Value> = cfg
                .contexts
                .iter()
                .map(|(n, c)| {
                    let signed_in = creds::load(n).ok().flatten().is_some_and(|t| t.server == c.server);
                    json!({
                        "current": if cfg.current.as_deref() == Some(n) { "*" } else { "" },
                        "name": n, "server": c.server, "user": c.user, "instance_id": c.instance_id,
                        "signed_in": signed_in,
                    })
                })
                .collect();
            show(
                g,
                &Value::Array(rows),
                Some(&[
                    ("", "/current"),
                    ("NAME", "/name"),
                    ("SERVER", "/server"),
                    ("USER", "/user"),
                    ("SIGNED IN", "/signed_in"),
                ]),
            )
        }
        ContextsCmd::Use { name } => {
            ctx::Config::update(|c| {
                c.get(&name)?;
                c.current = Some(name.clone());
                Ok(())
            })?;
            show(g, &json!({ "current": name }), None)
        }
        ContextsCmd::Current => {
            let cfg = ctx::Config::load()?;
            let name = cfg.current.clone().ok_or_else(|| CliError::new(Exit::NotFound, "no current context"))?;
            let c = cfg.get(&name)?;
            show(g, &json!({ "name": name, "server": c.server, "user": c.user, "instance_id": c.instance_id }), None)
        }
        ContextsCmd::Update { name, server, ca_file, default_cluster } => {
            let server = server.map(|s| ctx::normalise_server(&s)).transpose()?;
            let moved = ctx::Config::update(|c| {
                let entry = c
                    .contexts
                    .get_mut(&name)
                    .ok_or_else(|| CliError::new(Exit::NotFound, format!("no context `{name}`")))?;
                let mut moved = false;

                if let Some(s) = server {
                    moved = s != entry.server;

                    if moved {
                        entry.instance_id = None;
                        entry.user = None;
                    }

                    entry.server = s;
                }

                if let Some(p) = ca_file {
                    entry.ca_file = Some(p.to_string_lossy().into_owned());
                }

                if let Some(d) = default_cluster {
                    entry.default_cluster = (!d.is_empty()).then_some(d);
                }

                Ok(moved)
            })?;

            if moved {
                creds::delete(&name)?;
                eprintln!("The server changed: the stored login was removed. Sign in again with: forge auth login");
            }

            show(g, &json!({ "updated": name }), None)
        }
        ContextsCmd::Remove { name } => {
            let cfg = ctx::Config::load()?;
            let c = cfg.get(&name)?.clone();
            let revoked = match creds::for_server(&name, &c.server) {
                Ok(Some(tokens)) => {
                    let mut api = Api::with_context(c.server.clone(), c.ca_file.as_deref(), name.clone(), tokens)?;
                    api.logout().await.is_ok()
                }
                _ => false,
            };

            creds::delete(&name)?;
            ctx::Config::update(|cfg| {
                cfg.contexts.remove(&name);

                if cfg.current.as_deref() == Some(name.as_str()) {
                    cfg.current = None;
                }

                Ok(())
            })?;

            if !revoked {
                eprintln!("Note: the server session was not revoked (not signed in, or the server was unreachable).");
            }

            show(g, &json!({ "removed": name, "server_session_revoked": revoked }), None)
        }
    }
}

// ------------------------------------------------------------------ auth

#[derive(Subcommand, Debug)]
pub enum AuthCmd {
    /// Sign in with your browser (password, MFA, passkeys and SSO happen there).
    Login {
        /// How long to wait for the sign-in.
        #[arg(long, default_value_t = 600)]
        timeout_secs: u64,
        /// Sign in from another device: print a URL and a code to approve in
        /// the console (for SSH sessions and machines without a browser).
        #[arg(long)]
        device: bool,
    },
    /// Verify a one-time code so this session may run MFA-protected actions.
    StepUp {
        /// The code from your authenticator app; prompted for when omitted.
        #[arg(long)]
        code: Option<String>,
    },
    /// Sign out: revoke the session on the server and forget it locally.
    Logout,
    /// Show whether you are signed in.
    Status,
    /// The signed-in user, roles and MFA state.
    Whoami,
}

pub async fn auth(g: &Global, cmd: AuthCmd) -> Result<()> {
    match cmd {
        AuthCmd::StepUp { code } => {
            if g.token.is_some() {
                return Err(CliError::usage(
                    "FORGE_TOKEN cannot be stepped up: sign in with a context (`forge auth login`), then run `forge auth step-up`",
                )
                .with_code("step_up_needs_context"));
            }

            let t = target(g)?;
            let mut api = connect(g, &t).await?;

            let code = match code {
                Some(c) => c,
                None => crate::api::prompt_code(g.non_interactive)?,
            };

            let v = api.step_up(&code).await?;
            show(g, &v, None)
        }
        AuthCmd::Login { timeout_secs, device } => {
            if g.non_interactive {
                return Err(CliError::usage("interactive sign-in is disabled by --non-interactive; use FORGE_TOKEN"));
            }

            let t = target(g)?;
            let name = t
                .context
                .clone()
                .ok_or_else(|| CliError::usage("sign-in needs a context: forge contexts add <name> --server <url>"))?;
            let timeout = Duration::from_secs(timeout_secs);

            let tokens = if device {
                login::device(&t.server, t.ca_file.as_deref(), timeout).await?
            } else {
                login::browser(&t.server, t.ca_file.as_deref(), timeout).await?
            };

            let mut api = Api::with_context(t.server.clone(), t.ca_file.as_deref(), name.clone(), tokens.clone())?;
            let me = api.get("/api/v1/me").await?;
            let ws = api.get("/api/v1/workspace").await?;
            let instance = ws["id"].as_str().map(str::to_owned);
            if let (Some(recorded), Some(now)) = (&t.instance_id, &instance)
                && recorded != now
            {
                let _ = api.logout().await;
                return Err(CliError::new(
                    Exit::Conflict,
                    format!("{} is now a different deployment (workspace {now}, expected {recorded}); login discarded. If this is intended, remove and re-add the context.", t.server),
                )
                .with_code("instance_mismatch"));
            }

            let store = creds::save(&name, &tokens)?;
            let user = me["user"]["username"].as_str().or(me["user"]["email"].as_str()).map(str::to_owned);
            ctx::Config::update(|c| {
                if let Some(e) = c.contexts.get_mut(&name) {
                    e.instance_id = instance.clone();
                    e.user = user.clone();
                }

                Ok(())
            })?;
            let stored = if store == creds::Store::Keyring { "OS credential store" } else { "owner-only file" };
            eprintln!("Signed in to `{name}` as {} (token kept in the {stored}).", user.as_deref().unwrap_or("?"));
            show(g, &json!({ "context": name, "user": user, "instance_id": instance, "mfa": me["mfa"] }), None)
        }
        AuthCmd::Logout => {
            let t = target(g)?;
            let name = t.context.clone().ok_or_else(|| CliError::usage("no context selected"))?;
            let revoked = match creds::for_server(&name, &t.server)? {
                Some(tokens) => Api::with_context(t.server.clone(), t.ca_file.as_deref(), name.clone(), tokens)?
                    .logout()
                    .await
                    .is_ok(),
                None => false,
            };

            creds::delete(&name)?;
            ctx::Config::update(|c| {
                if let Some(e) = c.contexts.get_mut(&name) {
                    e.user = None;
                }

                Ok(())
            })?;
            show(g, &json!({ "context": name, "server_session_revoked": revoked }), None)
        }
        AuthCmd::Status => {
            let t = target(g)?;
            let signed_in = match &t.context {
                Some(n) => creds::for_server(n, &t.server)?.is_some(),
                None => g.token.is_some(),
            };

            let v = json!({ "context": t.context, "server": t.server, "instance_id": t.instance_id, "signed_in": signed_in });

            if !signed_in {
                // One document per run: the error alone (exit 3).
                let where_ = t.context.as_deref().unwrap_or(&t.server);
                return Err(CliError::auth(format!("not signed in to `{where_}`")).with_code("not_signed_in"));
            }

            show(g, &v, None)
        }
        AuthCmd::Whoami => {
            let t = target(g)?;
            let mut api = connect(g, &t).await?;
            let me = api.get("/api/v1/me").await?;

            if g.output == Format::Table {
                let u = &me["user"];
                println!("user      {}", u["username"].as_str().or(u["email"].as_str()).unwrap_or("?"));
                println!("name      {}", u["display_name"].as_str().unwrap_or("-"));
                println!("email     {}", u["email"].as_str().unwrap_or("-"));
                println!("mfa       {}", if me["mfa"] == true { "verified" } else { "not verified" });

                for b in me["bindings"].as_array().into_iter().flatten() {
                    println!(
                        "role      {} on {}",
                        b["role"].as_str().unwrap_or("?"),
                        // `{kind, id}` (or a plain string from older servers).
                        b["scope"].as_str().map_or_else(
                            || format!(
                                "{}:{}",
                                b["scope"]["kind"].as_str().unwrap_or("?"),
                                b["scope"]["id"].as_str().unwrap_or("?")
                            ),
                            str::to_owned
                        )
                    );
                }

                return Ok(());
            }

            show(g, &me, None)
        }
    }
}

// ------------------------------------------------------------------ nodes

/// `4 vCPU · 7.8 GiB RAM · 64 GiB disk` from a telemetry sample.
fn specs(m: &Value) -> String {
    let gib = |b: Option<u64>| b.map(|b| format!("{:.1} GiB", b as f64 / (1u64 << 30) as f64));
    let parts: Vec<String> = [
        m["cpus"].as_u64().filter(|c| *c > 0).map(|c| format!("{c} vCPU")),
        gib(m["mem_total_bytes"].as_u64().or(m["mem_total"].as_u64()).filter(|b| *b > 0)).map(|x| format!("{x} RAM")),
        gib(m["disk_total_bytes"].as_u64().or(m["disk_total"].as_u64()).filter(|b| *b > 0))
            .map(|x| format!("{x} disk")),
    ]
    .into_iter()
    .flatten()
    .collect();

    if parts.is_empty() { "-".into() } else { parts.join(" · ") }
}

const NODE_COLS: &[Col] = &[
    ("ID", "/id"),
    ("HOSTNAME", "/hostname"),
    ("NAME", "/display_name"),
    ("STATUS", "/status"),
    ("OS", "/os_label"),
    ("SPECS", "/specs"),
    ("AGENT", "/agent_version"),
    ("CLUSTER", "/cluster_id"),
];

#[derive(Subcommand, Debug)]
pub enum NodesCmd {
    /// List nodes you can see.
    List {
        /// Only nodes in this cluster (id, name or abbreviation).
        #[arg(long)]
        cluster: Option<String>,
        /// Only nodes in this workspace (id).
        #[arg(long)]
        workspace: Option<String>,
        #[command(flatten)]
        paging: out::Paging,
    },
    /// Show one node (id, hostname or display name).
    #[command(alias = "show")]
    Get { node: String },
    /// A node's tags: list them, or change them with --set, --add or --remove.
    Tags {
        node: String,
        /// Replace all tags (comma-separated; empty clears them).
        #[arg(long, value_delimiter = ',', conflicts_with_all = ["add", "remove"])]
        set: Option<Vec<String>>,
        /// Add tags (comma-separated).
        #[arg(long, value_delimiter = ',')]
        add: Vec<String>,
        /// Remove tags (comma-separated).
        #[arg(long, value_delimiter = ',')]
        remove: Vec<String>,
    },
    /// Send a read-only command (ping | collect_facts | service_status) and
    /// print its result.
    Command {
        node: String,
        kind: String,
        /// Service name (service_status).
        #[arg(long)]
        name: Option<String>,
        /// Print the queued job and return at once (follow it with
        /// `forge jobs watch <id>`).
        #[arg(long)]
        no_wait: bool,
        /// How long to wait for the result (exit 8 when it is not there yet;
        /// the job keeps running).
        #[arg(long, default_value_t = 25)]
        timeout_secs: u64,
    },
    /// Rename a node in the console (display name).
    Rename { node: String, name: String },
    /// Set a node's DNS label (`web-1` → `web-1.local`); an empty label
    /// goes back to the one derived from its name. Every node's hosts file
    /// follows.
    DnsName { node: String, label: String },
    /// Drain a node: placement skips it and Forge moves its movable services
    /// to other nodes; services with local data stay and raise an alert.
    Drain {
        node: String,
        /// Skip the confirmation prompt.
        #[arg(long)]
        yes: bool,
    },
    /// Undrain a node: it takes new placements again.
    Undrain { node: String },
    /// Browse and manage files on a node (File Explorer).
    #[command(subcommand)]
    Files(crate::filescmd::FilesCmd),
    /// Revoke a node's agent certificate (it can no longer connect).
    Revoke {
        node: String,
        /// One-time code from your authenticator app. Asked for on a
        /// terminal when the server requires MFA and you have enrolled.
        #[arg(long)]
        code: Option<String>,
        #[arg(long)]
        yes: bool,
    },
    /// Remove a node for good: revokes its certificate and deletes the node
    /// with its services, stack parts, settings and history. The machine can
    /// enroll again under the same name as a fresh node.
    Remove {
        node: String,
        /// One-time code from your authenticator app. Asked for on a
        /// terminal when the server requires MFA and you have enrolled.
        #[arg(long)]
        code: Option<String>,
        /// Skip the confirmation prompt.
        #[arg(long)]
        yes: bool,
    },
    #[command(flatten)]
    Extra(crate::admincmd::NodeExtraCmd),
}

pub async fn nodes(g: &Global, cmd: NodesCmd) -> Result<()> {
    let cmd = match cmd {
        NodesCmd::Files(f) => return crate::filescmd::run(g, f).await,
        NodesCmd::Extra(x) => return crate::admincmd::node_extra(g, x).await,
        other => other,
    };

    let remove = matches!(cmd, NodesCmd::Remove { .. });
    let t = target(g)?;
    let mut api = connect(g, &t).await?;

    match cmd {
        NodesCmd::List { cluster, workspace, paging } => {
            let path = match &workspace {
                Some(w) => format!("/api/v1/servers?workspace={}", enc(w)),
                None => "/api/v1/servers".to_owned(),
            };

            let mut v = api.get(&path).await?;

            // The API has no cluster filter: resolve the name to an id here
            // (the context's default cluster may be a name too).
            if let Some(c) = cluster.or(t.default_cluster.clone()) {
                let id = crate::admincmd::cluster_id(&mut api, &c).await?;

                if let Value::Array(rows) = &mut v {
                    filter_cluster(rows, &id);
                }
            }

            // Hardware from live telemetry (best effort: offline nodes show "-").
            let latest = api.get("/api/v1/metrics/latest").await.unwrap_or(Value::Null);

            for s in v.as_array_mut().into_iter().flatten() {
                let id = s["id"].as_str().unwrap_or_default().to_owned();

                // Saved facts (server ≥ issue #8 tags/facts) first, live telemetry otherwise.
                let m = if s["facts"].is_object() { s["facts"].clone() } else { latest[id.as_str()].clone() };
                s["specs"] = json!(specs(&m));
                let os = s["facts"]["os_version"].as_str().or(s["os"].as_str()).unwrap_or("-").to_owned();
                s["os_label"] = json!(format!("{os} {}", s["arch"].as_str().unwrap_or_default()).trim().to_owned());
            }

            out::list(g.output, rows_of(v), Some(NODE_COLS), paging.into(), out::Total::Rows);
            Ok(())
        }
        NodesCmd::Get { node } => {
            let v = resolve_node(&mut api, &node).await?;
            show(g, &v, None)
        }
        NodesCmd::Tags { node, set, add, remove } => {
            let id = resolve_node(&mut api, &node).await?["id"].as_str().unwrap_or_default().to_owned();
            let path = format!("/api/v1/tags/server/{id}");
            let current: Vec<String> =
                serde_json::from_value(api.get(&path).await?["tags"].clone()).unwrap_or_default();
            let clean = |v: Vec<String>| -> Vec<String> {
                v.into_iter().map(|t| t.trim().to_owned()).filter(|t| !t.is_empty()).collect()
            };

            let wanted = match set {
                Some(s) => Some(clean(s)),
                None if add.is_empty() && remove.is_empty() => None,
                None => {
                    let (add, remove) = (clean(add), clean(remove));
                    let mut t: Vec<String> = current.iter().filter(|t| !remove.contains(t)).cloned().collect();

                    for a in add {
                        if !t.contains(&a) {
                            t.push(a);
                        }
                    }

                    Some(t)
                }
            };

            let v = match wanted {
                Some(tags) => api.call(reqwest::Method::PUT, &path, Some(&json!({ "tags": tags }))).await?,
                None => json!({ "tags": current }),
            };

            show(g, &v, None)
        }
        NodesCmd::Command { node, kind, name, no_wait, timeout_secs } => {
            let id = resolve_node(&mut api, &node).await?["id"].as_str().unwrap_or_default().to_owned();
            let mut command = json!({ "type": kind });

            if let Some(n) = name {
                command["name"] = json!(n);
            }

            let job = api.post(&format!("/api/v1/servers/{id}/commands"), &json!({ "command": command })).await?;
            let job_id = job["id"].as_str().unwrap_or_default().to_owned();
            if no_wait || job_id.is_empty() {
                if g.output.is_machine() {
                    return show(g, &job, None);
                }

                println!("{job_id}");
                eprintln!("queued; follow it with `forge jobs watch {job_id}`");

                return Ok(());
            }

            let done = wait_jobs(&mut api, std::slice::from_ref(&job_id), Duration::from_secs(timeout_secs)).await;
            match done {
                Ok(jobs) => {
                    let j = jobs.into_iter().next().unwrap_or(Value::Null);
                    let output = j["state"]["output"].clone();
                    if g.output.is_machine() {
                        return show(g, &json!({ "job_id": job_id, "status": j["status"], "result": output }), None);
                    }

                    show(g, &command_result(&kind, &output), None)
                }
                Err(e) if e.exit == Exit::Timeout => Err(CliError::new(
                    Exit::Timeout,
                    format!(
                        "{kind} on {node} has no result after {timeout_secs}s; it keeps running: `forge jobs watch {job_id}`"
                    ),
                )),
                Err(e) => Err(e),
            }
        }
        NodesCmd::Rename { node, name } => {
            let id = resolve_node(&mut api, &node).await?["id"].as_str().unwrap_or_default().to_owned();
            let v = api
                .call(reqwest::Method::PUT, &format!("/api/v1/servers/{id}/name"), Some(&json!({ "name": name })))
                .await?;
            show(g, &v, None)
        }
        NodesCmd::DnsName { node, label } => {
            let id = resolve_node(&mut api, &node).await?["id"].as_str().unwrap_or_default().to_owned();
            let v = api
                .call(reqwest::Method::PUT, &format!("/api/v1/servers/{id}/dns-name"), Some(&json!({ "label": label })))
                .await?;
            show(g, &v, None)
        }
        NodesCmd::Drain { node, yes } => {
            let s = resolve_node(&mut api, &node).await?;
            let id = s["id"].as_str().unwrap_or_default().to_owned();
            let hostname = s["hostname"].as_str().unwrap_or_default();
            let question = format!(
                "Drain node {hostname} ({id})? New placements skip it, and its services without local data move to \
                 other nodes."
            );
            confirm(g, yes, &question)?;

            let v = api.post(&format!("/api/v1/servers/{id}/drain"), &json!({})).await?;
            show(g, &v, None)
        }
        NodesCmd::Undrain { node } => {
            let id = resolve_node(&mut api, &node).await?["id"].as_str().unwrap_or_default().to_owned();
            let v = api.post(&format!("/api/v1/servers/{id}/undrain"), &json!({})).await?;
            show(g, &v, None)
        }
        NodesCmd::Files(_) | NodesCmd::Extra(_) => Ok(()),
        NodesCmd::Revoke { node, code, yes } | NodesCmd::Remove { node, code, yes } => {
            let s = resolve_node(&mut api, &node).await?;
            let id = s["id"].as_str().unwrap_or_default().to_owned();
            let hostname = s["hostname"].as_str().unwrap_or_default().to_owned();

            let question = if remove {
                format!(
                    "Remove node {hostname} ({id}) from {}? Its certificate is revoked and the node, its \
                     services, stack parts, file settings, tunnels, metrics history and alarms are deleted. \
                     This cannot be undone.",
                    t.server
                )
            } else {
                format!("Revoke node {hostname} ({id}) on {}? It will disconnect and must be re-enrolled.", t.server)
            };

            confirm(g, yes, &question)?;

            // The server wants the host name typed as confirmation and, when it
            // requires MFA and you have enrolled, a fresh one-time code (a
            // missing code counts as a failed attempt, so ask first).
            let code = match code {
                Some(c) => Some(c),
                None => {
                    let me = api.get("/api/v1/me").await?;
                    let caps = api.get("/api/capabilities").await.unwrap_or(Value::Null);
                    let required = caps["capabilities"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .any(|c| c["id"] == "security.mfa_required" && c["enabled"] == true);

                    if required && me["mfa_enrolled"] == true {
                        Some(crate::api::prompt_code(g.non_interactive)?)
                    } else {
                        None
                    }
                }
            };

            let body = json!({ "confirm_hostname": hostname, "totp_code": code });

            if remove {
                api.call(reqwest::Method::DELETE, &format!("/api/v1/servers/{id}"), Some(&body)).await?;
                return show(g, &json!({ "removed": id, "hostname": hostname }), None);
            }

            let v = api.post(&format!("/api/v1/servers/{id}/revoke"), &body).await?;
            show(g, &v, None)
        }
    }
}

/// Keeps the nodes in cluster `id`.
fn filter_cluster(rows: &mut Vec<Value>, id: &str) {
    rows.retain(|s| s["cluster_id"].as_str() == Some(id));
}

/// The rows of a list answer (an array, or nothing).
pub(crate) fn rows_of(v: Value) -> Vec<Value> {
    match v {
        Value::Array(a) => a,
        _ => Vec::new(),
    }
}

/// A node command's output for people: plain text stays text; `ping` and
/// `service_status` answers are objects shown as key/value.
fn command_result(kind: &str, output: &Value) -> Value {
    match output {
        Value::Null => json!(format!("{kind}: done (no output)")),
        Value::String(s) => match serde_json::from_str::<Value>(s) {
            Ok(v @ (Value::Object(_) | Value::Array(_))) => v,
            _ => json!(s.trim_end()),
        },
        other => other.clone(),
    }
}

// ------------------------------------------------------------------ dns

#[derive(Subcommand, Debug)]
pub enum DnsCmd {
    /// Local names of nodes and services, with addresses and published ports.
    List {
        /// Only this workspace (id).
        #[arg(long)]
        workspace: Option<String>,
        /// Only `node` or `service` names.
        #[arg(long, value_parser = ["node", "service"])]
        kind: Option<String>,
        /// Only this compose stack's service and replica names (stack id).
        #[arg(long)]
        stack: Option<String>,
    },
    /// Show or change the suffix every local name ends in.
    #[command(subcommand)]
    Suffix(SuffixCmd),
}

#[derive(Subcommand, Debug)]
pub enum SuffixCmd {
    /// Show the suffix in use and where it comes from.
    Get,
    /// Set the suffix of every local name (`home.arpa`, `corp.internal`).
    /// Platform admins, MFA-verified session. Old names stop resolving.
    Set {
        /// Lowercase DNS labels separated by dots, at most 63 characters.
        suffix: String,
    },
    /// Clear the runtime setting and go back to the config file's suffix.
    Reset,
}

const SUFFIX_COLS: &[Col] =
    &[("SUFFIX", "/suffix"), ("SOURCE", "/source"), ("CONFIG", "/config_suffix"), ("DEFAULT", "/default_suffix")];

const DNS_COLS: &[Col] = &[
    ("NAME", "/name"),
    ("KIND", "/kind"),
    ("NODE", "/node"),
    ("ADDRESS", "/address"),
    ("MESH", "/mesh_address"),
    ("PORTS", "/ports_text"),
];

pub async fn dns(g: &Global, cmd: DnsCmd) -> Result<()> {
    let t = target(g)?;
    let mut api = connect(g, &t).await?;

    match cmd {
        DnsCmd::List { workspace, kind, stack } => {
            let mut q = Vec::new();

            if let Some(w) = workspace {
                q.push(format!("workspace={}", urlencoding(&w)));
            }

            if let Some(s) = stack {
                q.push(format!("stack={}", urlencoding(&s)));
            }

            let path = if q.is_empty() { "/api/v1/names".to_owned() } else { format!("/api/v1/names?{}", q.join("&")) };

            let v = api.get(&path).await?;
            let mut rows: Vec<Value> = v["names"].as_array().cloned().unwrap_or_default();

            if let Some(k) = kind {
                rows.retain(|r| r["kind"] == k.as_str());
            }

            for r in &mut rows {
                let ports: Vec<String> = r["ports"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|p| format!("{}/{}->{}", p["port"], p["proto"].as_str().unwrap_or("tcp"), p["target"]))
                    .collect();
                r["ports_text"] = json!(ports.join(", "));
            }

            show(g, &Value::Array(rows), Some(DNS_COLS))
        }
        DnsCmd::Suffix(c) => {
            let v = match c {
                SuffixCmd::Get => api.get("/api/v1/names/suffix").await?,
                SuffixCmd::Set { suffix } => {
                    api.call(reqwest::Method::PUT, "/api/v1/names/suffix", Some(&json!({ "suffix": suffix }))).await?
                }
                SuffixCmd::Reset => {
                    api.call(reqwest::Method::PUT, "/api/v1/names/suffix", Some(&json!({ "suffix": "" }))).await?
                }
            };

            show(g, &v, Some(SUFFIX_COLS))
        }
    }
}

fn urlencoding(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

// ------------------------------------------------------------------ clusters

#[derive(Subcommand, Debug)]
pub enum ClustersCmd {
    /// List clusters.
    List {
        #[command(flatten)]
        paging: out::Paging,
    },
    /// Show one cluster (id or name).
    Get { cluster: String },
    #[command(flatten)]
    Admin(crate::admincmd::ClusterAdminCmd),
    /// The cluster's ingress profile: get, set, apply, remove.
    #[command(subcommand)]
    Ingress(crate::ingresscmd::IngressCmd),
    /// Docker Swarm of a Swarm-mode cluster: members, roles, tokens.
    #[command(subcommand)]
    Swarm(crate::containercmd::ClusterSwarmCmd),
    /// The cluster network (WireGuard mesh and container networks): get,
    /// suggest, set, apply, resync, remove.
    #[command(subcommand)]
    Network(crate::networkcmd::NetworkCmd),
}

pub async fn clusters(g: &Global, cmd: ClustersCmd) -> Result<()> {
    let cmd = match cmd {
        ClustersCmd::Admin(a) => return crate::admincmd::cluster_admin(g, a).await,
        ClustersCmd::Ingress(i) => return crate::ingresscmd::run(g, i).await,
        ClustersCmd::Swarm(s) => return crate::containercmd::cluster_swarm(g, s).await,
        ClustersCmd::Network(n) => return crate::networkcmd::run(g, n).await,
        other => other,
    };

    let t = target(g)?;
    let mut api = connect(g, &t).await?;

    match cmd {
        ClustersCmd::List { paging } => {
            let v = api.get("/api/v1/clusters").await?;
            let cols: &[Col] = &[("ID", "/id"), ("NAME", "/name"), ("ABBR", "/abbr"), ("DESCRIPTION", "/description")];
            out::list_value(g.output, v, "", Some(cols), paging);
            Ok(())
        }
        ClustersCmd::Get { cluster } => {
            // Id, name or abbreviation (case-insensitive), like the other cluster commands.
            let id = crate::admincmd::cluster_id(&mut api, &cluster).await?;
            let v = api.get(&format!("/api/v1/clusters/{id}")).await?;
            show(g, &v, None)
        }
        ClustersCmd::Admin(_) | ClustersCmd::Ingress(_) | ClustersCmd::Swarm(_) | ClustersCmd::Network(_) => Ok(()),
    }
}

// ------------------------------------------------------------------ jobs

const JOB_COLS: &[Col] =
    &[("ID", "/id"), ("KIND", "/kind"), ("STATUS", "/status"), ("STEP", "/current_step"), ("ERROR", "/last_error")];

#[derive(Subcommand, Debug)]
pub enum JobsCmd {
    /// Recent jobs.
    List {
        /// Only jobs with this status: queued, running, succeeded, failed or cancelled.
        #[arg(long)]
        status: Option<String>,
        /// Only this workspace's jobs (id).
        #[arg(long)]
        workspace: Option<String>,
        #[command(flatten)]
        paging: out::Paging,
    },
    /// One job.
    #[command(alias = "show")]
    Get { job: String },
    /// A job's steps.
    Steps { job: String },
    /// Wait until a job finishes (exit 10 if it failed, 8 on timeout; the job keeps running).
    Wait {
        job: String,
        #[arg(long, default_value_t = 1800)]
        timeout_secs: u64,
    },
    /// Follow a job: print step changes and live output until it finishes
    /// (same exit codes as `wait`).
    Watch {
        job: String,
        #[arg(long, default_value_t = 1800)]
        timeout_secs: u64,
    },
    /// Ask a queued or running job to stop.
    Cancel {
        job: String,
        #[arg(long)]
        yes: bool,
    },
}

/// Follows one job: step changes and new live output go to stderr (they are
/// progress, not the result); the final job is the result on stdout.
async fn watch_job(g: &Global, api: &mut Api, job: &str, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    let mut seen_steps = 0usize;
    let mut printed = 0usize;
    let human = matches!(g.output, Format::Table);

    loop {
        let v = api.get(&format!("/api/v1/jobs/{}", enc(job))).await?;
        let steps = api.get(&format!("/api/v1/jobs/{}/steps", enc(job))).await.unwrap_or(Value::Null);

        for s in steps.as_array().into_iter().flatten().skip(seen_steps) {
            if human {
                eprintln!(
                    "  step {} {}: {}{}",
                    s["index"],
                    s["name"].as_str().unwrap_or("?"),
                    s["status"].as_str().unwrap_or("?"),
                    s["error"].as_str().map(|e| format!(" ({e})")).unwrap_or_default()
                );
            }

            seen_steps += 1;
        }

        let live = api.get(&format!("/api/v1/jobs/{}/live", enc(job))).await.unwrap_or(Value::Null);
        if let Some(text) = live["output"].as_str() {
            if text.len() < printed {
                printed = 0; // a new operation started: its output begins again
            }

            if human && text.len() > printed {
                eprint!("{}", text.get(printed..).unwrap_or_default());
            }

            printed = text.len();
        }

        let status = v["status"].as_str().unwrap_or_default().to_owned();
        if terminal(&status) {
            // JSON mode prints one document: the job on success, else the error.
            if human || status == "succeeded" {
                show(g, &v, None)?;
            }

            return if status == "succeeded" {
                Ok(())
            } else {
                Err(CliError::new(
                    Exit::JobFailed,
                    format!("job {job} {status}: {}", v["last_error"].as_str().unwrap_or("-")),
                )
                .with_code(format!("job_{status}")))
            };
        }

        if Instant::now() >= deadline {
            return Err(CliError::new(
                Exit::Timeout,
                format!("job {job} is still {status}; it keeps running (check with `forge jobs get {job}`)"),
            ));
        }

        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

/// Waits for jobs to finish; prints nothing. `Err` with exit 10 when one
/// failed or was cancelled, 8 on timeout (the jobs keep running).
pub(crate) async fn wait_jobs(api: &mut Api, ids: &[String], timeout: Duration) -> Result<Vec<Value>> {
    let deadline = Instant::now() + timeout;
    let mut delay = Duration::from_millis(500);
    let mut done: Vec<Option<Value>> = vec![None; ids.len()];

    loop {
        for (i, id) in ids.iter().enumerate() {
            if done[i].is_none() {
                let v = api.get(&format!("/api/v1/jobs/{}", enc(id))).await?;
                if terminal(v["status"].as_str().unwrap_or_default()) {
                    done[i] = Some(v);
                }
            }
        }

        if done.iter().all(Option::is_some) {
            let jobs: Vec<Value> = done.into_iter().flatten().collect();
            if let Some(bad) = jobs.iter().find(|j| j["status"] != "succeeded") {
                let status = bad["status"].as_str().unwrap_or("failed");
                return Err(CliError::new(
                    Exit::JobFailed,
                    format!(
                        "job {} {status}: {}",
                        bad["id"].as_str().unwrap_or("?"),
                        bad["last_error"].as_str().unwrap_or("-")
                    ),
                )
                .with_code(format!("job_{status}")));
            }

            return Ok(jobs);
        }

        if Instant::now() >= deadline {
            let pending: Vec<&str> =
                ids.iter().zip(&done).filter(|(_, d)| d.is_none()).map(|(id, _)| id.as_str()).collect();
            return Err(CliError::new(
                Exit::Timeout,
                format!("still running: {}; they keep running (check with `forge jobs get <id>`)", pending.join(", ")),
            ));
        }

        tokio::time::sleep(delay).await;
        delay = (delay * 2).min(Duration::from_secs(5));
    }
}

fn terminal(status: &str) -> bool {
    matches!(status, "succeeded" | "failed" | "cancelled")
}

pub async fn jobs(g: &Global, cmd: JobsCmd) -> Result<()> {
    let t = target(g)?;
    let mut api = connect(g, &t).await?;

    match cmd {
        JobsCmd::List { status, workspace, paging } => {
            // The API returns the newest `limit` jobs: ask for every row up
            // to the end of the page, then cut the page out.
            let page: out::Page = paging.into();
            let fetch = page.fetch().unwrap_or(10_000);
            let mut path = format!("/api/v1/jobs?limit={fetch}");

            for (k, v) in [("status", status), ("workspace", workspace)] {
                if let Some(v) = v {
                    path.push_str(&format!("&{k}={}", enc(&v)));
                }
            }

            let rows = rows_of(api.get(&path).await?);
            let more = rows.len() >= fetch;
            out::list(g.output, rows, Some(JOB_COLS), page, if more { out::Total::AtLeast } else { out::Total::Rows });
            Ok(())
        }
        JobsCmd::Get { job } => {
            let v = api.get(&format!("/api/v1/jobs/{}", enc(&job))).await?;
            show(g, &v, None)
        }
        JobsCmd::Steps { job } => {
            let v = api.get(&format!("/api/v1/jobs/{}/steps", enc(&job))).await?;
            show(g, &v, Some(&[("#", "/index"), ("STEP", "/name"), ("STATUS", "/status"), ("ERROR", "/error")]))
        }
        JobsCmd::Wait { job, timeout_secs } => {
            let r = wait_jobs(&mut api, std::slice::from_ref(&job), Duration::from_secs(timeout_secs)).await;
            if let Ok(jobs) = &r {
                show(g, &jobs.first().cloned().unwrap_or(Value::Null), None)?;
            }

            r.map(|_| ())
        }
        JobsCmd::Watch { job, timeout_secs } => watch_job(g, &mut api, &job, Duration::from_secs(timeout_secs)).await,
        JobsCmd::Cancel { job, yes } => {
            confirm(g, yes, &format!("Cancel job {job} on {}?", t.server))?;
            let v = api.post(&format!("/api/v1/jobs/{}/cancel", enc(&job)), &json!({})).await?;
            show(g, &v, None)
        }
    }
}

// ------------------------------------------------------------------ audit

#[derive(Subcommand, Debug)]
#[allow(clippy::large_enum_variant)] // parsed once per run
pub enum AuditCmd {
    /// Recent entries, newest first. Filters run on the server over the
    /// newest 10,000 entries.
    List {
        #[command(flatten)]
        paging: out::Paging,
        /// Only entries older than this sequence number (a cursor: the
        /// `SEQ` of the last row you have).
        #[arg(long)]
        before: Option<i64>,
        /// Only older entries: a duration back from now (`30m`, `12h`, `7d`)
        /// or epoch milliseconds.
        #[arg(long)]
        until: Option<String>,
        /// Actions starting with this, such as `stack.` or `auth.login`.
        #[arg(long)]
        action: Option<String>,
        /// Actor containing this, such as `user:<id>` or `system:forge`.
        #[arg(long)]
        actor: Option<String>,
        /// success, denied or failure.
        #[arg(long)]
        outcome: Option<String>,
        /// Resource containing this, such as `server:<id>`.
        #[arg(long)]
        resource: Option<String>,
        /// Only newer entries: a duration (`30m`, `12h`, `7d`) or epoch milliseconds.
        #[arg(long)]
        since: Option<String>,
        /// Only this workspace's entries (id). Needs `audit.read` on that
        /// workspace instead of the platform: for auditors granted there.
        #[arg(long)]
        workspace: Option<String>,
    },
    /// Verify the hash chain and signatures (exit 1 if broken).
    Verify,
}

/// `--since`: epoch milliseconds, or a duration back from now (`90s`, `30m`, `12h`, `7d`).
fn parse_since(s: &str) -> Result<i64> {
    let s = s.trim();
    if let Ok(ms) = s.parse::<i64>() {
        return Ok(ms);
    }

    let (num, unit) = s.split_at(s.len().saturating_sub(1));
    let n: i64 = num.parse().map_err(|_| CliError::usage(format!("--since: `{s}` is not 30m, 12h, 7d or epoch ms")))?;

    let secs = match unit {
        "s" => n,
        "m" => n * 60,
        "h" => n * 3600,
        "d" => n * 86_400,
        _ => return Err(CliError::usage(format!("--since: `{s}` is not 30m, 12h, 7d or epoch ms"))),
    };

    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());

    Ok(i64::try_from(now).unwrap_or(i64::MAX) - secs * 1000)
}

pub async fn audit(g: &Global, cmd: AuditCmd) -> Result<()> {
    let t = target(g)?;
    let mut api = connect(g, &t).await?;

    match cmd {
        AuditCmd::List { paging, before, until, action, actor, outcome, resource, since, workspace } => {
            let mut filter = String::new();

            for (k, v) in [
                ("action", action),
                ("actor", actor),
                ("outcome", outcome),
                ("resource", resource),
                ("workspace", workspace),
            ] {
                if let Some(v) = v {
                    filter.push_str(&format!("&{k}={}", enc(&v)));
                }
            }

            if let Some(s) = since {
                filter.push_str(&format!("&since={}", parse_since(&s)?));
            }

            if let Some(u) = until {
                filter.push_str(&format!("&until={}", parse_since(&u)?));
            }

            // Keyset pages of up to 200 entries (`/audit/page`), walked until
            // the requested page is covered; the first page carries the total.
            let page: out::Page = paging.into();
            let want = page.fetch();
            let mut rows: Vec<Value> = Vec::new();
            let mut cursor = before;
            let mut total = None;

            loop {
                let per = want.map_or(200, |w| w.saturating_sub(rows.len()).clamp(1, 200));
                let mut path = format!("/api/v1/audit/page?limit={per}{filter}");

                if total.is_none() {
                    path.push_str("&total=true");
                }

                if let Some(c) = cursor {
                    path.push_str(&format!("&before={c}"));
                }

                let p = api.get(&path).await?;

                if total.is_none() {
                    total = p["total"].as_u64().and_then(|n| usize::try_from(n).ok());
                }

                rows.extend(p["entries"].as_array().cloned().unwrap_or_default());
                cursor = p["next_cursor"].as_i64();

                if cursor.is_none() || want.is_some_and(|w| rows.len() >= w) {
                    break;
                }
            }

            let total = match total {
                Some(n) => out::Total::Known(n),
                None if cursor.is_some() => out::Total::AtLeast,
                None => out::Total::Rows,
            };

            out::list(
                g.output,
                rows,
                Some(&[
                    ("SEQ", "/seq"),
                    ("AT", "/at"),
                    ("ACTOR", "/actor"),
                    ("ACTION", "/action"),
                    ("OUTCOME", "/outcome"),
                    ("RESOURCE", "/resource"),
                ]),
                page,
                total,
            );
            Ok(())
        }
        AuditCmd::Verify => {
            let v = api.get("/api/v1/audit/verify").await?;
            show(g, &v, None)?;

            if v["ok"] == true {
                Ok(())
            } else {
                Err(CliError::new(Exit::General, "audit chain verification failed").with_code("audit_broken"))
            }
        }
    }
}

// ------------------------------------------------------------------ plugins

#[derive(Subcommand, Debug)]
pub enum PluginsCmd {
    /// Installed plugins.
    List,
    /// Generate a publisher signing key (offline; keep the file secret).
    Keygen { out: PathBuf },
    /// Sign a plugin package (offline); prints the base64url signature.
    Sign {
        #[arg(long)]
        key: PathBuf,
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long)]
        artifact: PathBuf,
    },
    /// Install a signed package, approving the given permissions.
    Install {
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long)]
        artifact: PathBuf,
        #[arg(long)]
        signature: String,
        #[arg(long = "approve")]
        approve: Vec<String>,
        /// Replace an installed plugin with this newer version.
        #[arg(long)]
        upgrade: bool,
    },
    /// Turn a plugin on (also releases a quarantined plugin).
    Enable { id: String },
    /// Turn a plugin off; it stays installed.
    Disable { id: String },
    /// Keep a plugin on its version (refuse upgrades).
    Pin { id: String },
    /// Allow upgrades again.
    Unpin { id: String },
    /// Uninstall a plugin.
    Remove {
        id: String,
        #[arg(long)]
        yes: bool,
    },
}

pub async fn plugins(g: &Global, cmd: PluginsCmd) -> Result<()> {
    let offline = |e: String| CliError::new(Exit::General, e);

    match cmd {
        PluginsCmd::Keygen { out } => {
            if out.exists() {
                return Err(CliError::new(
                    Exit::Conflict,
                    format!("{} already exists; refusing to overwrite a key", out.display()),
                ));
            }

            let (key, pkcs8) = PublisherKey::generate().map_err(|e| offline(e.to_string()))?;
            forge_secrets::write_private(&out, &pkcs8).map_err(|e| offline(e.to_string()))?;
            show(g, &json!({ "public_key": key.public_key_b64(), "key_file": out }), None)
        }
        PluginsCmd::Sign { key, manifest, artifact } => {
            let key = PublisherKey::from_pkcs8(&std::fs::read(key)?).map_err(|e| offline(e.to_string()))?;
            let m = std::fs::read(&manifest)?;
            Manifest::parse(std::str::from_utf8(&m).map_err(|e| offline(e.to_string()))?)
                .map_err(|e| offline(e.to_string()))?;
            show(g, &Value::String(key.sign(&m, &std::fs::read(artifact)?)), None)
        }
        cmd => {
            let t = target(g)?;
            let mut api = connect(g, &t).await?;

            let v = match cmd {
                PluginsCmd::List => api.get("/api/v1/plugins").await?,
                PluginsCmd::Install { manifest, artifact, signature, approve, upgrade } => {
                    api.post(
                        if upgrade { "/api/v1/plugins/upgrade" } else { "/api/v1/plugins" },
                        &json!({
                            "manifest_toml": std::fs::read_to_string(manifest)?,
                            "artifact_base64": STANDARD.encode(std::fs::read(artifact)?),
                            "signature": signature,
                            "approved_permissions": approve,
                        }),
                    )
                    .await?
                }
                PluginsCmd::Enable { id } => {
                    api.post(&format!("/api/v1/plugins/{}/enable", enc(&id)), &json!({})).await?
                }
                PluginsCmd::Disable { id } => {
                    api.post(&format!("/api/v1/plugins/{}/disable", enc(&id)), &json!({})).await?
                }
                PluginsCmd::Pin { id } => api.post(&format!("/api/v1/plugins/{}/pin", enc(&id)), &json!({})).await?,
                PluginsCmd::Unpin { id } => {
                    api.post(&format!("/api/v1/plugins/{}/unpin", enc(&id)), &json!({})).await?
                }
                PluginsCmd::Remove { id, yes } => {
                    confirm(g, yes, &format!("Uninstall plugin {id} on {}?", t.server))?;
                    api.call(reqwest::Method::DELETE, &format!("/api/v1/plugins/{}", enc(&id)), None).await?
                }
                PluginsCmd::Keygen { .. } | PluginsCmd::Sign { .. } => Value::Null,
            };

            show(g, &v, None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// F-01: `nodes list --cluster lab` resolves the cluster name to its id
    /// before filtering on `cluster_id` (comparing the name matched nothing).
    #[test]
    fn cluster_filter_takes_a_name() {
        let clusters = json!([
            { "id": "c-lab", "name": "lab", "abbr": "LB" },
            { "id": "c-prod", "name": "prod", "abbr": "PR" },
        ]);
        let mut nodes = vec![
            json!({ "hostname": "lab-a", "cluster_id": "c-lab" }),
            json!({ "hostname": "lab-b", "cluster_id": "c-lab" }),
            json!({ "hostname": "web-1", "cluster_id": "c-prod" }),
            json!({ "hostname": "loose", "cluster_id": null }),
        ];

        for needle in ["lab", "LAB", "lb", "c-lab"] {
            let id = crate::admincmd::pick(&clusters, &["name", "abbr"], "cluster", needle).map(|c| c["id"].clone());
            assert_eq!(id.ok(), Some(json!("c-lab")), "{needle}");
        }

        assert!(crate::admincmd::pick(&clusters, &["name", "abbr"], "cluster", "nope").is_err());
        filter_cluster(&mut nodes, "c-lab");
        let names: Vec<&str> = nodes.iter().filter_map(|n| n["hostname"].as_str()).collect();
        assert_eq!(names, ["lab-a", "lab-b"]);
    }

    #[test]
    fn command_results_are_readable() {
        assert_eq!(command_result("ping", &Value::Null), json!("ping: done (no output)"));
        assert_eq!(command_result("x", &json!("active\n")), json!("active"));
        assert_eq!(command_result("x", &json!("{\"active\":true}")), json!({ "active": true }));
    }
}
