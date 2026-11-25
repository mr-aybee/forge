//! The service port catalogue (D-100): which ports listen for which service
//! on which node, built read-only from what Forge already knows. Port
//! tunnels resolve their targets here; nothing is typed by hand.
//!
//! | Source | Ports | Dial address |
//! | --- | --- | --- |
//! | Native app (`spec.listen_port`) | one | loopback |
//! | Container app (`spec.ports` `[ip:]host:container`) | host ports | the bound address, else loopback |
//! | Stack service (the node plan's published ports) | published | the bound address (`0.0.0.0` → loopback) |
//! | App Store package (`installations` + the package's ports or `port` params) | package ports | loopback |
//! | SSH | the node's SSH endpoint, else 22 | loopback |

use std::collections::{BTreeMap, HashMap};
use std::net::IpAddr;

use forge_catalog::names::published_port;
use forge_core::model::{AgentStatus, Environment, Server};
use forge_core::ports::{
    InstallationRepository, ServerRepository, ServiceRepository, SshEndpointRepository, TenancyRepository,
};
use forge_core::stacks::StackRepository;
use forge_core::tunnels::TunnelTargetKind;
use forge_core::{EnvironmentId, ServerId, WorkspaceId};
use serde::Serialize;

use crate::state::AppState;

/// Loopback, as the agent dials it when no address is given.
pub const LOOPBACK: &str = "127.0.0.1";

/// One listening port of one service on one node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ServicePort {
    /// `app`, `stack`, `package` or `ssh`.
    pub kind: TunnelTargetKind,
    /// The target reference tunnels and grants store: the service id,
    /// `<stack id>/<service>`, the installation id or the server id.
    pub id: String,
    /// The name people type: the app name, `<stack>/<service>`, the package
    /// id (`postgresql`) or the node's label.
    pub name: String,
    /// Stack service: the stack's name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stack: Option<String>,
    /// Stack service: the compose service name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    /// Shown in lists: `web/site`, `web/seq-b/seq`, `postgresql on lab-b`, `ssh on lab-b`.
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environment_id: Option<EnvironmentId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,
    pub server_id: ServerId,
    /// The node's label.
    pub server: String,
    pub online: bool,
    pub port: u16,
    /// `tcp` or `udp`.
    pub proto: String,
    /// `http`, `db`, `ssh` or `tcp`: what a client would speak to it.
    pub role: String,
    /// The address the agent dials on the node.
    pub dial_ip: String,
}

impl ServicePort {
    /// Whether `q` names this entry: its id, name, label, `<ns>/<name>` or,
    /// for a stack service, `<stack>/<service>`.
    #[must_use]
    pub fn matches(&self, q: &str) -> bool {
        let q = q.trim();
        if q.is_empty() {
            return false;
        }

        q == self.id
            || q == self.name
            || q == self.label
            || self.environment.as_deref().is_some_and(|ns| q == format!("{ns}/{}", self.name))
            || (self.kind == TunnelTargetKind::Ssh && q == self.server_id.to_string())
    }
}

/// What a port most likely speaks, from the port inside the container.
#[must_use]
pub fn role_for_target(target: u16) -> &'static str {
    match target {
        80 | 443 | 8000 | 8008 | 8080 | 8443 | 3000 | 5000 | 5080 => "http",
        3306 | 5432 | 6379 | 27017 | 1433 | 9200 | 11211 | 8086 => "db",
        22 => "ssh",
        _ => "tcp",
    }
}

fn role_for_package(p: &forge_catalog::Package) -> &'static str {
    match p.category {
        forge_catalog::Category::Databases => "db",
        forge_catalog::Category::Web | forge_catalog::Category::Monitoring | forge_catalog::Category::Tools => "http",
        _ => "tcp",
    }
}

/// `host_ip` of a published port as the agent should dial it: a real
/// address stays, everything else (empty, `0.0.0.0`, `::`) is loopback.
#[must_use]
pub fn dial_address(host_ip: &str) -> String {
    match host_ip.trim().trim_matches(['[', ']']).parse::<IpAddr>() {
        Ok(ip) if !ip.is_unspecified() => ip.to_canonical().to_string(),
        _ => LOOPBACK.to_owned(),
    }
}

