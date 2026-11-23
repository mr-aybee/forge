//! fleet-sim: benchmark the control plane with N simulated agents.
//!
//! For each fleet size it starts a real `forge-server` process on SQLite,
//! enrolls N agents (in-process, each with its own key, certificate and
//! HTTP/2 mTLS connection), then measures:
//!
//! * control-plane idle RSS and CPU while all agents long-poll,
//! * API p50/p99 latency of `GET /api/v1/servers`,
//! * reconnect storm: kill + restart the server, time until all agents are back,
//! * (once) a real `forge-agent` process: idle RSS and CPU.
//!
//! Results are printed as Markdown and optionally written as JSON.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use clap::Parser;
use forge_agent::enroll::{HostFacts, enroll};
use forge_agent::{Agent, AgentConfig, StatePaths};
use futures::{StreamExt, stream};
use serde::Serialize;
use serde_json::{Value, json};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

mod soak;

const EMAIL: &str = "bench@example.com";
const PASSWORD: &str = "benchmark-password-123";

#[derive(Parser, Debug)]
#[command(about = "Simulate a fleet of Orbyn Forge agents")]
struct Args {
    /// Fleet sizes to run, e.g. `--agents 50 --agents 1000`.
    #[arg(long, default_values_t = [50u32, 1000])]
    agents: Vec<u32>,
    /// Seconds to sample the idle fleet.
    #[arg(long, default_value_t = 60)]
    idle_secs: u64,
    /// Long-poll hold the server uses (production default 25).
    #[arg(long, default_value_t = 25)]
    hold_secs: u32,
    /// Path of forge-server (default: next to this binary).
    #[arg(long)]
    server_bin: Option<PathBuf>,
    /// Path of forge-agent (default: next to this binary). Use `--no-agent-process` to skip.
    #[arg(long)]
    agent_bin: Option<PathBuf>,
    #[arg(long)]
    no_agent_process: bool,
    /// Turn off the fleet-wide local-names sync (`[names] enabled = false`) to measure its cost.
    #[arg(long)]
    no_names: bool,
    /// Write results as JSON here.
    #[arg(long)]
    out: Option<PathBuf>,
    /// Soak mode: API base URL of a server that is already running. The
    /// fleet (one `--agents` value) runs against it for `--duration-secs`.
    #[arg(long)]
    target: Option<String>,
    /// Soak mode: the server's agent endpoint.
    #[arg(long, default_value = "https://127.0.0.1:7443")]
    target_agent_url: String,
    /// Soak mode: user to sign in as (MFA off).
    #[arg(long, default_value = "admin")]
    login: String,
    /// Soak mode: file holding that user's password.
    #[arg(long)]
    password_file: Option<PathBuf>,
    /// Soak mode: where node identities are kept; existing ones are reused.
    #[arg(long, default_value = "fleet-sim-nodes")]
    state_dir: PathBuf,
    /// Soak mode: how long the fleet runs.
    #[arg(long, default_value_t = 300)]
    duration_secs: u64,
    /// Soak mode: seconds between report lines.
    #[arg(long, default_value_t = 30)]
    report_secs: u64,
    /// Soak mode: container log lines each node writes per second.
    #[arg(long, default_value_t = 0.0)]
    log_lines_per_sec: f64,
    /// Soak mode: containers per node (per-minute samples, log sources).
    #[arg(long, default_value_t = 0)]
    containers: u32,
    /// Soak mode: how many of `--containers` write the log lines (the rest
    /// are silent but still send per-minute samples); 0 means all.
    #[arg(long, default_value_t = 0)]
    logging_containers: u32,
    /// Soak mode: the analytics worker's agent listener the nodes upload
    /// their spool files to (default: the URL the server names).
    #[arg(long)]
    worker_url: Option<String>,
    /// Soak mode: the server's process id; reports then show its RSS and
    /// open file descriptors (Linux).
    #[arg(long)]
    server_pid: Option<u32>,
    /// Soak mode: fail when the server's RSS goes above this many MB in any
    /// report. Needs `--server-pid`.
    #[arg(long)]
    max_server_rss_mb: Option<f64>,
    /// Soak mode: seconds to wait after the load stops before the last
    /// server sample and spool count.
    #[arg(long, default_value_t = 30)]
    settle_secs: u64,
}

