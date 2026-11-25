//! `forge docker …`: a node's Docker Engine API over mutual TLS (D-125).
//!
//! * `forge docker endpoint show|enable|disable|check <node>` manages the
//!   node's TLS listener;
//! * `forge docker catalog` lists every Engine API call Forge forwards and
//!   the Forge action it needs;
//! * `forge docker api <node> <METHOD> <PATH>` makes one Engine API call
//!   through the control plane and streams the answer to stdout (or ).

use std::time::Duration;

use clap::{Subcommand, ValueEnum};
use futures::StreamExt;
use reqwest::Method;
use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;

use crate::admincmd::{enc, node_id};
use crate::error::{CliError, Exit, Result};
use crate::out::{self, Col};
use crate::{Global, connect, target};

#[derive(Subcommand, Debug)]
pub enum DockerCmd {
    #[command(flatten)]
    Resources(crate::containercmd::DockerResCmd),
    /// The node's Docker API endpoint (dockerd on TCP with mutual TLS).
    #[command(subcommand)]
    Endpoint(EndpointCmd),
    /// Every Docker Engine API call Forge forwards, with the Forge action it needs.
    Catalog {
        /// Only calls of this class.
        #[arg(long, value_enum)]
        class: Option<Class>,
    },
    /// One Docker Engine API call on a node, through the control plane.
    ///
    /// PATH is as in the Engine API reference, optionally with a version
    /// prefix and a query string: `/containers/json?all=1`,
    /// `/v1.47/images/nginx:1.27/json`. The answer streams to stdout, or to
    /// `--out-file`. Attach, exec start and BuildKit sessions upgrade the
    /// connection and are not available here: use `forge nodes terminal` for a
    /// shell in a container.
    Api {
        /// Node id, host name or name.
        node: String,
        /// HTTP method: GET, HEAD, POST, PUT or DELETE.
        method: String,
        /// Engine API path, with an optional query string.
        path: String,
        /// JSON request body (`@file` reads it from a file).
        #[arg(long, value_name = "JSON", conflicts_with = "upload")]
        data: Option<String>,
        /// Raw request body from a file, sent as `application/x-tar`
        /// (archive uploads, image loads, builds).
        #[arg(long, value_name = "FILE")]
        upload: Option<String>,
        /// Write the answer to this file instead of stdout.
        #[arg(long, value_name = "FILE")]
        out_file: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum EndpointCmd {
    /// State, address, versions and certificate expiry.
    Show {
        /// Node id, host name or name.
        node: String,
    },
    /// Turn the endpoint on: dockerd restarts and listens on TCP with mutual TLS.
    Enable {
        /// Node id, host name or name.
        node: String,
        /// Address the control plane dials (default: the address the node's agent connects from).
        #[arg(long)]
        host: Option<String>,
        /// TCP port dockerd listens on.
        #[arg(long, default_value_t = 2376)]
        port: u16,
        /// Address dockerd binds (default: `--host` when it is one of the node's addresses).
        #[arg(long)]
        listen: Option<String>,
        /// Accept that running containers stop while dockerd restarts (live-restore off).
        #[arg(long)]
        acknowledge_restart: bool,
        /// Return once the job is queued instead of waiting for it.
        #[arg(long)]
        no_wait: bool,
    },
    /// Turn the endpoint off and forget the node's Docker CA.
    Disable {
        /// Node id, host name or name.
        node: String,
        /// Forget the endpoint without asking the node (when the node is gone).
        #[arg(long)]
        force: bool,
        /// Return once the job is queued instead of waiting for it.
        #[arg(long)]
        no_wait: bool,
    },
    /// Ask dockerd now and record its version or the failure.
    Check {
        /// Node id, host name or name.
        node: String,
    },
}

/// What a call can do (which Forge action it needs).
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// Lists, inspects, logs, stats, events: `server.manage`.
    Read,
    /// Container, image, network and volume changes: `container.manage`.
    Manage,
    /// Exec, attach, archives, export: `container.exec`.
    Exec,
    /// Root on the node (swarm, plugins, build, privileged containers): `server.command`.
    Admin,
}

impl Class {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Manage => "manage",
            Self::Exec => "exec",
            Self::Admin => "admin",
        }
    }
}

const ENDPOINT_COLS: &[Col] = &[
    ("STATE", "/state"),
    ("HOST", "/host"),
    ("PORT", "/port"),
    ("ENGINE", "/engine_version"),
    ("API", "/api_version"),
    ("ERROR", "/last_error"),
];

const CATALOG_COLS: &[Col] =
    &[("METHOD", "/method"), ("PATH", "/path"), ("CLASS", "/class"), ("ACTION", "/action"), ("OP", "/op")];

fn endpoint_path(id: &str) -> String {
    format!("/api/v1/servers/{}/docker/endpoint", enc(id))
}

