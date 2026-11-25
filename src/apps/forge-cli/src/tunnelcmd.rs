//! `forge tunnel open|list|close|ports|grants` (#35, D-091, D-100): forward
//! a local port to a service, a database or SSH on a node, like `ssh -L`,
//! through the control plane. The API port is the only way in.
//!
//! Targets are resolved against the server's service-port catalogue
//! (`GET /api/v1/service-ports`): a service, a stack service, a
//! database package or a node's SSH endpoint. A raw `--node --port` is the
//! administrators' exception. `open` listens on `--bind:<local port>`,
//! registers the tunnel, and holds its control WebSocket until Ctrl-C or
//! until the server closes it. Each accepted TCP connection gets its own
//! WebSocket to the server, which relays it to the port on the node.
//!
//! With `--direct` (#35 phase 2) each accepted connection instead gets a
//! one-minute SSH certificate from the server and goes straight to the
//! node's SSH endpoint (host key pinned), forwarding only to the tunnel's
//! destination. The control WebSocket still holds the tunnel, and its end
//! ends every direct connection.
//!
//! The local port a target got is remembered in the context file
//! (`Context::tunnel_ports`), so the same service lands on the same local
//! port next time.

use std::io::{BufRead, IsTerminal, Write as _};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use clap::{Args, Subcommand};
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Message;

use crate::api::Api;
use crate::cmds::resolve_node;
use crate::error::{CliError, Exit, Result};
use crate::out::{self, Col};
use crate::termcmd::ws_connect;
use crate::{Global, Target, connect, ctx, target};

