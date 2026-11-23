//! `forge nodes files …`: the File Explorer from the command line (issue #2
//! and #5). Every command maps to the same `/api/v1/servers/{id}/files/…`
//! endpoint the console uses, so roots, Cedar permissions, MFA, protected
//! paths and audit apply identically. Paths are relative to a root
//! (`--root`, default: the whole-node root, admins only).

use std::io::{Read as _, Write as _};
use std::path::PathBuf;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use clap::Subcommand;
use reqwest::Method;
use serde_json::{Value, json};

use crate::api::Api;
use crate::cmds::{confirm, resolve_node};
use crate::error::{CliError, Exit, Result};
use crate::out::{self, Col, Format};
use crate::{Global, connect, target};

#[derive(Subcommand, Debug)]
pub enum FilesCmd {
    /// Roots you can browse on the node (whole node, services,
    /// containers); `roots add|remove` approve or drop extra roots.
    #[command(args_conflicts_with_subcommands = true, subcommand_negates_reqs = true)]
    Roots {
        #[arg(required = true)]
        node: Option<String>,
        #[command(subcommand)]
        op: Option<RootsCmd>,
    },
    /// List a directory.
    Ls {
        node: String,
        #[arg(default_value = "")]
        path: String,
        #[arg(long)]
        root: Option<String>,
        /// Include hidden entries.
        #[arg(long, short = 'a')]
        all: bool,
        /// Include hidden entries (same as --all).
        #[arg(long)]
        hidden: bool,
        /// Entries per call.
        #[arg(long, default_value_t = 1000)]
        limit: u32,
        /// Skip this many entries first (the next page starts at offset + limit).
        #[arg(long, default_value_t = 0)]
        offset: u32,
    },
    /// File or directory details (size, mode, owner, version).
    Stat {
        node: String,
        path: String,
        #[arg(long)]
        root: Option<String>,
    },
    /// Print a file's content to stdout (text or bytes).
    Cat {
        node: String,
        path: String,
        #[arg(long)]
        root: Option<String>,
        /// Stop after this many bytes.
        #[arg(long, default_value_t = 10 * 1024 * 1024)]
        max_bytes: u64,
    },
    /// SHA-256 of a file.
    Hash {
        node: String,
        path: String,
        #[arg(long)]
        root: Option<String>,
    },
    /// Download a file (never overwrites a local file unless --force).
    Download {
        node: String,
        remote: String,
        local: PathBuf,
        #[arg(long)]
        root: Option<String>,
        #[arg(long)]
        force: bool,
    },
    /// Upload a file (chunked, SHA-256 verified on the node).
    Upload {
        node: String,
        local: PathBuf,
        remote: String,
        #[arg(long)]
        root: Option<String>,
        /// Replace an existing file.
        #[arg(long)]
        overwrite: bool,
        /// Replace only if the remote file still has this version (from `stat`).
        #[arg(long, conflicts_with = "overwrite")]
        if_version: Option<String>,
    },
    /// Create a directory.
    Mkdir {
        node: String,
        path: String,
        #[arg(long)]
        root: Option<String>,
        /// Create missing parent folders too; an existing folder is not an error.
        #[arg(long, short = 'p')]
        parents: bool,
    },
    /// Move or rename within a root.
    Mv {
        node: String,
        from: String,
        to: String,
        #[arg(long)]
        root: Option<String>,
        #[arg(long)]
        overwrite: bool,
    },
    /// Copy within a root.
    Cp {
        node: String,
        from: String,
        to: String,
        #[arg(long)]
        root: Option<String>,
        #[arg(long)]
        overwrite: bool,
    },
    /// Delete (to the node's trash unless --permanent).
    Rm {
        node: String,
        path: String,
        #[arg(long)]
        root: Option<String>,
        /// Delete permanently instead of moving to trash.
        #[arg(long)]
        permanent: bool,
        #[arg(long)]
        yes: bool,
    },
    /// Write stdin (or --from) to a file, with a version check.
    Save {
        node: String,
        path: String,
        #[arg(long)]
        root: Option<String>,
        /// Local file to save (default: stdin).
        #[arg(long)]
        from: Option<PathBuf>,
        /// The version you read (from `stat`); omit to create a new file.
        #[arg(long)]
        if_version: Option<String>,
    },
    /// Search names and/or content under a path.
    Search {
        node: String,
        #[arg(default_value = "")]
        path: String,
        #[arg(long)]
        root: Option<String>,
        /// File name pattern (glob).
        #[arg(long)]
        name: Option<String>,
        /// Content pattern.
        #[arg(long)]
        content: Option<String>,
        #[arg(long)]
        regex: bool,
        #[arg(long)]
        case_sensitive: bool,
    },
    /// The node's file trash.
    #[command(subcommand)]
    Trash(TrashCmd),
    /// Saved revisions of a file.
    Revisions {
        node: String,
        path: String,
        #[arg(long)]
        root: Option<String>,
    },
    /// Create an empty file (fails if it exists).
    Touch {
        node: String,
        path: String,
        #[arg(long)]
        root: Option<String>,
    },
    /// Rename an entry in place (the new name stays in the same folder).
    Rename {
        node: String,
        path: String,
        new_name: String,
        #[arg(long)]
        root: Option<String>,
        #[arg(long)]
        overwrite: bool,
    },
    /// Set permission bits (Linux; administrators, MFA). MODE is octal, e.g. 0644.
    Chmod {
        node: String,
        mode: String,
        path: String,
        #[arg(long)]
        root: Option<String>,
        /// Also everything inside folders.
        #[arg(long, short = 'R')]
        recursive: bool,
        /// Only count what would change.
        #[arg(long)]
        dry_run: bool,
    },
    /// Set owner and/or group (Linux; administrators, MFA). SPEC is OWNER, OWNER:GROUP or :GROUP.
    Chown {
        node: String,
        spec: String,
        path: String,
        #[arg(long)]
        root: Option<String>,
        #[arg(long, short = 'R')]
        recursive: bool,
        #[arg(long)]
        dry_run: bool,
    },
    /// Show the access control list (Windows) or mode (Linux), read-only.
    Acl {
        node: String,
        path: String,
        #[arg(long)]
        root: Option<String>,
    },
    /// Pack files and folders into a new .zip or .tar.gz on the node.
    Compress {
        node: String,
        /// The archive to create; the format follows the name.
        dest: String,
        #[arg(required = true)]
        sources: Vec<String>,
        #[arg(long)]
        root: Option<String>,
    },
    /// Unpack a .zip, .tar, .tar.gz or .tgz into a new folder on the node.
    Extract {
        node: String,
        archive: String,
        dest: String,
        #[arg(long)]
        root: Option<String>,
        /// Replace an existing destination folder atomically (unpack to a
        /// temporary folder, then swap; needs `file.delete`).
        #[arg(long)]
        replace: bool,
    },
    /// Copy, move, trash or delete many entries in one call (one result each).
    Bulk {
        node: String,
        #[arg(value_enum)]
        action: BulkAction,
        #[arg(required = true)]
        paths: Vec<String>,
        /// Destination folder for copy and move.
        #[arg(long)]
        to: Option<String>,
        #[arg(long)]
        root: Option<String>,
        #[arg(long)]
        overwrite: bool,
        #[arg(long)]
        yes: bool,
    },
    /// Download several entries as one archive (never overwrites unless --force).
    Archive {
        node: String,
        local: PathBuf,
        #[arg(required = true)]
        paths: Vec<String>,
        #[arg(long)]
        root: Option<String>,
        /// zip (default) or tar_gz.
        #[arg(long, default_value = "zip")]
        format: String,
        #[arg(long)]
        force: bool,
    },
    /// Temporary SFTP access for a node.
    #[command(subcommand)]
    Sftp(SftpCmd),
}