pub async fn run(g: &Global, cmd: DockerCmd) -> Result<()> {
    if let DockerCmd::Resources(r) = cmd {
        return crate::containercmd::docker_resources(g, r).await;
    }

    let t = target(g)?;
    let mut api = connect(g, &t).await?;

    match cmd {
        DockerCmd::Resources(_) => Ok(()),
        DockerCmd::Endpoint(e) => endpoint(g, &mut api, e).await,
        DockerCmd::Catalog { class } => {
            let v = api.get("/api/v1/docker/catalog").await?;
            let rows: Vec<Value> = v
                .as_array()
                .into_iter()
                .flatten()
                .filter(|r| class.is_none_or(|c| r["class"] == c.as_str()))
                .cloned()
                .collect();
            out::result(g.output, &Value::Array(rows), Some(CATALOG_COLS));
            Ok(())
        }
        DockerCmd::Api { node, method, path, data, upload, out_file } => {
            let id = node_id(&mut api, &node).await?;
            call(&mut api, &id, &method, &path, Body::from_flags(data, upload)?, out_file.as_deref()).await
        }
    }
}

async fn endpoint(g: &Global, api: &mut crate::api::Api, cmd: EndpointCmd) -> Result<()> {
    match cmd {
        EndpointCmd::Show { node } => {
            let id = node_id(api, &node).await?;
            let v = api.get(&endpoint_path(&id)).await?;
            out::result(g.output, &v, Some(ENDPOINT_COLS));
            Ok(())
        }
        EndpointCmd::Check { node } => {
            let id = node_id(api, &node).await?;
            let v = api.post(&format!("{}/check", endpoint_path(&id)), &json!({})).await?;
            out::result(g.output, &v, Some(ENDPOINT_COLS));
            Ok(())
        }
        EndpointCmd::Enable { node, host, port, listen, acknowledge_restart, no_wait } => {
            let id = node_id(api, &node).await?;
            let body =
                json!({ "host": host, "port": port, "listen": listen, "acknowledge_restart": acknowledge_restart });
            let started = api.post(&endpoint_path(&id), &body).await?;
            finish(g, api, &id, &started, no_wait).await
        }
        EndpointCmd::Disable { node, force, no_wait } => {
            let id = node_id(api, &node).await?;
            let path = if force { format!("{}?force=true", endpoint_path(&id)) } else { endpoint_path(&id) };
            let started = api.call(Method::DELETE, &path, None).await?;
            finish(g, api, &id, &started, no_wait).await
        }
    }
}

/// Waits for the job (unless `no_wait`), then prints the endpoint.
async fn finish(g: &Global, api: &mut crate::api::Api, id: &str, started: &Value, no_wait: bool) -> Result<()> {
    let job = started["job_id"].as_str().unwrap_or_default().to_owned();

    if no_wait {
        out::result(g.output, started, None);
        return Ok(());
    }

    crate::cmds::wait_jobs(api, &[job], Duration::from_secs(10 * 60)).await?;
    let v = api.get(&endpoint_path(id)).await?;
    out::result(g.output, &v, Some(ENDPOINT_COLS));

    Ok(())
}

/// A request body for `forge docker api`.
enum Body {
    None,
    Json(Value),
    Raw(Vec<u8>),
}

impl Body {
    fn from_flags(data: Option<String>, upload: Option<String>) -> Result<Self> {
        if let Some(d) = data {
            let text = match d.strip_prefix('@') {
                Some(file) => std::fs::read_to_string(file).map_err(|e| CliError::usage(format!("{file}: {e}")))?,
                None => d,
            };

            let v = serde_json::from_str(&text).map_err(|e| CliError::usage(format!("--data is not JSON: {e}")))?;

            return Ok(Self::Json(v));
        }

        if let Some(f) = upload {
            return Ok(Self::Raw(std::fs::read(&f).map_err(|e| CliError::usage(format!("{f}: {e}")))?));
        }

        Ok(Self::None)
    }
}

async fn call(
    api: &mut crate::api::Api,
    id: &str,
    method: &str,
    path: &str,
    body: Body,
    output: Option<&str>,
) -> Result<()> {
    let method = Method::from_bytes(method.to_ascii_uppercase().as_bytes())
        .map_err(|_| CliError::usage(format!("unknown method {method}")))?;

    if !matches!(method, Method::GET | Method::HEAD | Method::POST | Method::PUT | Method::DELETE) {
        return Err(CliError::usage("the method must be GET, HEAD, POST, PUT or DELETE"));
    }

    let path = path.strip_prefix('/').unwrap_or(path);
    let url = format!("/api/v1/servers/{}/docker/engine/{path}", enc(id));

    // Engine calls stream (logs, events, pulls): no overall time limit.
    api.set_request_timeout(None);

    let resp = match body {
        Body::None => api.send(method, &url, None).await?,
        Body::Json(v) => api.send(method, &url, Some(&v)).await?,
        Body::Raw(bytes) => api.send_bytes(method, &url, bytes).await?,
    };

    let mut sink: Box<dyn tokio::io::AsyncWrite + Unpin> = match output {
        Some(f) => {
            Box::new(tokio::fs::File::create(f).await.map_err(|e| CliError::new(Exit::General, format!("{f}: {e}")))?)
        }
        None => Box::new(tokio::io::stdout()),
    };

    let mut stream = resp.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| CliError::new(Exit::Transport, format!("the stream ended: {e}")))?;
        sink.write_all(&chunk).await.map_err(|e| CliError::new(Exit::General, e.to_string()))?;
        sink.flush().await.map_err(|e| CliError::new(Exit::General, e.to_string()))?;
    }

    Ok(())
}