/// What a tunnel or a grant points at. `--service` is the generic selector;
/// `--app`, `--stack` and `--db` narrow it to one kind.
#[derive(Args, Debug, Clone, Default)]
pub struct TargetArgs {
    /// A service: its service port on the nodes it runs on.
    #[arg(long, value_name = "NAME", conflicts_with_all = ["stack", "service", "db", "ssh"])]
    pub app: Option<String>,
    /// A compose stack; name the service in it with --service.
    #[arg(long, value_name = "NAME", requires = "service", conflicts_with_all = ["db", "ssh"])]
    pub stack: Option<String>,
    /// A service by name, of any kind (service, stack service or
    /// package); with --stack, the service in that stack.
    #[arg(long, value_name = "NAME", conflicts_with_all = ["db", "ssh"])]
    pub service: Option<String>,
    /// A database: an App Store package installation.
    #[arg(long, value_name = "NAME", conflicts_with = "ssh")]
    pub db: Option<String>,
    /// The node (id, hostname or name). Required for --ssh and for a raw
    /// --port; for a service it picks one of the nodes the service runs on.
    #[arg(long, short = 'n', value_name = "NODE")]
    pub node: Option<String>,
    /// The node's SSH port (needs --node).
    #[arg(long, requires = "node")]
    pub ssh: bool,
    /// One port of a service that listens on several; with --node alone, a
    /// raw port on the node (administrators, `network.tunnel.any`).
    #[arg(long, short = 'p', value_name = "PORT")]
    pub port: Option<u16>,
    /// The service's environment, when the name exists in more than one.
    #[arg(long, value_name = "ENVIRONMENT")]
    pub environment: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum TunnelCmd {
    /// Forward a local port to a service, a database or SSH on a node (runs until Ctrl-C).
    Open {
        #[command(flatten)]
        target: TargetArgs,
        /// Every port of a service that listens on several (one tunnel each).
        #[arg(long, conflicts_with = "port")]
        all: bool,
        /// The local port to listen on; default: the remembered port for
        /// this target, else a free one (which is then remembered).
        #[arg(long, short = 'l', value_name = "LOCAL_PORT")]
        local_port: Option<u16>,
        /// The local address to listen on.
        #[arg(long, default_value = "127.0.0.1")]
        bind: IpAddr,
        /// Allow --bind on an address other machines can reach. Anyone who
        /// can reach it can use the tunnel.
        #[arg(long)]
        allow_remote_bind: bool,
        /// How long the tunnel may stay open: 30m, 8h, 3d or never
        /// (`never` needs `[tunnels] allow_never_expiry` and an admin role).
        #[arg(long = "for", value_name = "DURATION", default_value = "8h")]
        lifetime: String,
        /// Connect straight to the node's SSH port instead of relaying
        /// through the Forge server. Needs the node's SSH endpoint
        /// reachable from here.
        #[arg(long)]
        direct: bool,
    },
    /// List your open tunnels.
    List {
        /// Everyone's tunnels (needs user.manage).
        #[arg(long)]
        all: bool,
    },
    /// Close a tunnel and its connections.
    Close {
        /// The tunnel id (from `forge tunnel list`).
        #[arg(value_name = "ID", required_unless_present = "id_flag")]
        id: Option<String>,
        /// Same as the positional id (kept for scripts).
        #[arg(long = "id", hide = true)]
        id_flag: Option<String>,
    },
    /// The local ports this machine remembers for tunnel targets.
    Ports {
        #[command(subcommand)]
        action: Option<PortsCmd>,
    },
    /// Explicit tunnel grants for people without a role on the service (administrators).
    #[command(subcommand)]
    Grants(GrantsCmd),
}

#[derive(Subcommand, Debug)]
pub enum PortsCmd {
    /// Forget the remembered local port of a target (`app:site@lab-b:8080`,
    /// or the target name such as `site`).
    Forget {
        /// The mapping key as `forge tunnel ports` shows it, or a target name.
        target: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum GrantsCmd {
    /// List tunnel grants.
    List {
        /// Expired and revoked grants too.
        #[arg(long)]
        all: bool,
    },
    /// Grant one person access to a service, a database, SSH or a raw port.
    Create {
        /// The person (username, email or user id).
        #[arg(long, value_name = "USER")]
        user: String,
        #[command(flatten)]
        target: TargetArgs,
        /// How long the grant lasts: 30m, 8h, 3d or never (`never` needs
        /// `[tunnels] allow_never_expiry`).
        #[arg(long = "for", value_name = "DURATION", default_value = "8h")]
        lifetime: String,
        /// Why the grant exists (shown in the list and the audit log).
        #[arg(long)]
        note: Option<String>,
    },
    /// Revoke a grant; its live tunnels close.
    Revoke {
        /// The grant id (from `forge tunnel grants list`).
        #[arg(value_name = "ID")]
        id: String,
        /// Do not ask first (required with --non-interactive).
        #[arg(long)]
        yes: bool,
    },
}

const COLS: &[Col] = &[
    ("ID", "/id"),
    ("TARGET", "/target_text"),
    ("NODE", "/node"),
    ("PORT", "/port"),
    ("BIND", "/bind"),
    ("CONNECTIONS", "/connections"),
    ("UP", "/up"),
    ("DOWN", "/down"),
    ("EXPIRES", "/expires"),
    ("USER", "/user"),
];

const GRANT_COLS: &[Col] = &[
    ("ID", "/id"),
    ("USER", "/user_text"),
    ("TARGET", "/target_text"),
    ("NODE", "/node"),
    ("PORTS", "/ports_text"),
    ("EXPIRES", "/expires"),
    ("STATUS", "/status"),
    ("NOTE", "/note"),
];

const PORT_COLS: &[Col] =
    &[("TARGET", "/target"), ("NODE", "/node"), ("REMOTE", "/remote"), ("LOCAL", "/local"), ("KEY", "/key")];

/// `12.3 MB` for the table.
fn size(b: u64) -> String {
    #[allow(clippy::cast_precision_loss)]
    let f = b as f64;
    match b {
        0..1_000 => format!("{b} B"),
        1_000..1_000_000 => format!("{:.1} kB", f / 1e3),
        1_000_000..1_000_000_000 => format!("{:.1} MB", f / 1e6),
        _ => format!("{:.1} GB", f / 1e9),
    }
}

/// `--for 30m|8h|3d|never` → minutes, `None` for never.
pub(crate) fn parse_lifetime(s: &str) -> Result<Option<u64>> {
    let s = s.trim().to_ascii_lowercase();
    if s == "never" {
        return Ok(None);
    }

    let bad = || CliError::usage(format!("--for {s}: use a duration like 30m, 8h or 3d, or `never`"));
    let mut minutes: u64 = 0;
    let mut digits = String::new();
    let mut seen = false;

    for c in s.chars() {
        if c.is_ascii_digit() {
            digits.push(c);
            continue;
        }

        let n: u64 = digits.parse().map_err(|_| bad())?;
        digits.clear();

        let per = match c {
            'm' => 1,
            'h' => 60,
            'd' => 24 * 60,
            'w' => 7 * 24 * 60,
            _ => return Err(bad()),
        };

        minutes = minutes.checked_add(n.checked_mul(per).ok_or_else(bad)?).ok_or_else(bad)?;
        seen = true;
    }

    if !digits.is_empty() || !seen || minutes == 0 {
        return Err(bad());
    }

    Ok(Some(minutes))
}

/// `8 h`, `30 min`, `3 d` for "expires in …".
fn expires_in(ms_left: i64) -> String {
    let min = ms_left / 60_000;
    if min < 1 {
        "less than a minute".into()
    } else if min < 90 {
        format!("{min} min")
    } else if min < 48 * 60 {
        format!("{} h", (min + 30) / 60)
    } else {
        format!("{} d", (min + 12 * 60) / (24 * 60))
    }
}

/// `(expires in 8 h` / `(never expires` from a row's `expires_at`.
pub(crate) fn expiry_text(v: &Value) -> String {
    match v["expires_at"].as_i64() {
        Some(at) => format!("expires in {}", expires_in(at - crate::api::now_ms())),
        None => "never expires".into(),
    }
}

/// The `{kind, ref}` of a tunnel or grant as one word pair (`app site`).
fn target_text(t: &Value) -> String {
    let kind = t["kind"].as_str().unwrap_or("?");

    // The server stores the id in `ref` and sends the human name alongside.
    let r = t["name"].as_str().filter(|n| !n.is_empty()).or_else(|| t["ref"].as_str()).unwrap_or("?");

    match kind {
        "ssh" => "ssh".to_owned(),
        "raw" => "raw port".to_owned(),
        k => format!("{k} {r}"),
    }
}

/// Catalogue response → entries, whatever the wrapper is.
fn entries_of(v: Value) -> Vec<Value> {
    match v {
        Value::Array(a) => a,
        // Catalogue answers are bare arrays; `GET /tunnels` wraps under
        // `tunnels` and `GET /tunnel-grants` under `grants`.
        Value::Object(mut o) => ["ports", "entries", "items", "services", "tunnels", "grants", "result"]
            .iter()
            .find_map(|k| o.remove(*k).and_then(|x| x.as_array().cloned()))
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v[k].as_str().unwrap_or_default()
}

/// One port of a resolved target on one node.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PortChoice {
    port: u16,
    role: Option<String>,
}

/// A resolved target: what the server gets and what the user sees.
#[derive(Debug, Clone)]
struct Resolved {
    /// `app`, `stack`, `package`, `ssh` or `raw`.
    kind: String,
    /// The reference the server resolves: a name (`site`, `seq-b/seq`,
    /// `orders-pg`) or, for `ssh`/`raw`, the node id.
    reference: String,
    /// Shown to people: `site (web)`, `lab-b`.
    label: String,
    /// The target part of the remembered-port key.
    key_target: String,
    /// The chosen node, when one is chosen.
    node_id: Option<String>,
    node_name: String,
    /// Every node the service runs on (names), for the summary line.
    nodes: Vec<String>,
    /// Every port on the chosen node.
    ports: Vec<PortChoice>,
    /// The ports to open or grant (empty in a grant: every port).
    chosen: Vec<u16>,
}

impl Resolved {
    fn role_of(&self, port: u16) -> Option<&str> {
        self.ports.iter().find(|p| p.port == port).and_then(|p| p.role.as_deref())
    }
}

/// Which decisions `resolve` has to make.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// A node and the ports to open are needed.
    Open { all: bool },
    /// Node and ports are optional (a grant can cover every node and port).
    Grant,
}

fn node_label(n: &Value) -> String {
    n["display_name"].as_str().or(n["hostname"].as_str()).or(n["name"].as_str()).unwrap_or("?").to_owned()
}

fn name_of(servers: &[Value], id: &str, fallback: &str) -> String {
    servers
        .iter()
        .find(|n| n["id"].as_str() == Some(id))
        .map_or_else(|| if fallback.is_empty() { id.to_owned() } else { fallback.to_owned() }, node_label)
}

/// True when a catalogue entry is the service `needle` (in `stack`, when given).
fn matches_service(e: &Value, needle: &str, stack: Option<&str>) -> bool {
    let eq = |a: &str, b: &str| a.eq_ignore_ascii_case(b);
    let name = s(e, "name");
    let label = s(e, "label");
    let in_stack = stack.is_none_or(|st| {
        eq(s(e, "stack"), st)
            || name.rsplit_once('/').is_some_and(|(a, _)| eq(a, st))
            || label.rsplit_once('/').is_some_and(|(a, _)| eq(a, st))
            || label.to_ascii_lowercase().contains(&st.to_ascii_lowercase())
    });
    let is_service = eq(name, needle)
        || eq(label, needle)
        || eq(s(e, "id"), needle)
        || name.rsplit_once('/').is_some_and(|(_, b)| eq(b, needle))
        || stack.is_some_and(|st| eq(name, &format!("{st}/{needle}")));

    in_stack && is_service
}

/// Resolves the target flags to a node (where needed) and its ports.
async fn resolve(g: &Global, api: &mut Api, a: &TargetArgs, mode: Mode) -> Result<Resolved> {
    // SSH and raw ports: the node is the target.
    if a.ssh || (a.port.is_some() && a.app.is_none() && a.stack.is_none() && a.service.is_none() && a.db.is_none()) {
        let Some(node) = &a.node else {
            return Err(CliError::usage("--port alone is a raw port on a node: add --node <NODE>, or name a service"));
        };

        let n = resolve_node(api, node).await?;
        let node_id = s(&n, "id").to_owned();
        let node_name = node_label(&n);

        let (kind, port, role) = if a.ssh {
            let ssh = api.get(&format!("/api/v1/servers/{node_id}/ssh")).await?;
            let port = ssh["endpoint"]["port"].as_u64().and_then(|p| u16::try_from(p).ok()).unwrap_or(22);
            ("ssh", port, Some("ssh".to_owned()))
        } else {
            let Some(port) = a.port else {
                return Err(CliError::usage("name a service (--app, --stack --service, --db), --ssh or --port"));
            };

            ("raw", port, None)
        };

        if a.ssh && a.port.is_some_and(|p| p != port) {
            return Err(CliError::usage(format!("--ssh uses the node's SSH port ({port}); drop --port")));
        }

        return Ok(Resolved {
            kind: kind.to_owned(),
            reference: node_id.clone(),
            label: node_name.clone(),
            key_target: node_name.clone(),
            node_id: Some(node_id),
            node_name: node_name.clone(),
            nodes: vec![node_name],
            ports: vec![PortChoice { port, role }],
            chosen: vec![port],
        });
    }

    // A stack service is catalogued as `<stack>/<service>`; ask for it by that
    // name, or the server finds nothing for a bare service name.
    let stack_needle = match (&a.stack, &a.service) {
        (Some(st), Some(svc)) if !svc.contains('/') => Some(format!("{st}/{svc}")),
        _ => None,
    };

    let (kind, needle) = match (&a.app, &a.stack, &a.service, &a.db) {
        (Some(app), _, _, _) => (Some("app"), app.as_str()),
        (_, Some(_), Some(svc), _) => (Some("stack"), stack_needle.as_deref().unwrap_or(svc.as_str())),
        (_, _, _, Some(db)) => (Some("package"), db.as_str()),
        (_, _, Some(svc), _) => (None, svc.as_str()),
        _ => return Err(CliError::usage("name a target: --app, --stack --service, --service, --db, or --node --ssh")),
    };

    // The catalogue: the node's own, or the whole fleet's for this name.
    let pinned = match &a.node {
        Some(n) => Some(resolve_node(api, n).await?),
        None => None,
    };

    let raw = match &pinned {
        Some(n) => api.get(&format!("/api/v1/servers/{}/service-ports", s(n, "id"))).await?,
        None => {
            let mut q = format!("/api/v1/service-ports?service={}", crate::admincmd::enc(needle));

            if let Some(k) = kind {
                q.push_str(&format!("&kind={k}"));
            }

            if let Some(ns) = &a.environment {
                q.push_str(&format!("&environment={}", crate::admincmd::enc(ns)));
            }

            api.get(&q).await?
        }
    };

    let what = match kind {
        Some("app") => "service",
        Some("stack") => "stack service",
        Some("package") => "database",
        _ => "service",
    };

    let mut entries: Vec<Value> = entries_of(raw)
        .into_iter()
        .filter(|e| matches_service(e, needle, a.stack.as_deref()))
        .filter(|e| kind.is_none_or(|k| s(e, "kind") == k))
        .filter(|e| a.environment.as_deref().is_none_or(|ns| s(e, "environment") == ns || s(e, "environment_id") == ns))
        .collect();

    if entries.is_empty() {
        let where_ = pinned.as_ref().map(|n| format!(" on {}", node_label(n))).unwrap_or_default();
        return Err(CliError::new(
            Exit::NotFound,
            format!("no {what} `{needle}`{where_} with a listening port that you can see"),
        ));
    }

    // One kind and one environment, or the name is ambiguous.
    let mut kinds: Vec<(String, String, String)> = entries
        .iter()
        .map(|e| (s(e, "kind").to_owned(), s(e, "environment").to_owned(), s(e, "name").to_owned()))
        .collect();
    kinds.sort();
    kinds.dedup();

    if kinds.len() > 1 {
        let list: Vec<String> = kinds
            .iter()
            .map(|(k, ns, n)| if ns.is_empty() { format!("{k} {n}") } else { format!("{k} {n} ({ns})") })
            .collect();
        return Err(CliError::usage(format!(
            "`{needle}` matches {} services: {}; narrow it with --app, --stack, --db or --environment",
            kinds.len(),
            list.join(", ")
        ))
        .with_code("ambiguous"));
    }

    let first = entries.first().cloned().unwrap_or(Value::Null);
    let kind = s(&first, "kind").to_owned();
    let name = s(&first, "name").to_owned();
    let ns = s(&first, "environment").to_owned();

    let reference = match (&kind[..], &a.stack) {
        ("stack", Some(st)) if !name.contains('/') => format!("{st}/{name}"),
        _ => name.clone(),
    };

    let label = if ns.is_empty() { name.clone() } else { format!("{name} ({ns})") };

    // Nodes, in catalogue order; prefer one that is online.
    let servers = api.get("/api/v1/servers").await.ok().and_then(|v| v.as_array().cloned()).unwrap_or_default();
    let mut node_ids: Vec<String> = Vec::new();

    for e in &entries {
        let id = s(e, "server_id").to_owned();
        if !id.is_empty() && !node_ids.contains(&id) {
            node_ids.push(id);
        }
    }

    let nodes: Vec<String> = node_ids
        .iter()
        .map(|id| {
            let fallback = entries.iter().find(|e| s(e, "server_id") == id).map(|e| s(e, "server")).unwrap_or_default();
            name_of(&servers, id, fallback)
        })
        .collect();
    let online = |id: &str| servers.iter().any(|n| n["id"].as_str() == Some(id) && n["status"] == "online");

    let node_id: Option<String> = match (&pinned, mode) {
        (Some(n), _) => Some(s(n, "id").to_owned()),
        (None, Mode::Grant) => None,
        (None, Mode::Open { .. }) => node_ids.iter().find(|id| online(id)).or(node_ids.first()).cloned(),
    };

    if let Some(id) = &node_id {
        entries.retain(|e| s(e, "server_id") == id);
    }

    let node_name = node_id.as_deref().map(|id| name_of(&servers, id, "")).unwrap_or_default();
    let mut ports: Vec<PortChoice> = Vec::new();

    for e in &entries {
        let Some(port) = e["port"].as_u64().and_then(|p| u16::try_from(p).ok()) else { continue };
        if !ports.iter().any(|p| p.port == port) {
            ports.push(PortChoice { port, role: e["role"].as_str().map(str::to_owned) });
        }
    }

    if ports.is_empty() {
        return Err(CliError::new(Exit::NotFound, format!("{label} has no listening port on {node_name}")));
    }

    let chosen = match (mode, a.port) {
        (_, Some(p)) => {
            if !ports.iter().any(|c| c.port == p) {
                let have: Vec<String> = ports.iter().map(|c| c.port.to_string()).collect();
                return Err(CliError::usage(format!(
                    "{label} does not listen on {p} on {node_name}; its ports: {}",
                    have.join(", ")
                )));
            }

            vec![p]
        }
        (Mode::Grant, None) => Vec::new(),
        (Mode::Open { all: true }, None) => ports.iter().map(|c| c.port).collect(),
        (Mode::Open { all: false }, None) if ports.len() == 1 => vec![ports[0].port],
        (Mode::Open { all: false }, None) => vec![pick_port(g, &label, &node_name, &ports)?],
    };

    let key_target = reference_display(&kind, &name, a);

    Ok(Resolved { kind, reference, label, key_target, node_id, node_name, nodes, ports, chosen })
}

/// The target part of the remembered-port key: `site`, `seq-b/seq`, `orders-pg`.
fn reference_display(kind: &str, name: &str, a: &TargetArgs) -> String {
    match (kind, &a.stack) {
        ("stack", Some(st)) if !name.contains('/') => format!("{st}/{name}"),
        _ => name.to_owned(),
    }
}

/// Asks which port to open when a service listens on several.
fn pick_port(g: &Global, label: &str, node: &str, ports: &[PortChoice]) -> Result<u16> {
    let describe = |c: &PortChoice| match &c.role {
        Some(r) => format!("{} ({r})", c.port),
        None => c.port.to_string(),
    };

    let list: Vec<String> = ports.iter().map(describe).collect();

    if g.non_interactive || !std::io::stdin().is_terminal() || g.output.is_machine() {
        return Err(CliError::usage(format!(
            "{label} listens on {} on {node}; pick one with --port, or open every port with --all",
            list.join(", ")
        ))
        .with_code("choice_needed"));
    }

    eprintln!("{label} listens on several ports on {node}:");

    for (i, d) in list.iter().enumerate() {
        eprintln!("  {}) {d}", i + 1);
    }

    eprint!("Port to open [1-{}]: ", list.len());
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    let answer = line.trim();
    let by_index = answer.parse::<usize>().ok().filter(|i| (1..=ports.len()).contains(i)).map(|i| ports[i - 1].port);
    let by_port = answer.parse::<u16>().ok().filter(|p| ports.iter().any(|c| c.port == *p));

    by_index.or(by_port).ok_or_else(|| CliError::new(Exit::General, "cancelled").with_code("cancelled"))
}

/// The server's target object for a resolution.
fn target_body(r: &Resolved, ports: &[u16]) -> Value {
    let mut t = json!({ "kind": r.kind, "ref": r.reference });

    if let Some(id) = &r.node_id {
        t["server_id"] = json!(id);
    }

    if !ports.is_empty() {
        t["ports"] = json!(ports);
    }

    t
}

/// `lifetime_minutes` or `never_expires` for a request body.
fn lifetime_body(body: &mut Value, lifetime: &str) -> Result<()> {
    match parse_lifetime(lifetime)? {
        Some(min) => body["lifetime_minutes"] = json!(min),
        None => body["never_expires"] = json!(true),
    }

    Ok(())
}

pub async fn run(g: &Global, cmd: TunnelCmd) -> Result<()> {
    if let TunnelCmd::Ports { action } = cmd {
        return ports(g, action);
    }
    let t = target(g)?;
    let mut api = connect(g, &t).await?;

    match cmd {
        TunnelCmd::Open { target: ta, all, local_port, bind, allow_remote_bind, lifetime, direct } => {
            let how = How { local_port, bind, allow_remote_bind, direct };
            open(g, t, api, &ta, all, how, &lifetime).await
        }
        TunnelCmd::List { all } => {
            let mut v = api.get(&format!("/api/v1/tunnels{}", if all { "?all=true" } else { "" })).await?;
            let nodes = api.get("/api/v1/servers").await.ok().and_then(|v| v.as_array().cloned()).unwrap_or_default();
            let mut rows = if v.is_array() { v } else { v["tunnels"].take() };

            for r in rows.as_array_mut().into_iter().flatten() {
                let id = s(r, "server_id").to_owned();
                let fallback = s(r, "server").to_owned();
                r["node"] = json!(name_of(&nodes, &id, &fallback));
                r["target_text"] = json!(target_text(&r["target"]));
                r["up"] = json!(size(r["bytes_up"].as_u64().unwrap_or(0)));
                r["down"] = json!(size(r["bytes_down"].as_u64().unwrap_or(0)));
                r["expires"] = json!(expiry_text(r).trim_start_matches("expires ").to_owned());
            }

            out::result(g.output, &rows, Some(COLS));
            Ok(())
        }
        TunnelCmd::Close { id, id_flag } => {
            let id = id.or(id_flag).ok_or_else(|| CliError::usage("which tunnel? pass its id"))?;
            api.call(reqwest::Method::DELETE, &format!("/api/v1/tunnels/{}", crate::admincmd::enc(&id)), None).await?;
            out::result(g.output, &json!({ "closed": id }), None);
            Ok(())
        }
        TunnelCmd::Grants(cmd) => grants(g, &mut api, cmd).await,
        TunnelCmd::Ports { .. } => Ok(()),
    }
}

// ------------------------------------------------------------------ open

/// Local port for a target: `-l`, the remembered one, or a free one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pick {
    Given(u16),
    Remembered(u16),
    Fresh,
}

fn remembered(t: &Target, key: &str) -> Option<u16> {
    let name = t.context.as_deref()?;
    ctx::Config::load().ok()?.contexts.get(name)?.tunnel_ports.get(key).copied()
}

fn remember(t: &Target, key: &str, port: u16) {
    let Some(name) = t.context.clone() else { return };
    let key = key.to_owned();
    let _ = ctx::Config::update(|cfg| {
        if let Some(c) = cfg.contexts.get_mut(&name) {
            c.tunnel_ports.insert(key, port);
        }

        Ok(())
    });
}

/// A client hint for a database port, when the engine is recognisable.
fn db_hint(r: &Resolved, port: u16, local: SocketAddr) -> Option<String> {
    let text = format!("{} {} {}", r.reference, r.label, r.role_of(port).unwrap_or_default()).to_ascii_lowercase();
    let host = local.ip();
    let lp = local.port();

    let hint = if text.contains("postgres") || text.contains("-pg") || text.contains("pgsql") || port == 5432 {
        format!("psql -h {host} -p {lp} -U postgres")
    } else if text.contains("mysql") || text.contains("mariadb") || port == 3306 {
        format!("mysql -h {host} -P {lp} -u root -p")
    } else if text.contains("redis") || text.contains("valkey") || port == 6379 {
        format!("redis-cli -h {host} -p {lp}")
    } else if text.contains("mssql") || text.contains("sqlserver") || text.contains("sql-server") || port == 1433 {
        format!("sqlcmd -S tcp:{host},{lp} -U sa")
    } else if text.contains("mongo") || port == 27017 {
        format!("mongosh mongodb://{host}:{lp}")
    } else {
        return None;
    };

    Some(hint)
}

/// The local side as a person types it: `http://127.0.0.1:18080` for an
/// http port, the bare address otherwise.
fn local_text(role: Option<&str>, local: SocketAddr) -> String {
    match role {
        Some("http") | Some("web") | Some("ui") => format!("http://{local}"),
        Some("https") => format!("https://{local}"),
        _ => local.to_string(),
    }
}

/// One open tunnel being held.
struct Held {
    id: String,
    listener: TcpListener,
    control: crate::termcmd::Ws,
    direct: bool,
}

/// How `open` listens and connects.
#[derive(Debug, Clone, Copy)]
struct How {
    local_port: Option<u16>,
    bind: IpAddr,
    allow_remote_bind: bool,
    direct: bool,
}

async fn open(g: &Global, t: Target, mut api: Api, ta: &TargetArgs, all: bool, how: How, lifetime: &str) -> Result<()> {
    let How { local_port, bind, allow_remote_bind, direct } = how;
    if !bind.is_loopback() {
        if !allow_remote_bind {
            return Err(CliError::usage(format!(
                "--bind {bind} lets other machines use the tunnel; add --allow-remote-bind if that is what you want"
            )));
        }

        eprintln!("warning: listening on {bind}: anyone who can reach this address can use the tunnel");
    }

    parse_lifetime(lifetime)?;
    let r = resolve(g, &mut api, ta, Mode::Open { all }).await?;
    if local_port.is_some() && r.chosen.len() > 1 {
        return Err(CliError::usage("--local-port fits one tunnel; drop it with --all, or pick one port with --port"));
    }

    let Some(node_id) = r.node_id.clone() else {
        return Err(CliError::new(Exit::General, "the catalogue named no node for this service"));
    };

    let human = !g.output.is_machine();
    if human {
        let ports: Vec<String> = r.ports.iter().map(|p| p.port.to_string()).collect();

        match r.kind.as_str() {
            "ssh" => eprintln!("{} · SSH on {}", r.label, ports.join(", ")),
            "raw" => eprintln!("{} · port {} (raw)", r.label, ports.join(", ")),
            _ => eprintln!("{} runs on {} · listens on {}", r.label, r.nodes.join(", "), ports.join(", ")),
        }
    }

    let token = api.access_token().await?.ok_or_else(|| CliError::auth("not signed in"))?;
    let user = t.context.as_deref().and_then(|n| ctx::Config::load().ok()?.contexts.get(n)?.user.clone());
    let mut held = Vec::new();
    let mut machine = Vec::new();

    for port in r.chosen.clone() {
        let key = format!("{}:{}@{}:{port}", r.kind, r.key_target, r.node_name);
        let pick = match (local_port, remembered(&t, &key)) {
            (Some(p), _) => Pick::Given(p),
            (None, Some(p)) => Pick::Remembered(p),
            (None, None) => Pick::Fresh,
        };

        // Listen first: a busy local port fails before anything opens remotely.
        let (listener, pick) = match pick {
            Pick::Given(p) => (bind_local(SocketAddr::new(bind, p)).await?, pick),
            Pick::Remembered(p) => match bind_local(SocketAddr::new(bind, p)).await {
                Ok(l) => (l, pick),
                Err(_) => {
                    eprintln!("remembered local port {p} is busy; picking another");
                    (bind_local(SocketAddr::new(bind, 0)).await?, Pick::Fresh)
                }
            },
            Pick::Fresh => (bind_local(SocketAddr::new(bind, 0)).await?, Pick::Fresh),
        };

        let local = listener.local_addr().map_err(|e| CliError::new(Exit::General, e.to_string()))?;
        let mut body = json!({ "target": target_body(&r, &[port]), "bind": local.to_string(), "direct": direct });
        lifetime_body(&mut body, lifetime)?;
        let tunnel = api.post("/api/v1/tunnels", &body).await?;
        let id = s(&tunnel, "id").to_owned();
        if id.is_empty() {
            return Err(CliError::new(Exit::General, "the server opened a tunnel without an id; update the CLI"));
        }

        let role = tunnel["role"].as_str().map(str::to_owned).or_else(|| r.role_of(port).map(str::to_owned));
        let remote_port = tunnel["port"].as_u64().and_then(|p| u16::try_from(p).ok()).unwrap_or(port);
        let node_name = tunnel["server"].as_str().filter(|x| !x.is_empty()).unwrap_or(&r.node_name).to_owned();
        let control = ws_connect(&t, &token, &format!("/api/v1/tunnels/{id}/control")).await?;

        if matches!(pick, Pick::Fresh) {
            remember(&t, &key, local.port());
        }

        let shown = local_text(role.as_deref(), local);

        if human {
            let mem = if matches!(pick, Pick::Remembered(_)) { " (remembered)" } else { "" };
            let via = if direct { "direct SSH, " } else { "" };
            eprintln!(
                "→ {node_name}:{remote_port}  →  {shown}{mem}   ({via}{}, Ctrl-C to close)",
                expiry_text(&tunnel)
            );

            if r.kind == "ssh" {
                eprintln!("  ssh -p {} {}@{}", local.port(), user.as_deref().unwrap_or("<user>"), local.ip());
            } else if r.kind == "package"
                && let Some(h) = db_hint(&r, port, local)
            {
                eprintln!("  {h}");
            }
        }

        machine.push(json!({
            "id": id, "local": local.to_string(), "url": shown, "node": node_name, "server_id": node_id,
            "port": remote_port, "role": role, "target": target_body(&r, &[port]),
            "expires_at": tunnel["expires_at"], "remembered": matches!(pick, Pick::Remembered(_)),
            "direct": direct,
        }));
        held.push(Held { id, listener, control, direct });
    }

    if !human {
        let v = if machine.len() == 1 { machine.remove(0) } else { Value::Array(machine) };
        out::result(g.output, &v, None);
    }

    let api = Arc::new(Mutex::new(api));
    let runs = held.into_iter().map(|h| hold(t.clone(), api.clone(), token.clone(), h));
    futures::future::join_all(runs).await;

    Ok(())
}

async fn bind_local(addr: SocketAddr) -> Result<TcpListener> {
    TcpListener::bind(addr).await.map_err(|e| CliError::new(Exit::General, format!("cannot listen on {addr}: {e}")))
}

/// Serves one tunnel until Ctrl-C or the server closes it.
async fn hold(t: Target, api: Arc<Mutex<Api>>, token: String, h: Held) {
    let Held { id, listener, mut control, direct } = h;
    let mut last_token = token;
    let mut refresh = tokio::time::interval(Duration::from_secs(60));
    refresh.tick().await;

    // Direct connections: counted for the server's idle check, and ended
    // with the tunnel (they do not pass through the server).
    let active = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut conns = tokio::task::JoinSet::new();

    let reason = loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break "closed here".to_owned(),
            accepted = listener.accept() => match accepted {
                Ok((sock, peer)) if direct => {
                    let (api, id, active) = (api.clone(), id.clone(), active.clone());
                    conns.spawn(async move {
                        active.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        if let Err(e) = crate::tunnel_direct::connect(&api, &id, sock).await {
                            eprintln!("connection from {peer}: {e}");
                        }
                        active.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
                    });
                }
                Ok((sock, peer)) => {
                    let (t, api, id) = (t.clone(), api.clone(), id.clone());
                    tokio::spawn(async move {
                        if let Err(e) = relay(&t, &api, &id, sock).await {
                            eprintln!("connection from {peer}: {e}");
                        }
                    });
                }
                Err(e) => break format!("listening failed: {e}"),
            },
            // Keep the server following this sign-in when the token rotates.
            _ = refresh.tick() => {
                if active.load(std::sync::atomic::Ordering::Relaxed) > 0 {
                    let _ = control.send(Message::Text(json!({ "type": "active" }).to_string().into())).await;
                }
                while conns.try_join_next().is_some() {}
                let fresh = api.lock().await.access_token().await.ok().flatten();
                if let Some(tok) = fresh.filter(|tok| *tok != last_token) {
                    let frame = json!({ "type": "refresh", "token": tok }).to_string();
                    let _ = control.send(Message::Text(frame.into())).await;
                    last_token = tok;
                }
            }
            msg = control.next() => match msg {
                Some(Ok(Message::Text(text))) => {
                    let v: Value = serde_json::from_str(text.as_str()).unwrap_or(Value::Null);
                    if v["type"] == "closed" {
                        break format!("closed by the server ({})", v["reason"].as_str().unwrap_or("unknown"));
                    }
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break "the server closed the tunnel".to_owned(),
                Some(Ok(_)) => {}
            },
        }
    };