#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum BulkAction {
    Copy,
    Move,
    Trash,
    Delete,
}

impl BulkAction {
    fn as_str(self) -> &'static str {
        match self {
            Self::Copy => "copy",
            Self::Move => "move",
            Self::Trash => "trash",
            Self::Delete => "delete",
        }
    }
}

#[derive(Subcommand, Debug)]
pub enum SftpCmd {
    /// Gateway host, port and host key fingerprint.
    Info { node: String },
    /// Access grants: on one node, or across the fleet without a node.
    List {
        /// Node name or id; omit to list every node you may see.
        node: Option<String>,
        /// Only `active`, `expired` or `revoked` grants.
        #[arg(long)]
        status: Option<String>,
        /// Match the name, id, creator or node name.
        #[arg(long)]
        search: Option<String>,
    },
    /// One grant (no secrets).
    Get { node: String, grant: String },
    /// Change a grant; the password stays the same. Needs a recent MFA check.
    Update {
        node: String,
        grant: String,
        #[arg(long)]
        name: Option<String>,
        /// Replace the roots (repeat).
        #[arg(long = "root")]
        roots: Vec<String>,
        /// Allow create, upload and rename (`true` or `false`).
        #[arg(long)]
        write: Option<bool>,
        /// Allow delete and replace (`true` or `false`).
        #[arg(long)]
        delete: Option<bool>,
        /// New lifetime in minutes, counted from now.
        #[arg(long, conflicts_with = "never")]
        minutes: Option<u32>,
        /// Never expire (needs `[sftp] allow_never_expiry` and `file.admin`).
        #[arg(long)]
        never: bool,
        /// Replace the allowed client addresses (IP or CIDR, repeat).
        #[arg(long = "allow-ip", conflicts_with = "any_ip")]
        allowed_ips: Vec<String>,
        /// Allow any client address.
        #[arg(long)]
        any_ip: bool,
        #[arg(long)]
        max_sessions: Option<u32>,
        /// Per-session bandwidth cap in KiB/s.
        #[arg(long, conflicts_with = "no_bandwidth_limit")]
        bandwidth_kbps: Option<u32>,
        #[arg(long)]
        no_bandwidth_limit: bool,
        /// Total bytes the grant may transfer.
        #[arg(long, conflicts_with = "no_transfer_limit")]
        transfer_limit: Option<u64>,
        #[arg(long)]
        no_transfer_limit: bool,
    },
    /// Replace a grant's password; prints the new one once. Open sessions close.
    ResetPassword {
        node: String,
        grant: String,
        #[arg(long)]
        yes: bool,
    },
    /// Issue temporary access; prints the password once.
    Grant {
        node: String,
        /// A label for the person or tool.
        #[arg(long)]
        name: String,
        /// Roots to expose (repeat); default: the whole node.
        #[arg(long = "root")]
        roots: Vec<String>,
        #[arg(long)]
        write: bool,
        #[arg(long)]
        delete: bool,
        /// Lifetime in minutes.
        #[arg(long, default_value_t = 60)]
        minutes: u32,
        /// Never expire (needs `[sftp] allow_never_expiry` and `file.admin`).
        #[arg(long)]
        never: bool,
        /// Allowed client addresses (IP or CIDR, repeat).
        #[arg(long = "allow-ip")]
        allowed_ips: Vec<String>,
        #[arg(long, default_value_t = 1)]
        max_sessions: u32,
    },
    /// Revoke a grant; open sessions close.
    Revoke {
        node: String,
        grant: String,
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum RootsCmd {
    /// Approve a folder as a root (`file.admin`): a service folder on
    /// the node, or a path in a container.
    Add {
        node: String,
        /// Node path (service) or the path inside the container.
        path: String,
        /// Name shown in the root list.
        #[arg(long)]
        label: String,
        /// `service` (a node folder) or `container`.
        #[arg(long, value_parser = ["service", "container"], default_value = "service")]
        kind: String,
        /// Full container id (container roots).
        #[arg(long)]
        container: Option<String>,
        /// Allow changes in this root (default: read-only).
        #[arg(long)]
        writable: bool,
        /// The service this folder belongs to (id).
        #[arg(long)]
        service: Option<String>,
    },
    /// Remove an approved root (by id, `fr_…`); files are not touched.
    Remove {
        node: String,
        root: String,
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum TrashCmd {
    /// Trashed entries.
    List {
        node: String,
        #[arg(long)]
        root: Option<String>,
    },
    /// Restore one entry by id.
    Restore {
        node: String,
        id: String,
        #[arg(long)]
        root: Option<String>,
        #[arg(long)]
        overwrite: bool,
    },
    /// Purge one entry, or the whole trash without --id.
    Purge {
        node: String,
        #[arg(long)]
        id: Option<String>,
        #[arg(long)]
        root: Option<String>,
        #[arg(long)]
        yes: bool,
    },
}

const ENTRY_COLS: &[Col] = &[
    ("KIND", "/kind"),
    ("MODE", "/mode_str"),
    ("OWNER", "/owner"),
    ("GROUP", "/group"),
    ("SIZE", "/size"),
    ("MODIFIED", "/modified"),
    ("NAME", "/name"),
];

/// Prints SFTP connection details with the one-time password.
fn print_connection(output: Format, v: &Value) {
    if output == Format::Table {
        let c = &v["connection"];
        println!("host        {}", c["host"].as_str().unwrap_or_default());
        println!("port        {}", c["port"]);
        println!("username    {}", c["username"].as_str().unwrap_or_default());
        println!("password    {}", c["password"].as_str().unwrap_or_default());
        println!("fingerprint {}", c["fingerprint"].as_str().unwrap_or_default());
        eprintln!("(the password is shown only once)");
    } else {
        out::result(output, v, None);
    }
}

fn enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

/// `?root=…&path=…` (+ extra pairs).
fn query(root: Option<&str>, path: &str, extra: &[(&str, String)]) -> String {
    let mut q = vec![format!("path={}", enc(path))];
    if let Some(r) = root {
        q.push(format!("root={}", enc(r)));
    }
    for (k, v) in extra {
        q.push(format!("{k}={}", enc(v)));
    }
    format!("?{}", q.join("&"))
}

fn mode_str(mode: Option<u64>) -> String {
    let Some(m) = mode else { return "-".into() };
    let bits = ['r', 'w', 'x'];
    (0..9).map(|i| if m & (1 << (8 - i)) != 0 { bits[i % 3] } else { '-' }).collect()
}

fn ts(ms: Option<i64>) -> String {
    ms.and_then(|ms| time::OffsetDateTime::from_unix_timestamp(ms / 1000).ok())
        .map(|t| format!("{:04}-{:02}-{:02} {:02}:{:02}", t.year(), u8::from(t.month()), t.day(), t.hour(), t.minute()))
        .unwrap_or_else(|| "-".into())
}

/// Adds human columns to directory entries.
fn decorate(entries: &mut Value) {
    for e in entries.as_array_mut().into_iter().flatten() {
        let m = mode_str(e["mode"].as_u64());
        let t = ts(e["modified_ms"].as_i64());
        e["mode_str"] = json!(m);
        e["modified"] = json!(t);
    }
}

/// Parses an octal mode such as `644`, `0644` or `0o755`.
fn parse_mode(s: &str) -> Result<String> {
    let t = s.trim_start_matches("0o");
    let t = if t.len() > 1 { t.trim_start_matches('0') } else { t };
    let t = if t.is_empty() { "0" } else { t };
    match u32::from_str_radix(t, 8) {
        Ok(m) if m <= 0o7777 => Ok(format!("{m:04o}")),
        _ => Err(CliError::usage(format!("`{s}` is not an octal mode such as 0644"))),
    }
}

/// `OWNER`, `OWNER:GROUP` or `:GROUP` → (owner, group).
fn parse_owner(spec: &str) -> Result<(Option<String>, Option<String>)> {
    let (o, g) = match spec.split_once(':') {
        Some((o, g)) => (o, Some(g)),
        None => (spec, None),
    };
    let some = |s: &str| (!s.is_empty()).then(|| s.to_owned());
    let (o, g) = (some(o), g.and_then(some));
    if o.is_none() && g.is_none() {
        return Err(CliError::usage("give OWNER, OWNER:GROUP or :GROUP"));
    }
    Ok((o, g))
}

/// The sibling path `new_name` of `path` (`a/b.txt` + `c.txt` → `a/c.txt`).
fn sibling(path: &str, new_name: &str) -> Result<String> {
    if new_name.is_empty() || new_name.contains(['/', '\\']) || new_name == "." || new_name == ".." {
        return Err(CliError::usage("the new name must be a plain name, not a path"));
    }
    let trimmed = path.trim_end_matches(['/', '\\']);
    Ok(match trimmed.rfind(['/', '\\']) {
        Some(i) => format!("{}{new_name}", &trimmed[..=i]),
        None => new_name.to_owned(),
    })
}

/// Streams a GET response into `local` through a `.forge-part` file.
async fn save_to(resp: &mut reqwest::Response, local: &std::path::Path) -> Result<u64> {
    let tmp = local.with_extension("forge-part");
    let mut f = std::fs::File::create(&tmp)?;
    let mut n = 0u64;
    let res: Result<()> = async {
        while let Some(chunk) = resp.chunk().await.map_err(|e| CliError::new(Exit::Transport, e.to_string()))? {
            f.write_all(&chunk)?;
            n += chunk.len() as u64;
        }
        f.sync_all()?;
        Ok(())
    }
    .await;
    drop(f);
    if let Err(e) = res {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    std::fs::rename(&tmp, local)?;
    Ok(n)
}

fn refuse_existing(local: &std::path::Path, force: bool) -> Result<()> {
    if local.exists() && !force {
        return Err(CliError::new(Exit::Conflict, format!("{} exists; use --force to replace it", local.display())));
    }
    Ok(())
}

/// A root as a table row: `PATH` from its spec, `ACCESS` read-only or read-write.
fn root_row(r: &Value) -> Value {
    let path = r["path"]
        .as_str()
        .or_else(|| r["base"].as_str())
        .or_else(|| r["spec"]["base"].as_str())
        .map_or_else(|| if r["kind"] == "node" { "/".to_owned() } else { "-".to_owned() }, str::to_owned);
    json!({
        "id": r["id"],
        "kind": r["kind"],
        "label": r["label"],
        "path": path,
        "access": if r["read_only"] == true { "read-only" } else { "read-write" },
    })
}

/// The parent folder of `path` (`/a/b/c` → `/a/b`).
fn parent_of(path: &str) -> String {
    let t = path.trim_end_matches('/');
    match t.rfind('/') {
        Some(0) | None => "/".to_owned(),
        Some(i) => t[..i].to_owned(),
    }
}

/// A "not found" from mkdir or upload names one path segment (or none);
/// say which folder is missing, with the full path, and how to fix it.
fn name_missing_folder(e: CliError, path: &str, node: &str, hint_parents: bool) -> CliError {
    let missing = e.exit == Exit::NotFound
        || e.message.contains("was not found")
        || e.message.contains("does not exist")
        || e.message.contains("no such file");
    if !missing {
        return e;
    }
    let parent = parent_of(path);
    let hint = if hint_parents {
        "; use --parents to create it"
    } else {
        "; create it first with `forge nodes files mkdir --parents`"
    };
    let message = format!("parent folder {parent} of {path} does not exist on {node}{hint} (server: {})", e.message);
    CliError { message, ..e }
}

async fn node_id(api: &mut Api, node: &str) -> Result<String> {
    Ok(resolve_node(api, node).await?["id"].as_str().unwrap_or_default().to_owned())
}

pub async fn run(g: &Global, cmd: FilesCmd) -> Result<()> {
    let t = target(g)?;
    let mut api = connect(g, &t).await?;
    let base = |id: &str| format!("/api/v1/servers/{id}/files");
    match cmd {
        FilesCmd::Roots { node: Some(node), op: None } => {
            let id = node_id(&mut api, &node).await?;
            let v = api.get(&format!("{}/roots", base(&id))).await?;
            if g.output.is_machine() {
                out::result(g.output, &v, None);
            } else {
                let rows: Vec<Value> = v["roots"].as_array().into_iter().flatten().map(root_row).collect();
                out::result(
                    g.output,
                    &Value::Array(rows),
                    Some(&[
                        ("ID", "/id"),
                        ("KIND", "/kind"),
                        ("LABEL", "/label"),
                        ("PATH", "/path"),
                        ("ACCESS", "/access"),
                    ]),
                );
            }
        }
        FilesCmd::Roots {
            op: Some(RootsCmd::Add { node, path, label, kind, container, writable, service }), ..
        } => {
            let id = node_id(&mut api, &node).await?;
            let v = api
                .post(
                    &format!("{}/roots", base(&id)),
                    &json!({
                        "kind": kind,
                        "label": label,
                        "path": path,
                        "container": container,
                        "read_only": !writable,
                        "service_id": service,
                    }),
                )
                .await?;
            out::result(g.output, &v, None);
        }
        FilesCmd::Roots { op: Some(RootsCmd::Remove { node, root, yes }), .. } => {
            let id = node_id(&mut api, &node).await?;
            confirm(g, yes, &format!("Remove root {root} from {node}? Files stay; access through this root ends."))?;
            let v = api.call(Method::DELETE, &format!("{}/roots/{}", base(&id), enc(&root)), None).await?;
            out::result(g.output, &v, None);
        }
        FilesCmd::Roots { node: None, op: None } => {
            return Err(CliError::usage("name a node: `forge nodes files roots <node>`"));
        }
        FilesCmd::Ls { node, path, root, all, hidden, limit, offset } => {
            let id = node_id(&mut api, &node).await?;
            let q = query(
                root.as_deref(),
                &path,
                &[
                    ("limit", limit.to_string()),
                    ("offset", offset.to_string()),
                    ("hidden", (all || hidden).to_string()),
                ],
            );
            let mut v = api.get(&format!("{}/list{q}", base(&id))).await?;
            if g.output == Format::Table {
                let mut entries = v["entries"].take();
                decorate(&mut entries);
                out::result(g.output, &entries, Some(ENTRY_COLS));
                if let Some(total) = v["total"].as_u64().filter(|n| *n > u64::from(offset) + u64::from(limit)) {
                    eprintln!(
                        "showing {}–{} of {total} · next: --offset {}",
                        offset + 1,
                        offset + limit,
                        offset + limit
                    );
                }
            } else {
                out::result(g.output, &v, None);
            }
        }
        FilesCmd::Stat { node, path, root } => {
            let id = node_id(&mut api, &node).await?;
            let v = api.get(&format!("{}/stat{}", base(&id), query(root.as_deref(), &path, &[]))).await?;
            out::result(g.output, &v, None);
        }
        FilesCmd::Hash { node, path, root } => {
            let id = node_id(&mut api, &node).await?;
            let v = api.get(&format!("{}/hash{}", base(&id), query(root.as_deref(), &path, &[]))).await?;
            out::result(g.output, &v, None);
        }
        FilesCmd::Cat { node, path, root, max_bytes } => {
            let id = node_id(&mut api, &node).await?;
            let mut offset = 0u64;
            let mut version: Option<String> = None;
            let mut stdout = std::io::stdout().lock();
            loop {
                let mut extra = vec![("offset", offset.to_string())];
                if let Some(v) = &version {
                    // Pin the version: a file changing mid-read fails instead of mixing.
                    extra.push(("version", v.clone()));
                }
                let v = api.get(&format!("{}/read{}", base(&id), query(root.as_deref(), &path, &extra))).await?;
                let data = STANDARD
                    .decode(v["content_b64"].as_str().unwrap_or_default())
                    .map_err(|_| CliError::new(Exit::General, "bad content from the server"))?;
                let room = usize::try_from(max_bytes.saturating_sub(offset)).unwrap_or(usize::MAX);
                stdout.write_all(&data[..data.len().min(room)])?;
                offset += data.len() as u64;
                version = v["version"].as_str().map(str::to_owned);
                if v["eof"] == true || data.is_empty() {
                    break;
                }
                if offset >= max_bytes {
                    eprintln!("forge: stopped at {max_bytes} bytes (use --max-bytes or `files download`)");
                    break;
                }
            }
        }
        FilesCmd::Download { node, remote, local, root, force } => {
            let id = node_id(&mut api, &node).await?;
            refuse_existing(&local, force)?;
            let mut resp = api
                .send(Method::GET, &format!("{}/download{}", base(&id), query(root.as_deref(), &remote, &[])), None)
                .await?;
            // Write to a temp file next to the target, then rename: no half files.
            let n = save_to(&mut resp, &local).await?;
            out::result(g.output, &json!({ "file": local, "bytes": n }), None);
        }
        FilesCmd::Upload { node, local, remote, root, overwrite, if_version } => {
            let id = node_id(&mut api, &node).await?;
            let size =
                std::fs::metadata(&local).map_err(|e| CliError::usage(format!("{}: {e}", local.display())))?.len();
            let ow = match (overwrite, if_version) {
                (_, Some(v)) => json!({ "version": v }),
                (true, None) => json!("always"),
                (false, None) => json!("never"),
            };
            let up = api
                .post(
                    &format!("{}/uploads", base(&id)),
                    &json!({ "root": root, "path": remote, "size": size, "overwrite": ow }),
                )
                .await
                .map_err(|e| {
                    if e.exit == Exit::Conflict && !overwrite {
                        CliError {
                            message: format!("{} ({remote} exists: pass --overwrite to replace it)", e.message),
                            ..e
                        }
                    } else {
                        name_missing_folder(e, &remote, &node, false)
                    }
                })?;
            let up_id = up["id"].as_str().ok_or_else(|| CliError::new(Exit::General, "upload not started"))?.to_owned();
            let chunk = usize::try_from(up["chunk_size"].as_u64().unwrap_or(256 * 1024)).unwrap_or(256 * 1024);
            let mut file = std::fs::File::open(&local)?;
            let mut ctx = ring::digest::Context::new(&ring::digest::SHA256);
            let mut buf = vec![0u8; chunk];
            let mut offset = 0u64;
            let result: Result<()> = async {
                loop {
                    let n = file.read(&mut buf)?;
                    if n == 0 {
                        break;
                    }
                    ctx.update(&buf[..n]);
                    api.send_bytes(
                        Method::PUT,
                        &format!("{}/uploads/{up_id}?offset={offset}", base(&id)),
                        buf[..n].to_vec(),
                    )
                    .await?;
                    offset += n as u64;
                    if size > 0 && g.output == Format::Table {
                        eprint!("\r{:>3}%  {offset}/{size} bytes", offset * 100 / size);
                    }
                }
                Ok(())
            }
            .await;
            if g.output == Format::Table {
                eprintln!();
            }
            if let Err(e) = result {
                let _ = api.send(Method::DELETE, &format!("{}/uploads/{up_id}", base(&id)), None).await;
                return Err(e);
            }
            let sha = hex::encode(ctx.finish());
            let v = api.post(&format!("{}/uploads/{up_id}/commit", base(&id)), &json!({ "sha256": sha })).await?;
            out::result(g.output, &v, None);
        }
        FilesCmd::Mkdir { node, path, root, parents } => {
            let id = node_id(&mut api, &node).await?;
            let v = api
                .post(&format!("{}/mkdir", base(&id)), &json!({ "root": root, "path": path, "parents": parents }))
                .await
                .map_err(|e| name_missing_folder(e, &path, &node, !parents))?;
            out::result(g.output, &v, None);
        }
        FilesCmd::Mv { node, from, to, root, overwrite } => {
            let id = node_id(&mut api, &node).await?;
            let v = api
                .post(
                    &format!("{}/move", base(&id)),
                    &json!({ "root": root, "from": from, "to": to, "overwrite": overwrite }),
                )
                .await?;
            out::result(g.output, &v, None);
        }
        FilesCmd::Cp { node, from, to, root, overwrite } => {
            let id = node_id(&mut api, &node).await?;
            let v = api
                .post(
                    &format!("{}/copy", base(&id)),
                    &json!({ "root": root, "from": from, "to": to, "overwrite": overwrite }),
                )
                .await?;
            out::result(g.output, &v, None);
        }
        FilesCmd::Rm { node, path, root, permanent, yes } => {
            let id = node_id(&mut api, &node).await?;
            let what = if permanent { "Permanently delete" } else { "Move to trash" };
            confirm(g, yes, &format!("{what} {path} on {node}?"))?;
            let v = api
                .post(&format!("{}/delete", base(&id)), &json!({ "root": root, "path": path, "permanent": permanent }))
                .await?;
            out::result(g.output, &v, None);
        }
        FilesCmd::Save { node, path, root, from, if_version } => {
            let id = node_id(&mut api, &node).await?;
            let content = match from {
                Some(p) => std::fs::read(&p).map_err(|e| CliError::usage(format!("{}: {e}", p.display())))?,
                None => {
                    let mut b = Vec::new();
                    std::io::stdin().read_to_end(&mut b)?;
                    b
                }
            };
            let v = api
                .post(
                    &format!("{}/save", base(&id)),
                    &json!({ "root": root, "path": path, "content_b64": STANDARD.encode(&content), "version": if_version }),
                )
                .await?;
            out::result(g.output, &v, None);
        }
        FilesCmd::Search { node, path, root, name, content, regex, case_sensitive } => {
            let id = node_id(&mut api, &node).await?;
            if name.is_none() && content.is_none() {
                return Err(CliError::usage("give --name and/or --content"));
            }
            let spec = json!({
                "name": name.unwrap_or_default(),
                "content": content.unwrap_or_default(),
                "regex": regex,
                "case_sensitive": case_sensitive,
            });
            let v = api
                .post(&format!("{}/search", base(&id)), &json!({ "root": root, "path": path, "spec": spec }))
                .await?;
            out::result(g.output, &v, None);
        }
        FilesCmd::Revisions { node, path, root } => {
            let id = node_id(&mut api, &node).await?;
            let v = api.get(&format!("{}/revisions{}", base(&id), query(root.as_deref(), &path, &[]))).await?;
            out::result(g.output, &v, None);
        }
        FilesCmd::Trash(TrashCmd::List { node, root }) => {
            let id = node_id(&mut api, &node).await?;
            let q = root.map(|r| format!("?root={}", enc(&r))).unwrap_or_default();
            let v = api.get(&format!("{}/trash{q}", base(&id))).await?;
            let entries = v.get("entries").cloned().unwrap_or(v);
            out::result(
                g.output,
                &entries,
                Some(&[("ID", "/id"), ("PATH", "/path"), ("KIND", "/kind"), ("SIZE", "/size")]),
            );
        }
        FilesCmd::Trash(TrashCmd::Restore { node, id: entry, root, overwrite }) => {
            let id = node_id(&mut api, &node).await?;
            let v = api
                .post(
                    &format!("{}/trash/restore", base(&id)),
                    &json!({ "root": root, "id": entry, "overwrite": overwrite }),
                )
                .await?;
            out::result(g.output, &v, None);
        }
        FilesCmd::Trash(TrashCmd::Purge { node, id: entry, root, yes }) => {
            let id = node_id(&mut api, &node).await?;
            let what = entry.as_deref().map_or_else(|| "the whole trash".to_owned(), |e| format!("trash entry {e}"));
            confirm(g, yes, &format!("Permanently purge {what} on {node}?"))?;
            let v = api.post(&format!("{}/trash/purge", base(&id)), &json!({ "root": root, "id": entry })).await?;
            out::result(g.output, &v, None);
        }
        FilesCmd::Touch { node, path, root } => {
            let id = node_id(&mut api, &node).await?;
            // No version: the node creates the file and refuses an existing one.
            let v = api
                .post(&format!("{}/save", base(&id)), &json!({ "root": root, "path": path, "content_b64": "" }))
                .await?;
            out::result(g.output, &v, None);
        }
        FilesCmd::Rename { node, path, new_name, root, overwrite } => {
            let id = node_id(&mut api, &node).await?;
            let to = sibling(&path, &new_name)?;
            let v = api
                .post(
                    &format!("{}/move", base(&id)),
                    &json!({ "root": root, "from": path, "to": to, "overwrite": overwrite }),
                )
                .await?;
            out::result(g.output, &v, None);
        }
        FilesCmd::Chmod { node, mode, path, root, recursive, dry_run } => {
            let mode = parse_mode(&mode)?;
            let id = node_id(&mut api, &node).await?;
            let v = api
                .post(
                    &format!("{}/chmod", base(&id)),
                    &json!({ "root": root, "path": path, "mode": mode, "recursive": recursive, "dry_run": dry_run }),
                )
                .await?;
            out::result(g.output, &v, None);
        }
        FilesCmd::Chown { node, spec, path, root, recursive, dry_run } => {
            let (owner, group) = parse_owner(&spec)?;
            let id = node_id(&mut api, &node).await?;
            let v = api
                .post(
                    &format!("{}/chown", base(&id)),
                    &json!({
                        "root": root, "path": path, "owner": owner, "group": group,
                        "recursive": recursive, "dry_run": dry_run,
                    }),
                )
                .await?;
            out::result(g.output, &v, None);
        }
        FilesCmd::Acl { node, path, root } => {
            let id = node_id(&mut api, &node).await?;
            let v = api.get(&format!("{}/acl{}", base(&id), query(root.as_deref(), &path, &[]))).await?;
            match (g.output, v["acl"].as_str()) {
                (Format::Table, Some(acl)) => println!("{acl}"),
                _ => out::result(g.output, &v, None),
            }
        }
        FilesCmd::Compress { node, dest, sources, root } => {
            let id = node_id(&mut api, &node).await?;
            let v = api
                .post(&format!("{}/compress", base(&id)), &json!({ "root": root, "sources": sources, "dest": dest }))
                .await?;
            out::result(g.output, &v, None);
        }
        FilesCmd::Extract { node, archive, dest, root, replace } => {
            let id = node_id(&mut api, &node).await?;
            let v = api
                .post(
                    &format!("{}/extract", base(&id)),
                    &json!({ "root": root, "archive": archive, "dest": dest, "replace": replace }),
                )
                .await
                .map_err(|e| {
                    if e.exit == Exit::Conflict && !replace {
                        CliError { message: format!("{} (pass --replace to swap the folder {dest})", e.message), ..e }
                    } else {
                        e
                    }
                })?;
            out::result(g.output, &v, None);
        }
        FilesCmd::Bulk { node, action, paths, to, root, overwrite, yes } => {
            if matches!(action, BulkAction::Copy | BulkAction::Move) != to.is_some() {
                return Err(CliError::usage("copy and move need --to; trash and delete take no --to"));
            }
            let id = node_id(&mut api, &node).await?;
            if matches!(action, BulkAction::Trash | BulkAction::Delete) {
                let what = if action == BulkAction::Delete { "Permanently delete" } else { "Move to trash" };
                confirm(g, yes, &format!("{what} {} entries on {node}?", paths.len()))?;
            }
            let v = api
                .post(
                    &format!("{}/bulk", base(&id)),
                    &json!({ "root": root, "action": action.as_str(), "paths": paths, "to": to, "overwrite": overwrite }),
                )
                .await?;
            if g.output == Format::Table {
                out::result(g.output, &v["results"], Some(&[("OK", "/ok"), ("PATH", "/path"), ("ERROR", "/error")]));
            } else {
                out::result(g.output, &v, None);
            }
            let failed = v["failed"].as_u64().unwrap_or(0);
            if failed > 0 {
                return Err(CliError::new(Exit::General, format!("{failed} of {} entries failed", paths.len())));
            }
        }
        FilesCmd::Archive { node, local, paths, root, format, force } => {
            if format != "zip" && format != "tar_gz" {
                return Err(CliError::usage("--format is zip or tar_gz"));
            }
            refuse_existing(&local, force)?;
            let id = node_id(&mut api, &node).await?;
            let made = api
                .post(&format!("{}/archive", base(&id)), &json!({ "root": root, "paths": paths, "format": format }))
                .await?;
            let temp = made["temp_id"].as_str().ok_or_else(|| CliError::new(Exit::General, "no archive was made"))?;
            let q = root.map(|r| format!("?root={}", enc(&r))).unwrap_or_default();
            let mut resp = api.send(Method::GET, &format!("{}/archive/{}{q}", base(&id), enc(temp)), None).await?;
            let n = save_to(&mut resp, &local).await?;
            out::result(g.output, &json!({ "file": local, "bytes": n, "entries": made["entries"] }), None);
        }
        FilesCmd::Sftp(SftpCmd::Info { node }) => {
            let id = node_id(&mut api, &node).await?;
            let v = api.get(&format!("{}/sftp", base(&id))).await?;
            out::result(g.output, &v, None);
        }
        FilesCmd::Sftp(SftpCmd::List { node, status, search }) => {
            let mut q = Vec::new();
            if let Some(s) = &status {
                q.push(format!("status={}", enc(s)));
            }
            if let Some(s) = &search {
                q.push(format!("q={}", enc(s)));
            }
            if let Some(node) = node {
                let id = node_id(&mut api, &node).await?;
                q.insert(0, format!("server={}", enc(&id)));
            }
            let q = if q.is_empty() { String::new() } else { format!("?{}", q.join("&")) };
            let v = api.get(&format!("/api/v1/files/grants{q}")).await?;
            out::result(
                g.output,
                &v["grants"],
                Some(&[
                    ("ID", "/id"),
                    ("NAME", "/name"),
                    ("NODE", "/server_name"),
                    ("STATUS", "/status"),
                    ("WRITE", "/write"),
                    ("DELETE", "/delete"),
                    ("NEVER", "/never_expires"),
                    ("ROOTS", "/roots"),
                ]),
            );
        }
        FilesCmd::Sftp(SftpCmd::Get { node, grant }) => {
            let id = node_id(&mut api, &node).await?;
            let v = api.get(&format!("{}/grants/{}", base(&id), enc(&grant))).await?;
            out::result(g.output, &v, None);
        }
        FilesCmd::Sftp(SftpCmd::Update {
            node,
            grant,
            name,
            roots,
            write,
            delete,
            minutes,
            never,
            allowed_ips,
            any_ip,
            max_sessions,
            bandwidth_kbps,
            no_bandwidth_limit,
            transfer_limit,
            no_transfer_limit,
        }) => {
            let mut body = serde_json::Map::new();
            if let Some(v) = name {
                body.insert("name".into(), json!(v));
            }
            if !roots.is_empty() {
                body.insert("roots".into(), json!(roots));
            }
            if let Some(v) = write {
                body.insert("write".into(), json!(v));
            }
            if let Some(v) = delete {
                body.insert("delete".into(), json!(v));
            }
            if never {
                body.insert("never_expires".into(), json!(true));
            } else if let Some(m) = minutes {
                body.insert("expires_in_minutes".into(), json!(m));
                body.insert("never_expires".into(), json!(false));
            }
            if any_ip {
                body.insert("allowed_ips".into(), json!([]));
            } else if !allowed_ips.is_empty() {
                body.insert("allowed_ips".into(), json!(allowed_ips));
            }
            if let Some(v) = max_sessions {
                body.insert("max_sessions".into(), json!(v));
            }
            if no_bandwidth_limit {
                body.insert("bandwidth_kbps".into(), Value::Null);
            } else if let Some(v) = bandwidth_kbps {
                body.insert("bandwidth_kbps".into(), json!(v));
            }
            if no_transfer_limit {
                body.insert("transfer_limit".into(), Value::Null);
            } else if let Some(v) = transfer_limit {
                body.insert("transfer_limit".into(), json!(v));
            }
            if body.is_empty() {
                return Err(CliError::usage("nothing to change; pass at least one option"));
            }
            let id = node_id(&mut api, &node).await?;
            let v = api
                .call(Method::PATCH, &format!("{}/grants/{}", base(&id), enc(&grant)), Some(&Value::Object(body)))
                .await?;
            out::result(g.output, &v, None);
        }
        FilesCmd::Sftp(SftpCmd::ResetPassword { node, grant, yes }) => {
            let id = node_id(&mut api, &node).await?;
            confirm(g, yes, &format!("Reset the password of SFTP access {grant} on {node}? Open sessions close."))?;
            let v = api.post(&format!("{}/grants/{}/reset-password", base(&id), enc(&grant)), &json!({})).await?;
            print_connection(g.output, &v);
        }
        FilesCmd::Sftp(SftpCmd::Grant {
            node,
            name,
            roots,
            write,
            delete,
            minutes,
            never,
            allowed_ips,
            max_sessions,
        }) => {
            let id = node_id(&mut api, &node).await?;
            let roots = if roots.is_empty() { vec!["node".to_owned()] } else { roots };
            let v = api
                .post(
                    &format!("{}/grants", base(&id)),
                    &json!({
                        "name": name, "roots": roots, "write": write, "delete": delete,
                        "expires_in_minutes": minutes, "never_expires": never, "allowed_ips": allowed_ips,
                        "max_sessions": max_sessions,
                    }),
                )
                .await?;
            print_connection(g.output, &v);
        }
        FilesCmd::Sftp(SftpCmd::Revoke { node, grant, yes }) => {
            let id = node_id(&mut api, &node).await?;
            confirm(g, yes, &format!("Revoke SFTP access {grant} on {node}? Open sessions close."))?;
            let v = api.post(&format!("{}/grants/{}/revoke", base(&id), enc(&grant)), &json!({})).await?;
            out::result(g.output, &v, None);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_and_queries_render() {
        assert_eq!(mode_str(Some(0o754)), "rwxr-xr--");
        assert_eq!(mode_str(None), "-");
        assert_eq!(
            query(Some("app:web"), "a b/c.txt", &[("offset", "5".into())]),
            "?path=a+b%2Fc.txt&root=app%3Aweb&offset=5"
        );
        assert_eq!(ts(Some(0)), "1970-01-01 00:00");
    }

    #[test]
    fn modes_parse_as_octal() {
        assert_eq!(parse_mode("644").ok().as_deref(), Some("0644"));
        assert_eq!(parse_mode("0755").ok().as_deref(), Some("0755"));
        assert_eq!(parse_mode("0o4750").ok().as_deref(), Some("4750"));
        assert_eq!(parse_mode("0").ok().as_deref(), Some("0000"));
        assert!(parse_mode("0899").is_err());
        assert!(parse_mode("17777").is_err());
        assert!(parse_mode("rwx").is_err());
    }

    #[test]
    fn owner_specs_split() {
        let p = |s| parse_owner(s).ok();
        assert_eq!(p("www-data"), Some((Some("www-data".into()), None)));
        assert_eq!(p("root:adm"), Some((Some("root".into()), Some("adm".into()))));
        assert_eq!(p(":adm"), Some((None, Some("adm".into()))));
        assert_eq!(p("root:"), Some((Some("root".into()), None)));
        assert!(parse_owner(":").is_err());
        assert!(parse_owner("").is_err());
    }

    #[test]
    fn renames_stay_in_the_folder() {
        assert_eq!(sibling("etc/app/a.conf", "b.conf").ok().as_deref(), Some("etc/app/b.conf"));
        assert_eq!(sibling("a.conf", "b.conf").ok().as_deref(), Some("b.conf"));
        assert_eq!(sibling("/srv/dir/", "new").ok().as_deref(), Some("/srv/new"));
        assert!(sibling("a", "x/y").is_err());
        assert!(sibling("a", "..").is_err());
        assert!(sibling("a", "").is_err());
    }

    #[test]
    fn bulk_actions_match_the_api() {
        let names: Vec<_> =
            [BulkAction::Copy, BulkAction::Move, BulkAction::Trash, BulkAction::Delete].map(BulkAction::as_str).into();
        assert_eq!(names, ["copy", "move", "trash", "delete"]);
    }

    #[test]
    fn the_command_line_parses() {
        use clap::Parser;
        #[derive(Parser)]
        struct T {
            #[command(subcommand)]
            c: FilesCmd,
        }
        let ok = |args: &[&str]| T::try_parse_from(std::iter::once("t").chain(args.iter().copied())).is_ok();
        assert!(ok(&["chmod", "web1", "0644", "/srv/a", "-R", "--dry-run"]));
        assert!(ok(&["chown", "web1", "www-data:www-data", "/srv/a"]));
        assert!(ok(&["compress", "web1", "/tmp/a.zip", "/srv/a", "/srv/b"]));
        assert!(!ok(&["compress", "web1", "/tmp/a.zip"]));
        assert!(ok(&["bulk", "web1", "move", "/a", "/b", "--to", "/c"]));
        assert!(!ok(&["bulk", "web1", "shred", "/a"]));
        assert!(ok(&["archive", "web1", "out.zip", "/a", "/b"]));
        assert!(ok(&["sftp", "grant", "web1", "--name", "vendor", "--root", "app:web", "--write", "--minutes", "30"]));
        assert!(ok(&["sftp", "revoke", "web1", "g1", "--yes"]));
        assert!(ok(&["sftp", "list"]));
        assert!(ok(&["sftp", "list", "--status", "active", "--search", "vendor"]));
        assert!(ok(&["sftp", "list", "web1"]));
        assert!(ok(&["sftp", "get", "web1", "g1"]));
        assert!(ok(&["sftp", "update", "web1", "g1", "--write", "false", "--minutes", "120", "--any-ip"]));
        assert!(!ok(&["sftp", "update", "web1", "g1", "--minutes", "5", "--never"]));
        assert!(ok(&["sftp", "update", "web1", "g1", "--never", "--no-transfer-limit"]));
        assert!(ok(&["sftp", "reset-password", "web1", "g1", "--yes"]));
        assert!(ok(&["sftp", "grant", "web1", "--name", "x", "--never"]));
        assert!(ok(&["rename", "web1", "/srv/a.txt", "b.txt"]));
        assert!(ok(&["touch", "web1", "/srv/new.txt"]));
    }
}
