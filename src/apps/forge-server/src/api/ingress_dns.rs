//! DNS for cluster ingress: Forge keeps an A/AAAA record per endpoint domain
//! on the profile's PowerDNS node, pointing at the edges that should take
//! traffic, and moves it when an edge fails.
//!
//! * On **Apply**, records are published for every endpoint of the cluster.
//!   The zone of each domain is the longest matching zone on the DNS node
//!   (users manage zones, nameservers and other records on the DNS page).
//! * A background loop checks each edge every 15 seconds: the agent must be
//!   online and Nginx on the edge's public address must answer an HTTP request.
//!   Three results in a row change an edge's health (no flapping). When the
//!   set of addresses changes, the records are rewritten (active-active:
//!   healthy edges round-robin; active-standby: the primary, else the first
//!   healthy backup) and the change is audited.
//!
//! DNS commands run as `agent.command` jobs under the principal who last
//! applied the profile, re-authorized by the job runner on every step.

use std::collections::BTreeMap;
use std::time::Duration;

use forge_catalog::ingress::{ClusterProfile, EdgeHealth, IngressMode};
use forge_core::model::{AgentStatus, AuditEvent, AuditOutcome};
use forge_core::ports::{ClusterProfileRepository, ClusterRepository, ServerRepository, SiteRepository};
use forge_core::{ClusterId, EntityKind, EntityRef, ServerId};
use serde_json::{Value, json};
use tokio::sync::watch;

use super::fabric_reconcile::AppliedBy;
use super::sites::run_as;
use crate::state::AppState;

const TICK: Duration = Duration::from_secs(15);
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);
const WAIT: Duration = Duration::from_secs(30);

/// Health per edge, from the profile status.
pub fn health_of(status: &Value) -> BTreeMap<String, EdgeHealth> {
    serde_json::from_value(status.get("health").cloned().unwrap_or(Value::Null)).unwrap_or_default()
}

/// The zone `domain` belongs to: the longest zone that is the domain or a
/// parent of it.
#[must_use]
pub fn zone_for<'a>(domain: &str, zones: &'a [String]) -> Option<&'a String> {
    let fq = format!("{}.", domain.trim_end_matches('.').to_ascii_lowercase());
    zones
        .iter()
        .filter(|z| {
            let z = z.to_ascii_lowercase();
            fq == z || fq.ends_with(&format!(".{z}"))
        })
        .max_by_key(|z| z.len())
}

/// Writes the records for every endpoint of the cluster. Never fails the
/// caller: the result (per record) is returned and stored in the status.
pub async fn publish(
    state: &AppState,
    cluster: ClusterId,
    profile: &ClusterProfile,
    status: &Value,
    by: &AppliedBy,
) -> Value {
    let Some(ingress) = &profile.ingress else { return Value::Null };
    let Some(dns) = &ingress.dns else { return Value::Null };
    let health = health_of(status);
    let addresses = ingress.dns_addresses(|id| health.get(id).is_none_or(|h| h.healthy));
    let at = state.clock.now().0;
    let fail = |e: String| json!({ "addresses": addresses, "published_at": at, "error": e, "records": [] });
    let Ok(node) = dns.server_id.parse::<ServerId>() else { return fail("invalid DNS node".into()) };

    let zones = match run_as(
        state,
        by.principal.clone(),
        by.context,
        node,
        json!({ "type": "dns", "op": { "op": "zones" } }),
        WAIT,
    )
    .await
    {
        Ok(r) if r["status"] == "succeeded" => r["result"]
            .as_array()
            .map(|a| a.iter().filter_map(|z| z["name"].as_str().map(str::to_owned)).collect::<Vec<_>>())
            .unwrap_or_default(),
        Ok(r) => {
            return fail(format!(
                "cannot list zones: {}",
                r["error"].as_str().unwrap_or("no answer from the DNS node")
            ));
        }
        Err(e) => return fail(e.message),
    };

    let cid = cluster.to_string();
    let mut records = Vec::new();
    let sites = state.store.list_sites().await.unwrap_or_default();
    for site in sites.iter().filter(|s| s.config["cluster_id"].as_str() == Some(cid.as_str())) {
        for domain in site.config["spec"]["domains"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            let Some(zone) = zone_for(domain, &zones) else {
                records.push(json!({ "name": domain, "status": "failed", "error": "no zone for this domain on the DNS node (create it on the DNS page)" }));
                continue;
            };
            for (rtype, v6) in [("A", false), ("AAAA", true)] {
                let contents: Vec<&String> = addresses.iter().filter(|a| a.contains(':') == v6).collect();
                if contents.is_empty() {
                    continue;
                }
                let op = json!({ "op": "set_records", "zone": zone, "name": format!("{}.", domain.trim_end_matches('.')), "rtype": rtype, "ttl": dns.ttl, "contents": contents });
                let r = run_as(state, by.principal.clone(), by.context, node, json!({ "type": "dns", "op": op }), WAIT)
                    .await;
                records.push(match r {
                    Ok(r) => json!({ "name": domain, "type": rtype, "zone": zone, "status": r["status"], "error": r["error"] }),
                    Err(e) => json!({ "name": domain, "type": rtype, "zone": zone, "status": "failed", "error": e.message }),
                });
            }
        }
    }
    json!({ "addresses": addresses, "published_at": at, "records": records })
}