/// Builds the catalogue of one workspace.
pub async fn catalogue(state: &AppState, ws: WorkspaceId) -> Result<Vec<ServicePort>, String> {
    let e = |e: forge_core::DomainError| e.to_string();
    let mut servers = state.store.list_servers(Some(ws)).await.map_err(e)?;
    servers.retain(|s| s.status != AgentStatus::Revoked);
    let refs: Vec<&Server> = servers.iter().collect();
    let labels = crate::names::node_labels(&refs, &crate::names::stored_labels(state, ws).await?);
    let label = |id: ServerId| labels.get(&id).map(|l| l.label.clone()).unwrap_or_default();
    let by_id: HashMap<ServerId, &Server> = servers.iter().map(|s| (s.id, s)).collect();
    let online = |id: ServerId| by_id.get(&id).is_some_and(|s| s.status == AgentStatus::Online);
    let environments: HashMap<EnvironmentId, Environment> =
        state.store.list_environments(ws, None).await.map_err(e)?.into_iter().map(|n| (n.id, n)).collect();
    let mut out = Vec::new();

    // Services: active instances, ports from the stored spec.
    let instances = state.store.list_all_instances(None).await.map_err(e)?;
    let mut apps = HashMap::new();

    for i in instances.iter().filter(|i| i.role != "standby" && by_id.contains_key(&i.server_id)) {
        if let std::collections::hash_map::Entry::Vacant(v) = apps.entry(i.service_id) {
            v.insert(state.store.get_service(i.service_id).await.ok().flatten());
        }

        let Some(Some(app)) = apps.get(&i.service_id) else { continue };
        let Some(ns) = environments.get(&app.environment_id) else { continue };
        let entry = |port: u16, proto: &str, role: &str, dial_ip: String| ServicePort {
            kind: TunnelTargetKind::App,
            id: app.id.to_string(),
            name: app.name.clone(),
            stack: None,
            service: None,
            label: format!("{}/{}", ns.name, app.name),
            environment_id: Some(ns.id),
            environment: Some(ns.name.clone()),
            server_id: i.server_id,
            server: label(i.server_id),
            online: online(i.server_id),
            port,
            proto: proto.to_owned(),
            role: role.to_owned(),
            dial_ip,
        };

        if let Some(p) =
            app.spec.get("listen_port").and_then(serde_json::Value::as_u64).and_then(|p| u16::try_from(p).ok())
            && p > 0
        {
            out.push(entry(p, "tcp", "http", LOOPBACK.to_owned()));
        }

        for p in app.spec.get("ports").and_then(serde_json::Value::as_array).into_iter().flatten() {
            if let Some(pp) = p.as_str().and_then(published_port) {
                let dial = pp.bind.map_or_else(|| LOOPBACK.to_owned(), |b| b.to_canonical().to_string());
                out.push(entry(pp.port, &pp.proto, role_for_target(pp.target), dial));
            }
        }
    }

    // Compose stacks: the published ports of each node's part, from the plan
    // of the deployed revision (the latest one before the first deploy).
    let deps = crate::stacks::Deps {
        servers: &state.store,
        clusters: &state.store,
        fabrics: &state.store,
        files: &state.store,
    };

    for stack in state.store.list_stacks(None).await.map_err(e)?.into_iter().filter(|s| s.workspace_id == ws) {
        let Some(ns) = environments.get(&stack.environment_id) else { continue };
        let rev_no = if stack.deployed_revision > 0 { stack.deployed_revision } else { stack.revision };
        let Ok(Some(rev)) = state.store.get_revision(stack.id, rev_no).await else { continue };
        let Ok(env) = crate::stacks::open_env(&state.envelope, stack.id, stack.env_sealed.as_deref()) else { continue };
        let ns_slug = crate::stacks::ns_slug(&ns.name);

        let plan = match crate::stacks::plan(
            &deps,
            ws,
            stack.default_server,
            &ns_slug,
            &stack.name,
            stack.dir.as_deref(),
            &rev.content,
            &env,
            true,
        )
        .await
        {
            Ok(p) => p,
            Err(err) => {
                tracing::debug!(stack = %stack.name, error = %err, "stack not planned for the port catalogue");
                continue;
            }
        };

        let parts: HashMap<ServerId, String> = state
            .store
            .list_nodes(stack.id)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|n| (n.server_id, n.status))
            .collect();

        for node in plan.nodes.iter().filter(|n| by_id.contains_key(&n.server_id)) {
            if parts.get(&node.server_id).is_none_or(|s| s == "removing") {
                continue;
            }

            for p in &node.ports {
                let Ok(port) = p.published.trim().parse::<u16>() else { continue };
                let target = p.target.trim().parse::<u16>().unwrap_or(0);

                if port == 0 {
                    continue;
                }

                out.push(ServicePort {
                    kind: TunnelTargetKind::Stack,
                    id: format!("{}/{}", stack.id, p.service),
                    name: format!("{}/{}", stack.name, p.service),
                    stack: Some(stack.name.clone()),
                    service: Some(p.service.clone()),
                    label: format!("{}/{}/{}", ns.name, stack.name, p.service),
                    environment_id: Some(ns.id),
                    environment: Some(ns.name.clone()),
                    server_id: node.server_id,
                    server: label(node.server_id),
                    online: online(node.server_id),
                    port,
                    proto: if p.protocol.is_empty() { "tcp".to_owned() } else { p.protocol.to_ascii_lowercase() },
                    role: role_for_target(target).to_owned(),
                    dial_ip: dial_address(&p.host_ip),
                });
            }
        }
    }

    // App Store packages: installed ones, with the port parameters they were
    // installed with, else the package's default ports.
    for inst in state.store.list_installations(None).await.map_err(e)? {
        if inst.status != "installed" || !by_id.contains_key(&inst.server_id) {
            continue;
        }

        let Some(pkg) = forge_catalog::find(&inst.package) else { continue };
        let ports = package_ports(pkg, &inst.params);

        for port in ports {
            out.push(ServicePort {
                kind: TunnelTargetKind::Package,
                id: inst.id.to_string(),
                name: pkg.id.to_owned(),
                stack: None,
                service: None,
                label: format!("{} on {}", pkg.id, label(inst.server_id)),
                environment_id: None,
                environment: None,
                server_id: inst.server_id,
                server: label(inst.server_id),
                online: online(inst.server_id),
                port,
                proto: "tcp".into(),
                role: role_for_package(pkg).to_owned(),
                dial_ip: LOOPBACK.to_owned(),
            });
        }
    }

    // SSH: every node, on its configured endpoint port (22 by default).
    for s in &servers {
        let port = state.store.get_ssh_endpoint(s.id).await.ok().flatten().map_or(22, |ep| ep.port);
        out.push(ServicePort {
            kind: TunnelTargetKind::Ssh,
            id: s.id.to_string(),
            name: label(s.id),
            stack: None,
            service: None,
            label: format!("ssh on {}", label(s.id)),
            environment_id: None,
            environment: None,
            server_id: s.id,
            server: label(s.id),
            online: s.status == AgentStatus::Online,
            port,
            proto: "tcp".into(),
            role: "ssh".into(),
            dial_ip: LOOPBACK.to_owned(),
        });
    }

    out.sort_by(|a, b| {
        (a.label.as_str(), a.server.as_str(), a.port).cmp(&(b.label.as_str(), b.server.as_str(), b.port))
    });
    out.dedup();

    Ok(out)
}

