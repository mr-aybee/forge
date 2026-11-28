//! The **Open console** links of a service (`design/plans/service-orchestration.md`
//! step 5): its console link (`spec.console`) resolved to URLs, so the web
//! console does not repeat the address rules. Forge only builds the URL; it
//! neither proxies the page nor signs anyone in.
//!
//! An ingress endpoint that routes to the service at the console port wins:
//! its first host name, with the endpoint's scheme and port. Otherwise each
//! node the service is placed on gives one link, at the node's address and
//! the console port.
//!
//! **Open shell** reuses the container shell (`container.exec`) and the
//! node terminal (`terminal.open`); [`shell_targets`] only says where each
//! one goes and whether the caller may open it. Both endpoints authorize
//! and audit on their own.

use forge_catalog::console_link::{self, ConsoleLink, ConsoleScheme};
use forge_catalog::ingress::Target;
use forge_core::model::{Service, ServiceInstance, Site};
use forge_core::ports::{ServerRepository, SiteRepository};
use forge_core::{Action, DomainResult, EntityKind, EntityRef, ServerId, ServiceId};
use serde::Serialize;
use serde_json::Value;

use crate::state::AppState;

/// One URL a service's console opens on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConsoleUrl {
    /// The node it points at; absent for an ingress endpoint.
    pub server_id: Option<ServerId>,
    /// The host name or address in the URL.
    pub hostname: String,
    pub url: String,
}

/// The service's console link as stored, if it has one. A stored value
/// that does not read back counts as none: `apply_input` validated it.
#[must_use]
pub fn link_of(app: &Service) -> Option<ConsoleLink> {
    app.spec.get(console_link::SPEC_KEY).filter(|v| !v.is_null()).and_then(|v| serde_json::from_value(v.clone()).ok())
}

/// The console URLs of `app`, placed on `instances` (none without a link).
pub async fn console_urls(
    state: &AppState,
    app: &Service,
    instances: &[ServiceInstance],
) -> DomainResult<Vec<ConsoleUrl>> {
    let Some(link) = link_of(app) else { return Ok(Vec::new()) };

    let mut sites = SiteRepository::list_sites(&state.store).await?;
    sites.retain(|s| s.workspace_id == app.workspace_id);
    let routed = ingress_urls(&link, app.id, &sites);

    if !routed.is_empty() {
        return Ok(routed);
    }

    let mut out = Vec::new();

    for i in instances {
        let host = match crate::facts::node_address(state, i.server_id).await? {
            Some(a) => a,
            None => ServerRepository::get_server(&state.store, i.server_id).await?.hostname,
        };

        out.push(ConsoleUrl { server_id: Some(i.server_id), url: link.url_on(&host), hostname: host });
    }

    Ok(out)
}

/// The URLs of the ingress endpoints that route to service `id` at the
/// console port.
#[must_use]
pub fn ingress_urls(link: &ConsoleLink, id: ServiceId, sites: &[Site]) -> Vec<ConsoleUrl> {
    let id = id.to_string();
    let mut out = Vec::new();

    for site in sites.iter().filter(|s| routes_to(&s.config, &id, link.port)) {
        let spec = &site.config["spec"];

        let Some(host) =
            spec["domains"].as_array().into_iter().flatten().filter_map(Value::as_str).find(|d| !d.starts_with('*'))
        else {
            continue;
        };

        let tls = spec["tls"].as_str().is_some_and(|t| t != "off");
        let (scheme, key, default) =
            if tls { (ConsoleScheme::Https, "https_port", 443) } else { (ConsoleScheme::Http, "http_port", 80) };
        let port = spec[key].as_u64().and_then(|p| u16::try_from(p).ok()).unwrap_or(default);

        out.push(ConsoleUrl {
            server_id: None,
            hostname: host.to_owned(),
            url: console_link::url(scheme, host, Some(port), &link.path),
        });
    }

    out
}

