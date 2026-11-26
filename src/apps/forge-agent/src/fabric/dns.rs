//! forge-dns on a fabric node (`plans/native-mesh.md` 2.4, 2.5, 2.7): the
//! unit's environment file, the service itself and interface-scoped
//! resolving for the cluster zone through systemd-resolved. Best effort:
//! a node without the binary or systemd keeps its hosts-file names.

use std::net::Ipv4Addr;
use std::path::Path;

use super::run;

const BINARY: &str = "/usr/local/bin/forge-dns";
const ENV_FILE: &str = "/etc/forge/forge-dns.env";
const UNIT: &str = "forge-dns";

/// The environment file for listening on `mesh_ip`.
#[must_use]
pub fn env_text(mesh_ip: Ipv4Addr) -> String {
    let file = crate::host_names::records_path();

    format!("FORGE_DNS_FILE={}\nFORGE_DNS_ARGS=--listen {mesh_ip}:53\n", file.display())
}

/// The zones of the node's `names.json` (cluster zone and reverse zones).
async fn zones() -> Vec<String> {
    let Ok(text) = tokio::fs::read_to_string(crate::host_names::records_path()).await else { return Vec::new() };

    serde_json::from_str::<forge_catalog::names::Records>(&text).map(|r| r.zones).unwrap_or_default()
}

/// `running`, `stopped` or `missing`, for the node's facts.
pub async fn status() -> &'static str {
    if !Path::new(BINARY).exists() {
        return "missing";
    }

    match run("systemctl", &["is-active", UNIT]).await {
        Ok(s) if s.trim() == "active" => "running",
        _ => "stopped",
    }
}

/// Writes the environment file, (re)starts forge-dns when its listen
/// address changed and `names.json` exists, and points the interface at it.
/// Returns what happened, for the command's answer.
pub async fn configure(iface: &str, mesh_ip: Ipv4Addr) -> String {
    if !Path::new(BINARY).exists() || !Path::new("/run/systemd/system").is_dir() {
        return "missing".into();
    }

    let want = env_text(mesh_ip);
    let have = tokio::fs::read_to_string(ENV_FILE).await.unwrap_or_default();
    let changed = want != have;

    if changed && let Err(e) = write_env(&want).await {
        return format!("error: {e}");
    }

    if !crate::host_names::records_path().exists() {
        return "waiting for names".into();
    }

    let action = if changed { "restart" } else { "start" };

    if let Err(e) = run("systemctl", &["enable", UNIT]).await.and(run("systemctl", &[action, UNIT]).await) {
        return format!("error: {e}");
    }

    resolve_through(iface, mesh_ip).await;

    "running".into()
}

async fn write_env(text: &str) -> Result<(), String> {
    let text = text.to_owned();

    tokio::task::spawn_blocking(move || super::write_private(Path::new(ENV_FILE), &text))
        .await
        .map_err(|e| e.to_string())?
}

/// Interface-scoped DNS for the cluster and reverse zones, when
/// systemd-resolved runs; other resolvers of the node are untouched.
async fn resolve_through(iface: &str, mesh_ip: Ipv4Addr) {
    if !Path::new("/run/systemd/resolve").is_dir() {
        return;
    }

    let domains: Vec<String> = zones().await.into_iter().map(|z| format!("~{z}")).collect();

    if domains.is_empty() {
        return;
    }

    let mut args = vec!["domain".to_owned(), iface.to_owned()];
    args.extend(domains);
    let args: Vec<&str> = args.iter().map(String::as_str).collect();

    let r = run("resolvectl", &["dns", iface, &mesh_ip.to_string()]).await.and(run("resolvectl", &args).await);

    if let Err(e) = r {
        tracing::warn!(error = %e, "pointing the interface at forge-dns failed; the hosts file still serves names");
    }
}

/// Starts forge-dns after the first `names.json` arrives, when the network
/// already configured it.
/// Also points the cluster interface at it, as [`configure`] does, so the
/// node resolves the zone without waiting for the repair pass.
pub async fn on_records_written() {
    if !Path::new(BINARY).exists() {
        return;
    }

    let Ok(env) = tokio::fs::read_to_string(ENV_FILE).await else { return };
    let _ = run("systemctl", &["start", UNIT]).await;

    let Some(mesh_ip) = listen_ip(&env) else { return };

    if let Some(iface) = iface_of(mesh_ip).await {
        resolve_through(&iface, mesh_ip).await;
    }
}

/// The address in the env file's `--listen <ip>:53`.
fn listen_ip(env: &str) -> Option<Ipv4Addr> {
    let rest = env.split("--listen ").nth(1)?;
    let addr = rest.split_whitespace().next()?;

    addr.parse::<std::net::SocketAddrV4>().ok().map(|a| *a.ip())
}

/// The interface holding `ip` (`ip -o -4 addr show`).
async fn iface_of(ip: Ipv4Addr) -> Option<String> {
    let out = run("ip", &["-o", "-4", "addr", "show"]).await.ok()?;
    let needle = format!("inet {ip}/");

    out.lines().find(|l| l.contains(&needle)).and_then(|l| l.split_whitespace().nth(1)).map(str::to_owned)
}

/// Undoes [`configure`] when the fabric is removed: stop and disable the
/// service, revert the interface's DNS, delete the env file and records.
pub async fn remove(iface: &str) {
    if Path::new("/run/systemd/system").is_dir() && Path::new(BINARY).exists() {
        let _ = run("systemctl", &["disable", "--now", UNIT]).await;
    }

    if Path::new("/run/systemd/resolve").is_dir() {
        let _ = run("resolvectl", &["revert", iface]).await;
    }

    for p in [Path::new(ENV_FILE).to_path_buf(), crate::host_names::records_path()] {
        if let Err(e) = tokio::fs::remove_file(&p).await
            && e.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(path = %p.display(), error = %e, "removing a forge-dns file failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_file_listens_on_the_mesh_address() {
        let t = env_text(Ipv4Addr::new(10, 64, 0, 3));

        assert!(t.contains("FORGE_DNS_ARGS=--listen 10.64.0.3:53\n"));
        assert!(t.contains("names.json"));
        assert_eq!(listen_ip(&t), Some(Ipv4Addr::new(10, 64, 0, 3)));
    }
}