/// A package's listening ports: every `*port` parameter it was installed
/// with (`port`, `http_port`, `console_port`), else its declared ports.
#[must_use]
pub fn package_ports(pkg: &forge_catalog::Package, params: &BTreeMap<String, String>) -> Vec<u16> {
    let mut from_params: Vec<u16> = pkg
        .params
        .iter()
        .filter(|p| p.key == "port" || p.key.ends_with("_port"))
        .filter_map(|p| params.get(p.key).map(String::as_str).or(Some(p.default)).and_then(|v| v.parse().ok()))
        .filter(|p| *p > 0)
        .collect();

    if from_params.is_empty() {
        from_params = pkg.ports.to_vec();
    }

    from_params.sort_unstable();
    from_params.dedup();

    from_params
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn entry(kind: TunnelTargetKind, id: &str, name: &str, ns: Option<&str>, port: u16) -> ServicePort {
        ServicePort {
            kind,
            id: id.into(),
            name: name.into(),
            stack: None,
            service: None,
            label: ns.map_or_else(|| name.to_owned(), |n| format!("{n}/{name}")),
            environment_id: None,
            environment: ns.map(str::to_owned),
            server_id: ServerId::new(),
            server: "lab-b".into(),
            online: true,
            port,
            proto: "tcp".into(),
            role: "http".into(),
            dial_ip: LOOPBACK.into(),
        }
    }

    #[test]
    fn entries_match_ids_names_and_qualified_names() {
        let app = entry(TunnelTargetKind::App, "app-1", "site", Some("web"), 3000);
        assert!(app.matches("site") && app.matches("app-1") && app.matches("web/site"));
        assert!(!app.matches("sites") && !app.matches("") && !app.matches("prod/site"));
        let svc = entry(TunnelTargetKind::Stack, "stk-1/seq", "seq-b/seq", Some("web"), 80);
        assert!(svc.matches("seq-b/seq") && svc.matches("web/seq-b/seq") && svc.matches("stk-1/seq"));
        assert!(!svc.matches("seq-b"), "a stack alone is not a service");
        let ssh = entry(TunnelTargetKind::Ssh, "n1", "lab-b", None, 22);
        assert!(ssh.matches("lab-b") && ssh.matches(&ssh.server_id.to_string()));
    }

    #[test]
    fn dial_addresses_and_roles() {
        assert_eq!(dial_address(""), "127.0.0.1");
        assert_eq!(dial_address("0.0.0.0"), "127.0.0.1");
        assert_eq!(dial_address("::"), "127.0.0.1");
        assert_eq!(dial_address("10.64.0.17"), "10.64.0.17");
        assert_eq!(dial_address("[::1]"), "::1");
        assert_eq!(role_for_target(80), "http");
        assert_eq!(role_for_target(5432), "db");
        assert_eq!(role_for_target(5341), "tcp");
    }

    #[test]
    fn package_ports_follow_the_install_parameters() {
        let pg = forge_catalog::find("postgresql").unwrap();
        assert_eq!(package_ports(pg, &BTreeMap::new()), vec![5432]);
        let mut params = BTreeMap::new();
        params.insert("port".to_owned(), "5433".to_owned());
        assert_eq!(package_ports(pg, &params), vec![5433]);
        let nginx = forge_catalog::find("nginx").unwrap();
        assert_eq!(package_ports(nginx, &BTreeMap::new()), nginx.ports.to_vec(), "declared ports without params");
    }
}