/// Whether a site's typed targets (default rule or a path rule) include
/// service `id` at `port`.
fn routes_to(config: &Value, id: &str, port: u16) -> bool {
    let hits = |v: &Value| {
        ["targets", "standby_targets"].iter().any(|k| {
            // A missing or foreign list routes nowhere.
            serde_json::from_value::<Vec<Target>>(v[*k].clone())
                .unwrap_or_default()
                .iter()
                .any(|t| matches!(t, Target::App { service_id, port: p } if service_id == id && *p == port))
        })
    };

    hits(config) || config["spec"]["rules"].as_array().is_some_and(|rules| rules.iter().any(hits))
}

/// Where **Open shell** goes on one node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ShellTarget {
    pub server_id: ServerId,
    /// A container service: the container to `docker exec` into. Absent
    /// for a native service: the node terminal.
    pub container: Option<String>,
}

/// The nodes of `app` the caller may open a shell on: a container shell
/// (`container.exec`) for a container service, the node terminal
/// (`terminal.open`) for a native one. A Swarm service has none: Swarm
/// names its containers. A hint for the console; the shell endpoints
/// authorize every session.
pub async fn shell_targets(
    state: &AppState,
    caller: &crate::auth::Caller,
    app: &Service,
    instances: &[ServiceInstance],
) -> DomainResult<Vec<ShellTarget>> {
    let container = match app.runtime.as_str() {
        "container" if app.spec.get("swarm").is_none_or(Value::is_null) => true,
        "systemd" => false,
        _ => return Ok(Vec::new()),
    };

    let action = if container { Action::ContainerExec } else { Action::TerminalOpen };
    let refs: Vec<EntityRef> = instances.iter().map(|i| EntityRef::new(EntityKind::Server, i.server_id)).collect();
    let allowed = caller.can_many(action, refs).await;

    let slots = if container {
        forge_core::deploy::DeploymentRepository::list_slots(&state.store, app.id).await?
    } else {
        Vec::new()
    };

    let env = app.spec.get("environment").and_then(Value::as_str).unwrap_or_default();
    let mut out = Vec::new();

    for (i, ok) in instances.iter().zip(allowed) {
        if !ok {
            continue;
        }

        let name = container.then(|| {
            // A deployment runs slot copies (`--<slot>`); the one with the
            // most traffic is the one to look at.
            let slot = slots.iter().filter(|s| s.server_id == i.server_id).max_by_key(|s| s.weight);

            match slot {
                Some(s) => format!("forge-app-{env}-{}--{}", app.name, s.slot),
                None => format!("forge-app-{env}-{}", app.name),
            }
        });

        out.push(ShellTarget { server_id: i.server_id, container: name });
    }

    Ok(out)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use serde_json::json;

    use super::*;

    fn site(config: Value) -> Site {
        Site {
            id: "s1".into(),
            workspace_id: forge_core::WorkspaceId::new(),
            name: "web".into(),
            config,
            edge_status: json!({}),
            created_at: forge_core::Timestamp(0),
            updated_at: forge_core::Timestamp(0),
        }
    }

    #[test]
    fn an_endpoint_routing_to_the_console_port_gives_its_host_name() {
        let id = ServiceId::new();
        let link = ConsoleLink { port: 9001, path: "/ui".into(), scheme: ConsoleScheme::Http };
        let tls = site(json!({
            "spec": { "domains": ["*.example.com", "s3.example.com"], "tls": "acme" },
            "targets": [{ "kind": "app", "service_id": id.to_string(), "port": 9001 }],
        }));
        let other_port = site(json!({
            "spec": { "domains": ["api.example.com"] },
            "targets": [{ "kind": "app", "service_id": id.to_string(), "port": 9000 }],
        }));
        let rule = site(json!({
            "spec": { "domains": ["admin.example.com"], "http_port": 8080,
                      "rules": [{ "targets": [{ "kind": "app", "service_id": id.to_string(), "port": 9001 }] }] },
        }));

        let urls = ingress_urls(&link, id, &[tls, other_port, rule]);
        let got: Vec<&str> = urls.iter().map(|u| u.url.as_str()).collect();

        assert_eq!(got, ["https://s3.example.com/ui", "http://admin.example.com:8080/ui"]);
        assert!(urls.iter().all(|u| u.server_id.is_none()));
    }
}