/// One check of an edge: agent online and port 80 open on its public address.
async fn probe(state: &AppState, server_id: &str, public_ip: Option<&str>) -> (bool, Option<String>) {
    let Ok(id) = server_id.parse::<ServerId>() else { return (false, Some("invalid id".into())) };
    match state.store.get_server(id).await {
        Ok(s) if s.status == AgentStatus::Online => {}
        Ok(s) => return (false, Some(format!("agent {:?}", s.status).to_lowercase())),
        Err(e) => return (false, Some(e.to_string())),
    }
    let Some(ip) = public_ip.and_then(|i| i.parse::<std::net::IpAddr>().ok()) else {
        return (false, Some("no public IP".into()));
    };
    // A real HTTP exchange, not just a TCP connect: the kernel of a frozen or
    // hung host still completes the handshake, but nothing answers.
    let exchange = async {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut s = tokio::net::TcpStream::connect((ip, 80)).await.map_err(|e| format!("port 80: {e}"))?;
        s.write_all(b"HEAD / HTTP/1.0\r\nHost: forge-health\r\nUser-Agent: forge-ingress-health\r\n\r\n")
            .await
            .map_err(|e| format!("port 80: {e}"))?;
        let mut buf = [0u8; 12];
        let n = s.read(&mut buf).await.map_err(|e| format!("port 80: {e}"))?;
        if buf[..n].starts_with(b"HTTP/") { Ok(()) } else { Err("port 80: no HTTP answer".to_owned()) }
    };
    match tokio::time::timeout(PROBE_TIMEOUT, exchange).await {
        Ok(Ok(())) => (true, None),
        Ok(Err(e)) => (false, Some(e)),
        Err(_) => (false, Some("port 80: no answer within 3 s".into())),
    }
}

/// One pass over a cluster: probe, update health, republish on change.
async fn check_cluster(state: &AppState, cluster: ClusterId) -> Result<(), String> {
    let Some((profile, mut status)) = super::ingress::load_profile(state, cluster).await.map_err(|e| e.message)? else {
        return Ok(());
    };
    let Some(ingress) = &profile.ingress else { return Ok(()) };
    let Some(dns) = &ingress.dns else { return Ok(()) };
    if !dns.auto_failover || ingress.mode == IngressMode::Standalone {
        return Ok(());
    }
    let Some(by) = status.get("applied_by").cloned().and_then(|v| serde_json::from_value::<AppliedBy>(v).ok()) else {
        return Ok(()); // never applied
    };
    let now = state.clock.now().0;
    let mut health = health_of(&status);
    health.retain(|id, _| ingress.edges.iter().any(|e| &e.server_id == id));
    let mut flipped = Vec::new();
    for e in &ingress.edges {
        let (ok, err) = probe(state, &e.server_id, e.public_ip.as_deref()).await;
        let h = health.entry(e.server_id.clone()).or_default();
        if h.observe(ok, err.clone(), now) {
            flipped.push(json!({ "edge": e.server_id, "healthy": h.healthy, "error": err }));
        }
    }
    status["health"] = serde_json::to_value(&health).map_err(|e| e.to_string())?;
    let wanted = ingress.dns_addresses(|id| health.get(id).is_none_or(|h| h.healthy));
    let published: Vec<String> =
        serde_json::from_value(status.pointer("/dns/addresses").cloned().unwrap_or(Value::Null)).unwrap_or_default();
    if wanted != published {
        let result = publish(state, cluster, &profile, &status, &by).await;
        let ok = result.get("error").is_none()
            && result["records"].as_array().is_some_and(|r| r.iter().all(|x| x["status"] == "succeeded"));
        status["dns"] = result;
        let ws = state.store.get_cluster(cluster).await.ok().map(|c| c.workspace_id);
        let ev = AuditEvent::new(
            "system:ingress-health",
            "ingress.dns.failover",
            EntityRef::new(EntityKind::Cluster, cluster),
            if ok { AuditOutcome::Success } else { AuditOutcome::Failure },
        )
        .in_org(ws)
        .with_details(
            json!({ "from": published, "to": wanted, "changes": flipped, "on_behalf_of": by.principal.to_string() }),
        );
        if let Err(e) = state.audit.append(ev).await {
            tracing::error!(error = %e, "audit write failed");
        }
        if ok {
            tracing::info!(cluster = %cluster, from = ?published, to = ?wanted, "ingress DNS moved");
        } else {
            tracing::warn!(cluster = %cluster, "ingress DNS update failed; retrying next pass");
        }
    }
    super::ingress::save_status(state, cluster, &profile, status).await.map_err(|e| e.message)
}

/// Background loop.
pub(crate) async fn run(state: AppState, mut shutdown: watch::Receiver<bool>) {
    let mut tick = tokio::time::interval(TICK);
    loop {
        tokio::select! {
            _ = tick.tick() => {}
            _ = shutdown.changed() => break,
        }
        let Ok(all) = state.store.list_cluster_profiles().await else { continue };
        for rec in all {
            let wants = rec.profile.pointer("/ingress/dns/auto_failover").and_then(Value::as_bool).unwrap_or(false)
                && rec.profile.pointer("/ingress/mode").and_then(Value::as_str) != Some("standalone");
            if !wants {
                continue;
            }
            if let Err(e) = check_cluster(&state, rec.cluster_id).await {
                tracing::warn!(cluster = %rec.cluster_id, error = %e, "ingress health check failed");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::zone_for;

    #[test]
    fn picks_the_longest_matching_zone() {
        let zones = vec!["example.com.".to_owned(), "eu.example.com.".to_owned(), "other.org.".to_owned()];
        assert_eq!(zone_for("shop.eu.example.com", &zones).map(String::as_str), Some("eu.example.com."));
        assert_eq!(zone_for("example.com", &zones).map(String::as_str), Some("example.com."));
        assert_eq!(zone_for("SHOP.Example.com", &zones).map(String::as_str), Some("example.com."));
        assert!(zone_for("notexample.com", &zones).is_none());
    }
}
