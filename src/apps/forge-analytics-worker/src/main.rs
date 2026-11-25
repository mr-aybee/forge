//! `forge-analytics-worker --config <file>`: started by forge-server, never
//! by hand (D-141).
//!
//! - The first line on stdin is the control secret. forge-server keeps
//!   stdin open; when it closes (forge-server stopped or died), the worker
//!   flushes and exits, so it never outlives its supervisor.
//! - With an agent listener configured, the second line is its TLS
//!   identity ([`AgentTls`] as JSON); the key is never written to disk.
//! - Once it accepts connections it prints `ready <endpoint>` on stdout.
//! - Logs go to stderr, which forge-server passes through to its own.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use forge_analytics_proto::config::{AgentTls, PASSWORD_ENV, READY_PREFIX, WorkerConfig};
use forge_analytics_worker::server::{Listener, serve};
use forge_analytics_worker::wiring::Worker;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::watch;

/// Longest wait for the secret on stdin.
const SECRET_WAIT: Duration = Duration::from_secs(10);
/// Longest wait for the loops to flush and stop on shutdown.
const STOP_WAIT: Duration = Duration::from_secs(20);

fn config_path() -> anyhow::Result<PathBuf> {
    let mut args = std::env::args_os().skip(1);

    while let Some(a) = args.next() {
        if a == "--version" {
            println!("forge-analytics-worker {}", env!("CARGO_PKG_VERSION"));
            std::process::exit(0);
        }

        if a == "--config" {
            return args.next().map(PathBuf::from).context("--config needs a path");
        }
    }

    anyhow::bail!("usage: forge-analytics-worker --config <file> (started by forge-server)")
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();
    let path = config_path()?;
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;

    runtime.block_on(run(path))
}

async fn run(path: PathBuf) -> anyhow::Result<()> {
    let raw = tokio::fs::read_to_string(&path).await.with_context(|| format!("reading {}", path.display()))?;
    let config: WorkerConfig = serde_json::from_str(&raw).context("parsing the worker configuration")?;
    let mut stdin = BufReader::new(tokio::io::stdin());
    let mut secret = String::new();
    tokio::time::timeout(SECRET_WAIT, stdin.read_line(&mut secret)).await.context("no control secret on stdin")??;
    let secret = secret.trim().to_owned();
    anyhow::ensure!(secret.len() >= 32, "the control secret on stdin is too short");
    let password = std::env::var(PASSWORD_ENV).ok();

    // The agent listener's identity follows on the next line (D-142).
    let agent_tls = match &config.agent_listener {
        Some(_) => {
            let mut line = String::new();
            tokio::time::timeout(SECRET_WAIT, stdin.read_line(&mut line))
                .await
                .context("no agent listener identity on stdin")??;
            Some(serde_json::from_str::<AgentTls>(line.trim()).context("parsing the agent listener identity")?)
        }
        None => None,
    };

    let worker = Arc::new(Worker::build(&config, password.as_deref(), Arc::new(forge_core::SystemClock)).await?);
    let (stop, stopped) = watch::channel(false);
    let loops = worker.start(&stopped);

    if let (Some(l), Some(id)) = (&config.agent_listener, &agent_tls) {
        let tls = worker.listener_tls(&l.ca_pem, id).context("agent listener TLS")?;
        let uploads = worker
            .uploads(&l.state_dir, usize::try_from(l.max_uploads).unwrap_or(4))
            .context("opening the spool id log")?;
        let tcp = tokio::net::TcpListener::bind(l.listen)
            .await
            .with_context(|| format!("binding the agent listener on {}", l.listen))?;
        tokio::spawn(forge_analytics_worker::ingest::upload::serve(tcp, tls, uploads, stopped.clone()));
    }

    drop(agent_tls);
    let listener = Listener::bind(&config.endpoint).await.context("binding the control socket")?;
    let mut out = tokio::io::stdout();
    out.write_all(format!("{READY_PREFIX}{}\n", listener.describe()?).as_bytes()).await?;
    out.flush().await?;
    tracing::info!(pid = std::process::id(), "analytics worker ready");

    // Stdin stays open while forge-server runs; EOF (or an error) means it is gone.
    let stop_on_eof = stop.clone();
    tokio::spawn(async move {
        let mut line = String::new();

        loop {
            line.clear();

            match stdin.read_line(&mut line).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }

        stop_on_eof.send_replace(true);
    });
    let stop_on_signal = stop.clone();
    tokio::spawn(async move {
        terminated().await;
        stop_on_signal.send_replace(true);
    });

    serve(&listener, worker, Arc::new(secret), stopped).await;
    tracing::info!("analytics worker stopping");

    for t in loops {
        if tokio::time::timeout(STOP_WAIT, t).await.is_err() {
            tracing::warn!("an analytics loop did not stop in time");
        }
    }

    listener.cleanup().await;
    drop(stop);

    Ok(())
}

/// SIGTERM or Ctrl-C.
async fn terminated() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = term.recv() => {}
                    _ = tokio::signal::ctrl_c() => {}
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