#[derive(Debug, Default, Serialize)]
struct Sample {
    rss_mb_avg: f64,
    rss_mb_max: f64,
    cpu_pct_avg: f64,
}

#[derive(Debug, Serialize)]
struct RunResult {
    agents: u32,
    enroll_secs: f64,
    all_online_secs: f64,
    server_idle: Sample,
    api_p50_ms: f64,
    api_p99_ms: f64,
    reconnect_storm_secs: f64,
    db_size_mb: f64,
    agent_process: Option<Sample>,
}

/// Simulated agents run in this process on the benchmark host, so they
/// must never change it: they allow only read-only commands and refuse the
/// rest locally (the server still builds, signs and delivers every command,
/// and gets a result). Before this, each simulated agent rewrote the host's
/// real hosts file when the names sync reached it.
fn sim_config(url: String) -> AgentConfig {
    let mut cfg = AgentConfig::new(url);
    cfg.allowed_commands = vec!["ping".into(), "collect_facts".into()];
    cfg
}

fn sibling(name: &str) -> PathBuf {
    let exe = if cfg!(windows) { format!("{name}.exe") } else { name.to_owned() };
    std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join(&exe))).unwrap_or_else(|| exe.into())
}

fn free_port() -> anyhow::Result<u16> {
    Ok(std::net::TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
}

struct ServerProc {
    bin: PathBuf,
    config: PathBuf,
    base: String,
    agent_url: String,
    child: Option<Child>,
}

impl ServerProc {
    fn new(bin: PathBuf, dir: &Path, hold: u32, names: bool) -> anyhow::Result<Self> {
        let (api, agent) = (free_port()?, free_port()?);
        let data = dir.join("data").display().to_string().replace('\\', "/");
        let toml = format!(
            "data_dir = '{data}'\napi_listen = '127.0.0.1:{api}'\nagent_listen = '127.0.0.1:{agent}'\n\
             public_url = 'http://127.0.0.1:{api}'\ncookie_secure = false\nheartbeat_hold_secs = {hold}\n\
             [names]\nenabled = {names}\n"
        );
        let config = dir.join("forge.toml");
        std::fs::write(&config, toml)?;
        Ok(Self {
            bin,
            config,
            base: format!("http://127.0.0.1:{api}"),
            agent_url: format!("https://127.0.0.1:{agent}"),
            child: None,
        })
    }

    fn cmd(&self) -> Command {
        let mut c = Command::new(&self.bin);
        c.arg("--config").arg(&self.config).env("RUST_LOG", "warn").stdout(Stdio::null()).stderr(Stdio::null());
        c
    }

    fn init(&self) -> anyhow::Result<()> {
        let ok = self.cmd().args(["init", "--email", EMAIL]).env("FORGE_ADMIN_PASSWORD", PASSWORD).status()?.success();
        if !ok {
            bail!("forge-server init failed");
        }
        Ok(())
    }

    async fn start(&mut self) -> anyhow::Result<()> {
        self.child = Some(self.cmd().arg("serve").spawn().context("starting forge-server")?);
        for _ in 0..200 {
            if reqwest::get(format!("{}/api/health", self.base)).await.is_ok_and(|r| r.status().is_success()) {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        bail!("server did not become healthy")
    }

    fn pid(&self) -> Option<Pid> {
        self.child.as_ref().map(|c| Pid::from_u32(c.id()))
    }

    fn kill(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

impl Drop for ServerProc {
    fn drop(&mut self) {
        self.kill();
    }
}

struct Api {
    http: reqwest::Client,
    base: String,
    csrf: String,
}

impl Api {
    async fn login(base: &str, login: &str, password: &str) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder().cookie_store(true).pool_max_idle_per_host(0).build()?;
        // Login challenge (D-064): solve it when the server asks for one.
        let mut body = json!({"login": login, "password": password});
        let c: Value =
            http.get(format!("{base}/api/v1/auth/challenge")).query(&[("login", login)]).send().await?.json().await?;
        if c["required"] == true {
            let salt = forge_secrets::pow::salt_from_hex(c["salt"].as_str().unwrap_or_default())
                .ok_or_else(|| anyhow::anyhow!("bad challenge salt"))?;
            let bits = u8::try_from(c["difficulty"].as_u64().unwrap_or(0)).unwrap_or(forge_secrets::pow::MAX_BITS);
            body["challenge_id"] = c["id"].clone();
            body["nonce"] = json!(forge_secrets::pow::solve(&salt, bits.min(forge_secrets::pow::MAX_BITS)));
        }
        let r: Value =
            http.post(format!("{base}/api/v1/auth/login")).json(&body).send().await?.error_for_status()?.json().await?;
        Ok(Self { http, base: base.to_owned(), csrf: r["csrf_token"].as_str().unwrap_or_default().to_owned() })
    }

    async fn get(&self, path: &str) -> anyhow::Result<Value> {
        Ok(self.http.get(format!("{}{path}", self.base)).send().await?.error_for_status()?.json().await?)
    }

    async fn post(&self, path: &str, body: Value) -> anyhow::Result<Value> {
        Ok(self
            .http
            .post(format!("{}{path}", self.base))
            .header("x-csrf-token", &self.csrf)
            .json(&body)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    async fn online(&self) -> usize {
        self.get("/api/v1/servers")
            .await
            .ok()
            .and_then(|v| v.as_array().cloned())
            .map_or(0, |a| a.iter().filter(|s| s["status"] == "online").count())
    }
}

async fn wait_online(api: &Api, n: usize, timeout: Duration) -> anyhow::Result<f64> {
    let start = Instant::now();
    while api.online().await < n {
        if start.elapsed() > timeout {
            bail!("only {} of {n} agents online after {timeout:?}", api.online().await);
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    Ok(start.elapsed().as_secs_f64())
}

/// Samples RSS (MB) and CPU (% of one core) of `pids` once per second.
async fn sample(pids: &[Pid], secs: u64) -> Vec<Sample> {
    let mut sys = System::new();
    let kind = ProcessRefreshKind::nothing().with_memory().with_cpu();
    sys.refresh_processes_specifics(ProcessesToUpdate::Some(pids), true, kind);
    let mut acc: Vec<(Vec<f64>, Vec<f64>)> = vec![(vec![], vec![]); pids.len()];
    for _ in 0..secs.max(2) {
        tokio::time::sleep(Duration::from_secs(1)).await;
        sys.refresh_processes_specifics(ProcessesToUpdate::Some(pids), true, kind);
        for (i, pid) in pids.iter().enumerate() {
            if let Some(p) = sys.process(*pid) {
                #[allow(clippy::cast_precision_loss)]
                acc[i].0.push(p.memory() as f64 / 1_048_576.0);
                acc[i].1.push(f64::from(p.cpu_usage()));
            }
        }
    }
    acc.into_iter()
        .map(|(rss, cpu)| Sample {
            rss_mb_avg: avg(&rss),
            rss_mb_max: rss.iter().copied().fold(0.0, f64::max),
            cpu_pct_avg: avg(&cpu),
        })
        .collect()
}

#[allow(clippy::cast_precision_loss)]
fn avg(v: &[f64]) -> f64 {
    if v.is_empty() { 0.0 } else { v.iter().sum::<f64>() / v.len() as f64 }
}

#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

async fn run(args: &Args, n: u32, with_agent_process: bool) -> anyhow::Result<RunResult> {
    let dir = tempfile::tempdir()?;
    let mut server = ServerProc::new(
        args.server_bin.clone().unwrap_or_else(|| sibling("forge-server")),
        dir.path(),
        args.hold_secs,
        !args.no_names,
    )?;
    server.init()?;
    server.start().await?;
    let api = Api::login(&server.base, EMAIL, PASSWORD).await?;

    // Enroll N agents concurrently.
    let t0 = Instant::now();
    let agents_dir = dir.path().join("agents");
    let enrolled: Vec<anyhow::Result<StatePaths>> = stream::iter(0..n)
        .map(|i| {
            let (api, url, base) = (&api, server.agent_url.clone(), agents_dir.join(i.to_string()));
            async move {
                let tok = api.post("/api/v1/enrollment-tokens", json!({})).await?;
                let paths = StatePaths::new(base);
                let facts = HostFacts { hostname: format!("sim-{i:05}"), os: "linux".into(), arch: "x86_64".into() };
                enroll(&paths, sim_config(url), tok["token"].as_str().context("token")?, facts).await?;
                Ok(paths)
            }
        })
        .buffer_unordered(64)
        .collect()
        .await;
    let enroll_secs = t0.elapsed().as_secs_f64();

    let (stop, rx) = tokio::sync::watch::channel(false);
    for p in enrolled {
        let agent = Agent::load(p?)?;
        tokio::spawn(agent.run(rx.clone()));
    }
    let all_online_secs = wait_online(&api, n as usize, Duration::from_secs(300)).await?;

    // Optional real agent process.
    let mut agent_child = None;
    if with_agent_process {
        let bin = args.agent_bin.clone().unwrap_or_else(|| sibling("forge-agent"));
        let tok = api.post("/api/v1/enrollment-tokens", json!({})).await?;
        let state = dir.path().join("real-agent");
        let enroll = Command::new(&bin)
            .args(["--state-dir"])
            .arg(&state)
            .args(["enroll", "--server", &server.agent_url])
            .env("FORGE_ENROLL_TOKEN", tok["token"].as_str().unwrap_or_default())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()?;
        if enroll.status.success() {
            agent_child = Some(
                Command::new(&bin)
                    .arg("--state-dir")
                    .arg(&state)
                    .arg("run")
                    .env("RUST_LOG", "warn")
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()?,
            );
            wait_online(&api, n as usize + 1, Duration::from_secs(60)).await?;
        } else {
            eprintln!(
                "warning: real agent enrollment failed ({}); skipping agent process sample\n{}{}",
                enroll.status,
                String::from_utf8_lossy(&enroll.stderr).trim(),
                if cfg!(windows) {
                    "\nhint: on Windows, `forge-agent enroll` locks %ProgramData%\\OrbynForge and needs \
                     Administrator; run fleet-sim elevated to sample the real agent"
                } else {
                    ""
                }
            );
        }
    }

    // Idle sampling.
    let mut pids = vec![server.pid().context("server pid")?];
    if let Some(c) = &agent_child {
        pids.push(Pid::from_u32(c.id()));
    }
    let mut samples = sample(&pids, args.idle_secs).await.into_iter();
    let server_idle = samples.next().unwrap_or_default();
    let agent_process = samples.next();

    // API latency.
    let mut lat = Vec::with_capacity(200);
    for _ in 0..200 {
        let t = Instant::now();
        api.get("/api/v1/servers").await?;
        lat.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    lat.sort_by(f64::total_cmp);

    let db = dir.path().join("data").join("forge.db");
    #[allow(clippy::cast_precision_loss)]
    let db_size_mb = std::fs::metadata(&db).map_or(0.0, |m| m.len() as f64 / 1_048_576.0);

    // Reconnect storm: the whole fleet loses the control plane at once.
    server.kill();
    tokio::time::sleep(Duration::from_secs(2)).await;
    let restarted_ms = i64::try_from(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_millis())?;
    server.start().await?;
    let t1 = Instant::now();
    let api = Api::login(&server.base, EMAIL, PASSWORD).await?;
    let expected = n as usize + usize::from(agent_child.is_some());
    // The database still says "online" right after a restart, so count only
    // agents that have actually reconnected (heartbeat after the restart).
    loop {
        let back =
            api.get("/api/v1/servers").await?.as_array().map_or(0, |a| {
                a.iter().filter(|s| s["last_seen_at"].as_i64().is_some_and(|t| t >= restarted_ms)).count()
            });
        if back >= expected {
            break;
        }
        if t1.elapsed() > Duration::from_secs(300) {
            bail!("only {back} of {expected} agents reconnected");
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let reconnect_storm_secs = t1.elapsed().as_secs_f64();

    let _ = stop.send(true);
    if let Some(mut c) = agent_child {
        let _ = c.kill();
        let _ = c.wait();
    }
    server.kill();

    Ok(RunResult {
        agents: n,
        enroll_secs,
        all_online_secs,
        server_idle,
        api_p50_ms: percentile(&lat, 0.50),
        api_p99_ms: percentile(&lat, 0.99),
        reconnect_storm_secs,
        db_size_mb,
        agent_process,
    })
}

fn markdown(results: &[RunResult]) -> String {
    let mut s = String::from(
        "| Agents | Enroll (s) | All online (s) | CP RSS avg / max (MB) | CP CPU avg (% core) | API p50 / p99 (ms) | Reconnect storm (s) | DB (MB) |\n|---:|---:|---:|---:|---:|---:|---:|---:|\n",
    );
    for r in results {
        s.push_str(&format!(
            "| {} | {:.1} | {:.1} | {:.1} / {:.1} | {:.2} | {:.1} / {:.1} | {:.1} | {:.2} |\n",
            r.agents,
            r.enroll_secs,
            r.all_online_secs,
            r.server_idle.rss_mb_avg,
            r.server_idle.rss_mb_max,
            r.server_idle.cpu_pct_avg,
            r.api_p50_ms,
            r.api_p99_ms,
            r.reconnect_storm_secs,
            r.db_size_mb
        ));
    }
    if let Some(a) = results.iter().find_map(|r| r.agent_process.as_ref()) {
        s.push_str(&format!(
            "\nReal `forge-agent` process, idle: RSS avg {:.1} MB (max {:.1} MB), CPU avg {:.2} % of one core.\n",
            a.rss_mb_avg, a.rss_mb_max, a.cpu_pct_avg
        ));
    }
    s
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter("error").init();
    let args = Args::parse();
    if let Some(target) = &args.target {
        let agents = match args.agents.as_slice() {
            [n] => *n,
            _ => bail!("soak mode (--target) takes exactly one --agents value"),
        };
        let file = args.password_file.as_ref().context("soak mode needs --password-file")?;
        let password = std::fs::read_to_string(file).context("reading --password-file")?.trim().to_owned();
        let r = soak::run(soak::SoakArgs {
            agents,
            api: target.trim_end_matches('/').to_owned(),
            agent_url: args.target_agent_url.clone(),
            login: args.login.clone(),
            password,
            state_dir: args.state_dir.clone(),
            duration: Duration::from_secs(args.duration_secs),
            report_every: Duration::from_secs(args.report_secs.max(1)),
            hold: Duration::from_secs(u64::from(args.hold_secs)),
            log_lines_per_sec: args.log_lines_per_sec.max(0.0),
            containers: args.containers,
            logging_containers: args.logging_containers,
            worker_url: args.worker_url.clone().filter(|u| !u.is_empty()),
            server_pid: args.server_pid,
            settle: Duration::from_secs(args.settle_secs),
        })
        .await?;
        let json = serde_json::to_string_pretty(&r)?;
        println!("{json}");
        if let Some(out) = &args.out {
            std::fs::write(out, json)?;
        }
        return soak::check_rss(&r, args.max_server_rss_mb);
    }
    let mut results = Vec::new();
    for (i, n) in args.agents.iter().enumerate() {
        eprintln!("== {n} agents ==");
        let r = run(&args, *n, i == 0 && !args.no_agent_process).await?;
        eprintln!("{}", serde_json::to_string(&r)?);
        results.push(r);
    }
    println!("{}", markdown(&results));
    if let Some(out) = &args.out {
        std::fs::write(out, serde_json::to_string_pretty(&results)?)?;
    }
    Ok(())
}

/// Fails when a command or flag of this binary is missing from its reference
/// page (`docs/src/content/docs/reference/cli/fleet-sim.md`): every subcommand has a
/// `## fleet-sim <path>` (or `### …`) heading and every flag and argument appears.
#[cfg(test)]
mod docs_coverage {
    use clap::CommandFactory;

    fn page() -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../docs/src/content/docs/reference/cli/fleet-sim.md");
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{} is missing: {e}", path.display()))
    }

    fn walk(cmd: &clap::Command, path: &str, doc: &str, missing: &mut Vec<String>) {
        for a in cmd.get_arguments() {
            let id = a.get_id().as_str();
            if matches!(id, "help" | "version") {
                continue;
            }
            let shown = match a.get_long() {
                Some(l) => format!("--{l}"),
                None => format!("<{}>", id.to_uppercase()),
            };
            let alt = format!("[{}]", id.to_uppercase());
            if !doc.contains(&shown) && !doc.contains(&alt) {
                missing.push(format!("{path}: {shown}"));
            }
        }
        for sub in cmd.get_subcommands().filter(|s| s.get_name() != "help") {
            let p = format!("{path} {}", sub.get_name());
            if !doc.contains(&format!("## {p}\n")) && !doc.contains(&format!("### {p}\n")) {
                missing.push(format!("heading `## {p}`"));
            }
            walk(sub, &p, doc, missing);
        }
    }

    #[test]
    fn every_command_and_flag_is_documented() {
        let doc = page();
        let mut missing = Vec::new();
        walk(&super::Args::command(), "fleet-sim", &doc, &mut missing);
        assert!(missing.is_empty(), "undocumented in reference/cli/fleet-sim.md: {missing:?}");
    }
}