    conns.abort_all();
    let _ = control.close(None).await;
    eprintln!("Tunnel {id}: {reason}");
}

/// One accepted connection: a WebSocket per connection, bytes both ways.
async fn relay(t: &Target, api: &Arc<Mutex<Api>>, id: &str, sock: TcpStream) -> Result<()> {
    let _ = sock.set_nodelay(true);
    let token = api.lock().await.access_token().await?.ok_or_else(|| CliError::auth("not signed in"))?;
    let ws = ws_connect(t, &token, &format!("/api/v1/tunnels/{id}/connect")).await?;
    let (mut ws_tx, mut ws_rx) = ws.split();
    let (mut rd, mut wr) = sock.into_split();

    // Each direction ends on its own (half-close): local EOF sends `eof` and
    // keeps receiving; the server's `eof` ends our writes. A closed socket
    // (the tunnel ended) stops both.
    let closed = tokio::sync::Notify::new();
    let up = async {
        let mut buf = vec![0u8; 64 * 1024];

        loop {
            tokio::select! {
                r = rd.read(&mut buf) => match r {
                    Ok(0) | Err(_) => {
                        let eof = json!({ "type": "eof" }).to_string();
                        let _ = ws_tx.send(Message::Text(eof.into())).await;
                        break;
                    }
                    Ok(n) => {
                        if ws_tx.send(Message::Binary(buf[..n].to_vec().into())).await.is_err() {
                            break;
                        }
                    }
                },
                () = closed.notified() => break,
            }
        }

        ws_tx
    };
    let down = async {
        while let Some(Ok(m)) = ws_rx.next().await {
            match m {
                Message::Binary(b) => {
                    if wr.write_all(&b).await.is_err() {
                        break;
                    }
                }
                Message::Text(text) => {
                    let v: Value = serde_json::from_str(text.as_str()).unwrap_or(Value::Null);
                    if v["type"] == "eof" {
                        // The node's side finished sending.
                        let _ = wr.shutdown().await;
                        return;
                    }

                    if let Some(e) = v["message"].as_str() {
                        eprintln!("the node refused the connection: {e}");
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }

        let _ = wr.shutdown().await;
        closed.notify_one();
    };
    let (mut ws_tx, ()) = tokio::join!(up, down);
    let _ = ws_tx.close().await;

    Ok(())
}

// ------------------------------------------------------------------ ports (remembered)

/// `kind:target@node:remote` → `(kind, target, node, remote)`.
fn split_key(key: &str) -> Option<(&str, &str, &str, &str)> {
    let (kind, rest) = key.split_once(':')?;
    let (target, node_port) = rest.rsplit_once('@')?;
    let (node, remote) = node_port.rsplit_once(':')?;

    Some((kind, target, node, remote))
}

fn ports(g: &Global, action: Option<PortsCmd>) -> Result<()> {
    let t = target(g)?;

    let Some(name) = t.context.clone() else {
        return Err(CliError::usage("remembered ports belong to a context; select one (`forge contexts use`)"));
    };

    match action {
        None => {
            let cfg = ctx::Config::load()?;
            let map = cfg.get(&name).map(|c| c.tunnel_ports.clone()).unwrap_or_default();
            let rows: Vec<Value> = map
                .iter()
                .map(|(k, local)| {
                    let (kind, tgt, node, remote) = split_key(k).unwrap_or(("?", k, "?", "?"));
                    let target = if matches!(kind, "ssh" | "raw") { kind.to_owned() } else { format!("{kind} {tgt}") };
                    json!({ "key": k, "kind": kind, "target": target, "node": node, "remote": remote, "local": local })
                })
                .collect();
            out::result(g.output, &Value::Array(rows), Some(PORT_COLS));
            Ok(())
        }
        Some(PortsCmd::Forget { target: what }) => {
            let gone = ctx::Config::update(|cfg| {
                let Some(c) = cfg.contexts.get_mut(&name) else { return Ok(Vec::new()) };
                let keys: Vec<String> = c.tunnel_ports.keys().cloned().collect();
                let exact: Vec<String> = keys.iter().filter(|k| *k == &what).cloned().collect();
                let by_target: Vec<String> = keys
                    .iter()
                    .filter(|k| split_key(k).is_some_and(|(_, tgt, node, _)| tgt == what || node == what))
                    .cloned()
                    .collect();
                let loose: Vec<String> = keys.iter().filter(|k| k.contains(&what)).cloned().collect();
                let hit = [exact, by_target, loose].into_iter().find(|v| !v.is_empty()).unwrap_or_default();

                for k in &hit {
                    c.tunnel_ports.remove(k);
                }

                Ok(hit)
            })?;

            if gone.is_empty() {
                return Err(CliError::new(
                    Exit::NotFound,
                    format!("no remembered port matches `{what}` (see `forge tunnel ports`)"),
                ));
            }

            out::result(g.output, &json!({ "forgotten": gone }), None);
            Ok(())
        }
    }
}

// ------------------------------------------------------------------ grants

async fn grants(g: &Global, api: &mut Api, cmd: GrantsCmd) -> Result<()> {
    let base = "/api/v1/tunnel-grants";

    match cmd {
        GrantsCmd::List { all } => {
            let v = api.get(&format!("{base}{}", if all { "?all=true" } else { "" })).await?;
            let nodes = api.get("/api/v1/servers").await.ok().and_then(|v| v.as_array().cloned()).unwrap_or_default();
            let mut rows = entries_of(v);

            for r in &mut rows {
                let tgt = r["target"].clone();
                let sid = tgt["server_id"].as_str().or(r["server_id"].as_str()).unwrap_or_default().to_owned();
                r["node"] = json!(if sid.is_empty() {
                    "any".to_owned()
                } else {
                    name_of(&nodes, &sid, r["server"].as_str().unwrap_or_default())
                });

                let ref_text = match tgt["kind"].as_str() {
                    Some("ssh") => "ssh".to_owned(),
                    Some("raw") => "raw port".to_owned(),
                    _ => target_text(&tgt),
                };

                r["target_text"] = json!(ref_text);
                r["ports_text"] = json!(match &tgt["ports"] {
                    Value::Array(p) if !p.is_empty() => {
                        p.iter().map(|x| x.to_string().trim_matches('"').to_owned()).collect::<Vec<_>>().join(",")
                    }
                    Value::String(x) => x.clone(),
                    _ => "*".to_owned(),
                });
                r["user_text"] = r["user"]
                    .as_str()
                    .or(r["username"].as_str())
                    .or(r["user_name"].as_str())
                    .or(r["user_id"].as_str())
                    .map_or(Value::Null, |u| json!(u));
                r["expires"] = json!(expiry_text(r).trim_start_matches("expires ").to_owned());

                if r["status"].is_null() {
                    r["status"] = json!(if r["revoked_at"].is_null() { "active" } else { "revoked" });
                }
            }

            out::result(g.output, &Value::Array(rows), Some(GRANT_COLS));
            Ok(())
        }
        GrantsCmd::Create { user, target: ta, lifetime, note } => {
            parse_lifetime(&lifetime)?;
            let users = api.get("/api/v1/users").await?;
            let u = crate::admincmd::pick(
                &if users.is_array() { users } else { users["users"].clone() },
                &["username", "email"],
                "user",
                &user,
            )?;
            let r = resolve(g, api, &ta, Mode::Grant).await?;
            let mut body = json!({ "user_id": s(&u, "id"), "target": target_body(&r, &r.chosen) });
            lifetime_body(&mut body, &lifetime)?;

            if let Some(n) = note {
                body["note"] = json!(n);
            }

            let v = api.post(base, &body).await?;

            if !g.output.is_machine() {
                let who = u["username"].as_str().or(u["email"].as_str()).unwrap_or(&user);
                let on = if r.node_name.is_empty() { "every node".to_owned() } else { r.node_name.clone() };

                let ports = if r.chosen.is_empty() {
                    "every port".to_owned()
                } else {
                    format!("port {}", r.chosen.iter().map(ToString::to_string).collect::<Vec<_>>().join(", "))
                };

                eprintln!("{who} may tunnel to {} on {on} ({ports}) · {}", r.label, expiry_text(&v));
            }

            out::result(g.output, &v, None);
            Ok(())
        }
        GrantsCmd::Revoke { id, yes } => {
            crate::cmds::confirm(g, yes, &format!("Revoke tunnel grant {id}? Tunnels opened through it close"))?;
            api.call(reqwest::Method::DELETE, &format!("{base}/{}", crate::admincmd::enc(&id)), None).await?;
            out::result(g.output, &json!({ "revoked": id }), None);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_well() {
        assert_eq!(size(512), "512 B");
        assert_eq!(size(12_300_000), "12.3 MB");
    }

    #[test]
    fn lifetimes_parse() {
        assert_eq!(parse_lifetime("30m").ok(), Some(Some(30)));
        assert_eq!(parse_lifetime("8h").ok(), Some(Some(480)));
        assert_eq!(parse_lifetime("3d").ok(), Some(Some(3 * 24 * 60)));
        assert_eq!(parse_lifetime("1h30m").ok(), Some(Some(90)));
        assert_eq!(parse_lifetime("never").ok(), Some(None));
        assert!(parse_lifetime("8").is_err());
        assert!(parse_lifetime("0h").is_err());
        assert!(parse_lifetime("soon").is_err());
    }

    #[test]
    fn expiry_reads_like_a_person_says_it() {
        assert_eq!(expires_in(8 * 3_600_000), "8 h");
        assert_eq!(expires_in(29 * 60_000), "29 min");
        assert_eq!(expires_in(3 * 24 * 3_600_000), "3 d");
        assert_eq!(expiry_text(&json!({ "expires_at": null })), "never expires");
    }

    #[test]
    fn keys_split_and_match_services() {
        assert_eq!(split_key("app:site@lab-b:8080"), Some(("app", "site", "lab-b", "8080")));
        assert_eq!(split_key("stack:seq-b/seq@lab-c:80"), Some(("stack", "seq-b/seq", "lab-c", "80")));
        assert_eq!(split_key("nonsense"), None);
        let e = json!({ "kind": "stack", "name": "seq", "label": "seq-b/seq" });
        assert!(matches_service(&e, "seq", Some("seq-b")));
        assert!(!matches_service(&e, "seq", Some("other")));
        assert!(matches_service(&json!({ "name": "Site" }), "site", None));
    }

    #[test]
    fn local_side_is_a_url_only_for_http() {
        let a: SocketAddr = "127.0.0.1:18080".parse().unwrap_or(SocketAddr::new(IpAddr::from([127, 0, 0, 1]), 1));
        assert_eq!(local_text(Some("http"), a), "http://127.0.0.1:18080");
        assert_eq!(local_text(Some("ingest"), a), "127.0.0.1:18080");
        assert_eq!(local_text(None, a), "127.0.0.1:18080");
    }

    #[test]
    fn database_hints_follow_the_engine() {
        let r = Resolved {
            kind: "package".into(),
            reference: "orders-pg".into(),
            label: "orders-pg (web)".into(),
            key_target: "orders-pg".into(),
            node_id: Some("n1".into()),
            node_name: "lab-c".into(),
            nodes: vec!["lab-c".into()],
            ports: vec![PortChoice { port: 5432, role: Some("postgres".into()) }],
            chosen: vec![5432],
        };
        let a: SocketAddr = "127.0.0.1:15432".parse().unwrap_or(SocketAddr::new(IpAddr::from([127, 0, 0, 1]), 1));
        assert_eq!(db_hint(&r, 5432, a).as_deref(), Some("psql -h 127.0.0.1 -p 15432 -U postgres"));
        let mut other = r.clone();
        other.reference = "cache".into();
        other.label = "cache".into();
        other.ports = vec![PortChoice { port: 9999, role: Some("custom".into()) }];
        assert_eq!(db_hint(&other, 9999, a), None);
    }

    #[test]
    fn target_body_carries_node_and_ports() {
        let r = Resolved {
            kind: "app".into(),
            reference: "site".into(),
            label: "site (web)".into(),
            key_target: "site".into(),
            node_id: Some("n1".into()),
            node_name: "lab-b".into(),
            nodes: vec!["lab-b".into()],
            ports: vec![PortChoice { port: 8080, role: Some("http".into()) }],
            chosen: vec![8080],
        };
        assert_eq!(
            target_body(&r, &[8080]),
            json!({ "kind": "app", "ref": "site", "server_id": "n1", "ports": [8080] })
        );
        let mut any = r;
        any.node_id = None;
        assert_eq!(target_body(&any, &[]), json!({ "kind": "app", "ref": "site" }));
    }
}
