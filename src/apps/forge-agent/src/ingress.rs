//! Cluster ingress: the global Nginx config and keepalived (root, inside
//! `forge-agent-helper`).

use std::path::Path;

use forge_catalog::ingress::{
    GLOBAL_CONF, IngressOp, KEEPALIVED_CONF, NginxSettings, TAKEN_OVER, give_back, take_over,
};

const MAIN_CONF: &str = "/etc/nginx/nginx.conf";
use serde_json::{Value, json};

use crate::sites::{nginx_bin, reload, run};

fn write_or_remove(path: &str, content: Option<&str>) -> Result<(), String> {
    match content {
        Some(c) => {
            if let Some(dir) = Path::new(path).parent() {
                std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            }
            std::fs::write(path, c).map_err(|e| format!("{path}: {e}"))
        }
        None => match std::fs::remove_file(path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(format!("{path}: {e}")),
            _ => Ok(()),
        },
    }
}

async fn service(action: &str, name: &str) -> Result<(), String> {
    if Path::new("/run/systemd/system").is_dir() {
        run("systemctl", &[action, name]).await.map(|_| ())
    } else {
        // Containers / non-systemd hosts: best effort with the daemon itself.
        match action {
            "stop" | "disable" => run("pkill", &["-x", name]).await.map(|_| ()),
            _ => {
                let _ = run("pkill", &["-HUP", "-x", name]).await;
                if run("pgrep", &["-x", name]).await.is_err() { run(name, &[]).await.map(|_| ()) } else { Ok(()) }
            }
        }
    }
}

async fn apply(nginx_cfg: &NginxSettings, keepalived: Option<String>) -> Result<Value, String> {
    let nginx = nginx_bin()?;
    let previous = std::fs::read_to_string(GLOBAL_CONF).ok();
    // Directives the distribution already sets in nginx.conf are taken over
    // (commented with a marker) so ours apply and nginx sees no duplicates.
    let main_before = std::fs::read_to_string(MAIN_CONF).ok();
    if let Some(main) = &main_before {
        let taken = take_over(&give_back(main), &nginx_cfg.directives());
        if &taken != main {
            write_or_remove(MAIN_CONF, Some(&taken))?;
        }
    }
    write_or_remove(GLOBAL_CONF, Some(&nginx_cfg.render()))?;
    if let Err(e) = run(nginx, &["-t"]).await {
        write_or_remove(GLOBAL_CONF, previous.as_deref())?;
        if let Some(m) = &main_before {
            write_or_remove(MAIN_CONF, Some(m))?;
        }
        return Err(format!("nginx -t failed; previous configuration restored:\n{e}"));
    }
    reload(nginx).await?;
    let mut ka = Value::Null;
    if let Some(conf) = keepalived {
        if !Path::new("/usr/sbin/keepalived").exists() {
            return Err("keepalived is not installed on this node (App Store → Keepalived)".into());
        }
        write_or_remove(KEEPALIVED_CONF, Some(&conf))?;
        service("enable", "keepalived").await.ok();
        service("restart", "keepalived").await?;
        ka = json!("configured");
    }
    Ok(json!({ "nginx": "applied", "keepalived": ka }))
}

async fn remove() -> Result<Value, String> {
    write_or_remove(GLOBAL_CONF, None)?;
    if let Ok(main) = std::fs::read_to_string(MAIN_CONF)
        && main.contains(TAKEN_OVER)
    {
        write_or_remove(MAIN_CONF, Some(&give_back(&main)))?;
    }
    if let Ok(nginx) = nginx_bin() {
        reload(nginx).await.ok();
    }
    if Path::new(KEEPALIVED_CONF).exists()
        && std::fs::read_to_string(KEEPALIVED_CONF).is_ok_and(|c| c.contains("Managed by Orbyn Forge"))
    {
        service("stop", "keepalived").await.ok();
        write_or_remove(KEEPALIVED_CONF, None)?;
    }
    Ok(json!({ "removed": true }))
}

async fn status(vips: &[String]) -> Value {
    let nginx_running = run("pgrep", &["-x", "nginx"]).await.is_ok();
    let nginx_installed = nginx_bin().is_ok();
    let ka_running = run("pgrep", &["-x", "keepalived"]).await.is_ok();
    let addrs = run("ip", &["-o", "addr"]).await.unwrap_or_default();
    let held: Vec<&String> = vips
        .iter()
        .filter(|v| {
            let ip = v.split('/').next().unwrap_or(v);
            addrs.split_whitespace().any(|w| w.split('/').next() == Some(ip))
        })
        .collect();
    json!({
        "nginx": if nginx_running { "running" } else if nginx_installed { "stopped" } else { "not-installed" },
        "global_config": Path::new(GLOBAL_CONF).exists(),
        "keepalived": if ka_running { "running" } else if Path::new("/usr/sbin/keepalived").exists() { "stopped" } else { "not-installed" },
        "vips_held": held,
        "traffic": traffic_snapshot().map_or(Value::Null, |t| serde_json::to_value(t).unwrap_or(Value::Null)),
    })
}

/// Throughput per endpoint and upstream over the last minute, from the end
/// of the traffic log (at most 2 MB is read).
/// `None` when this node is not an edge (no traffic log, or idle for 10 minutes).
#[must_use]
pub fn traffic_snapshot() -> Option<std::collections::BTreeMap<String, forge_catalog::ingress::SiteTraffic>> {
    use std::io::{Read, Seek, SeekFrom};
    const WINDOW: f64 = 60.0;
    const MAX: u64 = 2 * 1024 * 1024;
    let mut f = std::fs::File::open(forge_catalog::ingress::TRAFFIC_LOG).ok()?;
    let meta = f.metadata().ok()?;
    let len = meta.len();
    // Skip the read when nothing was logged recently.
    if meta.modified().ok().and_then(|m| m.elapsed().ok()).is_some_and(|age| age.as_secs() > 600) {
        return Some(std::collections::BTreeMap::new());
    }
    f.seek(SeekFrom::Start(len.saturating_sub(MAX))).ok()?;
    let mut buf = Vec::new();
    f.take(MAX).read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    // Skip a partial first line when we started mid-file.
    let text = if len > MAX { text.split_once('\n').map_or("", |(_, r)| r) } else { &text };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
    Some(forge_catalog::ingress::traffic(text, now, WINDOW))
}

pub async fn run_op(op: &IngressOp) -> Result<Value, String> {
    op.validate()?;
    match op {
        IngressOp::Apply { nginx, keepalived } => {
            let ka = keepalived.as_ref().and_then(|k| k.spec.render_keepalived(k.index, &k.auth));
            apply(nginx, ka).await
        }
        IngressOp::Remove => remove().await,
        IngressOp::Status { vips } => Ok(status(vips).await),
    }
}
