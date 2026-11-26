//! HTTP API router.

mod access;
pub(crate) mod agent_updates;
mod alerts;
pub mod analytics_settings;
mod app_health;
pub(crate) mod app_storage;
pub(crate) mod approvals;
mod audit;
pub(crate) mod cert_renewal;
mod certificates;
mod clusters;
pub(crate) mod connectors;
mod console;
mod container_telemetry;
pub(crate) mod containers;
pub mod deployments;
pub(crate) mod device;
mod docker;
mod docker_resources;
mod events;
pub(crate) mod fabric;
pub(crate) mod fabric_reconcile;
mod files;
mod hooks;
pub(crate) mod idempotency;
mod ingress;
pub(crate) mod ingress_dns;
mod jobs;
pub(crate) mod logins;
mod logs;
pub(crate) mod mcp;
mod meta;
mod names;
mod nodes;
mod notifications;
mod objstore;
mod observations;
pub(crate) mod onboarding;
pub(crate) mod openapi;
pub(crate) mod package_updates;
mod packages;
mod passkeys;
mod pins;
mod plugins;
mod processes;
mod projects;
mod reauth;
mod registries;
pub(crate) mod remote_sessions;
mod routing;
pub(crate) mod schema_types;
pub mod server_settings;
mod servers;
mod service_accounts;
mod service_ports;
mod services;
mod session;
mod settings;
mod silences;
mod sites;
mod stacks;
mod storage;
mod storage_grants;
mod swarm;
mod swarm_stacks;
pub mod system_update;
mod tags;
mod telemetry;
mod tenancy;
mod terminal;
mod traffic;
mod tunnels;
mod vault;
mod webhooks;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::http::{HeaderName, HeaderValue, header};
use axum::routing::{delete, get, patch, post, put};
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::trace::TraceLayer;

use crate::state::AppState;

/// Plugin packages are the largest bodies.
const MAX_BODY: usize = 16 * 1024 * 1024;

/// Responses on these paths may carry a secret (session, OAuth or API token,
/// revealed password, SFTP grant, enrollment token). They are never compressed:
/// compressing a secret next to attacker-influenced text leaks it through the
/// compressed size (BREACH). Nor are they stored for `Idempotency-Key` replays.
pub(crate) const NO_COMPRESSION: &[&str] = &[
    "/auth/",
    "/oauth/",
    "/me/",
    "/secrets",
    "/vault/",
    "reveal",
    "token",
    "/sftp",
    "grant",
    "/enroll",
    "/mcp",
    "/docker/engine/",
];

/// Drops `Accept-Encoding` on secret-bearing paths so the compression layer
/// sends them as-is.
async fn no_compression_for_secrets(
    mut req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let path = req.uri().path().to_ascii_lowercase();
    if NO_COMPRESSION.iter().any(|p| path.contains(p)) {
        req.headers_mut().remove(header::ACCEPT_ENCODING);
    }

    next.run(req).await
}

/// Compress everything else (Brotli, Zstandard or gzip, as the client
/// accepts). Images, event streams and tiny bodies are skipped by the default
/// predicate; responses that set a cookie are skipped too.
fn compression() -> tower_http::compression::CompressionLayer<impl tower_http::compression::Predicate> {
    use tower_http::compression::predicate::{DefaultPredicate, Predicate};
    let no_cookie = |_: axum::http::StatusCode,
                     _: axum::http::Version,
                     h: &axum::http::HeaderMap,
                     _: &axum::http::Extensions| { !h.contains_key(header::SET_COOKIE) };

    // File downloads and other raw bytes are sent as-is: they are often
    // compressed already, and they are streamed from the node.
    let not_raw = tower_http::compression::predicate::NotForContentType::const_new("application/octet-stream");

    tower_http::compression::CompressionLayer::new().compress_when(DefaultPredicate::new().and(no_cookie).and(not_raw))
}

pub fn router(state: AppState) -> Router {
    let v1 = Router::new()
        // session & identity
        .route("/auth/login", post(session::login))
        .route("/auth/challenge", get(session::challenge))
        .route("/auth/captcha", get(session::captcha))
        .route("/auth/logout", post(session::logout))
        .route("/auth/totp", post(session::totp_begin))
        .route("/auth/step-up", post(session::step_up))
        .route("/auth/totp/confirm", post(session::totp_confirm))
        .route("/auth/oidc/login", get(session::oidc_login))
        .route("/auth/oidc/callback", get(session::oidc_callback))
        .route("/me", get(session::me).patch(session::update_me))
        .route("/me/capabilities", get(console::capabilities))
        .route("/me/pins", get(pins::list))
        .route("/me/pins/order", put(pins::reorder))
        .route("/me/pins/{kind}/{id}", put(pins::add).delete(pins::remove))
        // MCP connectors: consent, Connected apps, registered clients (issue #6)
        .route("/oauth/consent/{id}", get(connectors::consent_get).post(connectors::consent_decide))
        .route("/oauth/device/{code}", get(device::get).post(device::decide))
        .route("/approvals", get(approvals::list))
        .route("/approvals/{id}", get(approvals::get))
        .route("/approvals/{id}/approve", post(approvals::approve))
        .route("/approvals/{id}/deny", post(approvals::deny))
        .route("/connected-apps", get(connectors::connections))
        .route("/connected-apps/{id}/revoke", post(connectors::revoke_connection))
        .route("/oauth/clients", get(connectors::clients))
        .route("/oauth/clients/{id}/revoke", post(connectors::revoke_client))
        .route("/me/password", post(session::change_password))
        // registry
        .route("/workspace", get(tenancy::current_workspace))
        .route("/tenants", get(tenancy::list_tenants).post(tenancy::create_tenant))
        .route("/enrollment-tokens", post(servers::create_enrollment_token))
        .route("/projects", get(projects::list).post(projects::create))
        .route("/projects/{id}", get(projects::get).put(projects::update).delete(projects::delete))
        .route("/environments", get(tenancy::list_environments).post(tenancy::create_environment))
        .route("/environments/{id}", put(tenancy::rename_environment).delete(tenancy::delete_environment))
        .route("/environments/{id}/cluster", put(tenancy::set_environment_cluster))
        .route("/environments/{id}/project", put(tenancy::set_environment_project))
        .route("/users", get(tenancy::list_users).post(tenancy::create_user))
        .route("/role-bindings", get(tenancy::list_bindings).post(tenancy::create_binding))
        .route("/role-bindings/{id}", delete(tenancy::delete_binding))
        // servers
        .route("/servers", get(servers::list))
        .route("/servers/{id}", get(servers::get).patch(clusters::assign).delete(servers::remove))
        .route("/clusters", get(clusters::list).post(clusters::create))
        .route("/nodes/probe", post(nodes::probe))
        .route("/nodes/onboard", post(nodes::onboard))
        .route("/onboarding", get(nodes::onboarding_info))
        .route("/secrets", get(nodes::list_secrets))
        .route("/secrets/{id}", delete(nodes::delete_secret))
        .route("/clusters/{id}", get(clusters::get).put(clusters::update).delete(clusters::delete))
        .route("/clusters/{id}/profile", get(ingress::get).put(ingress::put))
        .route("/clusters/{id}/swarm", get(swarm::get))
        .route("/registries", get(registries::list).post(registries::create))
        .route("/stack-templates", get(stacks::templates))
        .route("/stacks/from-template", post(stacks::from_template))
        .route("/webhooks", get(webhooks::list).post(webhooks::create))
        .route("/webhooks/{id}", put(webhooks::update).delete(webhooks::delete))
        .route("/webhooks/{id}/rotate", post(webhooks::rotate))
        .route("/webhooks/{id}/deliveries", get(webhooks::deliveries))
        .route("/registries/{id}", put(registries::update).delete(registries::delete))
        .route("/servers/{id}/docker/images", get(docker_resources::images))
        .route("/servers/{id}/docker/volumes", get(docker_resources::volumes))
        .route("/servers/{id}/docker/networks", get(docker_resources::networks))
        .route("/servers/{id}/docker/disk-usage", get(docker_resources::disk_usage))
        .route("/servers/{id}/docker/events", get(docker_resources::events))
        .route("/servers/{id}/docker/ops", post(docker_resources::op))
        .route("/clusters/{id}/swarm/members", post(swarm::join))
        .route("/clusters/{id}/swarm/members/{server}", patch(swarm::update_node).delete(swarm::leave))
        .route("/clusters/{id}/swarm/tokens/rotate", post(swarm::rotate_tokens))
        .route("/clusters/{id}/fabric", get(fabric::get).put(fabric::put).delete(fabric::delete))
        .route("/clusters/{id}/fabric/suggest", post(fabric::suggest))
        .route("/fabric/suggest", post(fabric::suggest_new))
        .route("/vault/items", get(vault::list).post(vault::create))
        .route("/vault/items/{id}", put(vault::update).delete(vault::delete))
        .route("/vault/items/{id}/reveal", post(vault::reveal))
        .route("/clusters/{id}/fabric/apply", post(fabric::apply))
        .route("/clusters/{id}/fabric/nodes/{node}/resync", post(fabric::resync))
        .route("/clusters/{id}/profile/apply", post(ingress::apply))
        .route("/clusters/{id}/profile/remove", post(ingress::remove))
        .route("/clusters/{id}/live", get(ingress::live))
        .route("/clusters/{id}/routing", get(routing::get))
        .route("/servers/{id}/revoke", post(servers::revoke))
        .route("/servers/{id}/name", axum::routing::put(servers::rename))
        .route("/servers/{id}/dns-name", axum::routing::put(names::set_dns_name))
        .route("/names", get(names::list))
        .route("/tunnels", get(tunnels::list).post(tunnels::open))
        .route("/tunnels/{id}", axum::routing::delete(tunnels::close))
        .route("/tunnels/{id}/control", get(tunnels::control))
        .route("/tunnels/{id}/connect", get(tunnels::connect))
        .route("/tunnels/{id}/ssh-cert", axum::routing::post(tunnels::ssh_cert))
        .route("/tunnel-grants", get(tunnels::list_grants).post(tunnels::create_grant))
        .route("/tunnel-grants/{id}", axum::routing::delete(tunnels::revoke_grant))
        .route("/service-ports", get(service_ports::list))
        .route("/servers/{id}/service-ports", get(service_ports::for_server))
        .route("/names/suffix", get(names::get_suffix).put(names::put_suffix))
        .route("/alert-rules", get(alerts::list_rules).post(alerts::create_rule))
        .route("/alert-rules/{id}", get(alerts::get_rule).put(alerts::update_rule).delete(alerts::delete_rule))
        .route("/alert-subjects", get(alerts::list_subjects))
        .route("/alert-rule-templates", get(alerts::list_templates))
        .route("/alarms", get(alerts::list_alarms))
        .route("/alarms/{id}", get(alerts::get_alarm))
        .route("/alarms/{id}/acknowledge", post(alerts::acknowledge))
        .route("/alarms/{id}/unacknowledge", post(alerts::unacknowledge))
        .route("/alarms/{id}/notes", post(alerts::add_note))
        .route("/notification-channels", get(notifications::list_channels).post(notifications::create_channel))
        .route(
            "/notification-channels/{id}",
            get(notifications::get_channel).put(notifications::update_channel).delete(notifications::delete_channel),
        )
        .route("/notification-channels/{id}/test", post(notifications::test_channel))
        .route("/notification-policies", get(notifications::list_policies).post(notifications::create_policy))
        .route(
            "/notification-policies/{id}",
            get(notifications::get_policy).put(notifications::update_policy).delete(notifications::delete_policy),
        )
        .route("/notification-deliveries", get(notifications::list_deliveries))
        .route("/notification-deliveries/{id}", get(notifications::get_delivery))
        .route("/silences", get(silences::list_silences).post(silences::create_silence))
        .route("/silences/{id}", get(silences::get_silence))
        .route("/silences/{id}/extend", post(silences::extend_silence))
        .route("/silences/{id}/end", post(silences::end_silence))
        .route("/servers/{id}/commands", post(servers::command))
        .route("/servers/{id}/ssh", get(terminal::get_ssh).put(terminal::put_ssh))
        .route("/servers/{id}/terminal", get(terminal::open))
        .route("/servers/{id}/terminal/check", get(terminal::check))
        .route("/servers/{id}/exec", get(terminal::exec))
        .route("/servers/{id}/containers/{container_id}/shell", get(terminal::container_shell))
        .route("/servers/{id}/containers/{container_id}/shell/check", get(terminal::container_shell_check))
        .route("/terminal-sessions", get(remote_sessions::list))
        .route("/terminal-sessions/{id}/revoke", post(remote_sessions::revoke))
        .route("/ssh/ca", get(terminal::ca))
        .route("/servers/{id}/containers", post(containers::run))
        .route("/servers/{id}/metrics", get(telemetry::metrics))
        .route("/metrics/latest", get(telemetry::latest))
        .route("/servers/{id}/history", get(telemetry::history))
        .route("/servers/{id}/report", get(telemetry::report))
        .route("/analytics/status", get(telemetry::analytics_status))
        .route("/analytics/settings", get(analytics_settings::get).put(analytics_settings::put))
        .route(
            "/analytics/restart",
            axum::routing::post(analytics_settings::restart).delete(analytics_settings::cancel_restart),
        )
        .route("/observations", get(observations::list))
        .route("/observations/history", get(observations::history))
        .route("/traffic", get(traffic::get))
        .route("/system/update", get(system_update::get))
        .route("/system/update/check", post(system_update::check))
        .route("/system/update/releases", get(system_update::releases))
        .route("/system/update/apply", post(system_update::apply))
        .route("/servers/{id}/connectivity", get(telemetry::connectivity).post(telemetry::probe))
        .route("/services", get(services::list).post(services::create))
        .route("/services/{id}", get(services::get).put(services::update).delete(services::delete))
        .route("/services/{id}/instances", axum::routing::put(services::place))
        .route("/services/health-summary", get(app_health::summary_list))
        .route("/services/{id}/health", get(app_health::get).put(app_health::update))
        .route("/services/{id}/health/validate", post(app_health::validate))
        .route("/services/{id}/storage", get(app_storage::list).post(app_storage::attach))
        .route("/services/{id}/storage/{access_key}", axum::routing::delete(app_storage::detach))
        .route("/services/{id}/deploy", post(services::deploy))
        .route("/services/{id}/swarm", get(swarm::service_get).post(swarm::service_action))
        .route("/services/{id}/swarm/logs", get(swarm::service_logs))
        // Releases and deployments (#73)
        .route("/deployments", post(deployments::apply))
        .route("/deployments/validate", post(deployments::validate))
        .route("/deployments/plan", post(deployments::plan))
        .route("/deployments/{id}", get(deployments::get))
        .route("/deployments/{id}/events", get(deployments::events))
        .route("/deployments/{id}/{action}", post(deployments::request))
        .route("/services/{id}/releases", get(deployments::releases))
        .route("/services/{id}/releases/{number}", get(deployments::release))
        .route("/services/{id}/deployments", get(deployments::deployments).post(deployments::deploy_release))
        .route("/services/{id}/rollback", post(deployments::rollback))
        .route("/services/{id}/slots", get(deployments::slots))
        .route("/services/{id}/instances/{server_id}/actions", post(services::action))
        .route("/settings", get(settings::get))
        .route("/server/settings", get(server_settings::get))
        .route(
            "/server/public-address",
            axum::routing::put(server_settings::put_public_address).delete(server_settings::delete_public_address),
        )
        .route(
            "/server/allowed-hosts",
            axum::routing::put(server_settings::put_allowed_hosts).delete(server_settings::delete_allowed_hosts),
        )
        .route("/settings/{key}", axum::routing::put(settings::put).delete(settings::delete))
        .route("/access/catalog", get(access::catalog))
        .route("/access/profiles", get(access::list).post(access::create))
        .route("/access/profiles/{id}", get(access::get).put(access::publish).delete(access::archive))
        .route("/access/profiles/{id}/duplicate", post(access::duplicate))
        .route("/access/assignments", get(access::list_assignments).post(access::assign))
        .route("/access/assignments/{id}", delete(access::unassign))
        .route("/access/groups", get(access::list_groups).post(access::create_group))
        .route("/access/groups/{id}", patch(access::update_group).delete(access::delete_group))
        .route("/access/groups/{id}/members/{user_id}", put(access::add_member).delete(access::remove_member))
        .route("/access/effective", get(access::effective))
        .route("/service-accounts", get(service_accounts::list).post(service_accounts::create))
        .route("/service-accounts/{id}", patch(service_accounts::update))
        .route("/service-accounts/{id}/tokens", get(service_accounts::tokens).post(service_accounts::issue_token))
        .route("/service-accounts/{id}/tokens/{token}", delete(service_accounts::revoke_token))
        .route("/api-keys", get(settings::list_keys).post(settings::create_key))
        .route("/api-keys/{id}", delete(settings::delete_key))
        .route("/users/{id}/api-keys", get(settings::list_user_keys).delete(settings::delete_user_keys))
        .route("/users/{id}/api-keys/{key}", delete(settings::delete_user_key))
        .route("/certificates", get(certificates::list).post(certificates::create))
        .route("/certificates/{id}", delete(certificates::delete))
        .route("/sites", get(sites::list).post(sites::create))
        .route("/stacks", get(stacks::list).post(stacks::create))
        .route("/stacks/preview", post(stacks::preview_stack))
        .route("/stacks/containers", post(stacks::create_container))
        .route("/stacks/{id}", get(stacks::get).put(stacks::update).delete(stacks::delete))
        .route("/stacks/{id}/revisions", get(stacks::revisions))
        .route("/stacks/{id}/revisions/{revision}", get(stacks::revision))
        .route("/stacks/{id}/deploy", post(stacks::deploy))
        .route("/stacks/{id}/services", post(stacks::services))
        .route("/stacks/{id}/refresh", post(stacks::refresh))
        .route("/stacks/{id}/logs", get(stacks::logs))
        .route("/stacks/{id}/down", post(stacks::down))
        .route("/stacks/{id}/expose", post(stacks::expose))
        .route("/sites/preview", post(sites::preview))
        .route("/sites/{id}", get(sites::get).put(sites::update).delete(sites::delete))
        .route("/sites/{id}/apply", post(sites::apply))
        .route("/sites/{id}/remove", post(sites::remove))
        .route("/sites/{id}/cache/clear", post(sites::clear_cache))
        .route("/sites/{id}/logs", get(sites::logs))
        .route("/sites/{id}/renew", post(cert_renewal::renew_now))
        .route("/servers/{id}/dns", post(sites::dns))
        .route("/catalog", get(packages::catalog))
        .route("/packages/install", post(packages::install))
        .route("/installations", get(packages::list))
        .route("/installations/{id}/actions", post(packages::action))
        .route("/installations/{id}/update", post(package_updates::update))
        .route("/installations/{id}/policy", put(package_updates::set_policy))
        .route("/package-updates", get(package_updates::list))
        .route("/package-releases", get(package_updates::releases))
        .route("/package-updates/check", post(package_updates::check))
        // jobs
        .route("/jobs", get(jobs::list).post(jobs::submit))
        .route("/jobs/{id}", get(jobs::get))
        .route("/jobs/{id}/steps", get(jobs::steps))
        .route("/jobs/{id}/live", get(jobs::live))
        .route("/jobs/{id}/cancel", post(jobs::cancel))
        // audit
        .route("/audit", get(audit::list))
        .route("/audit/page", get(audit::page))
        .route("/audit/histogram", get(audit::histogram))
        .route("/audit/actions", get(audit::actions))
        .route("/audit/verify", get(audit::verify))
        // plugins
        .route("/plugins", get(plugins::list).post(plugins::install))
        .route("/plugins/upgrade", post(plugins::upgrade))
        .route("/plugins/{id}", delete(plugins::uninstall))
        .route("/plugins/{id}/enable", post(plugins::enable))
        .route("/plugins/{id}/disable", post(plugins::disable))
        .route("/plugins/{id}/pin", post(plugins::pin))
        .route("/plugins/{id}/unpin", post(plugins::unpin))
        .route("/plugins/{id}/invoke", post(plugins::invoke))
        // live events
        .route("/events", get(events::subscribe))
        // object storage (api/storage.rs)
        .merge(storage::routes())
        .merge(storage_grants::routes())
        // file explorer (api/files.rs)
        .merge(files::routes())
        .merge(onboarding::routes())
        .merge(passkeys::routes())
        // tags on nodes, clusters, environments, services (api/tags.rs)
        .merge(tags::routes())
        .merge(logins::routes())
        .merge(logs::routes())
        .merge(container_telemetry::routes())
        .merge(processes::routes())
        .merge(docker::routes())
        .merge(swarm_stacks::routes())
        .merge(agent_updates::routes())
        // An unknown `/api/v1/...` path is a JSON 404, never the console's
        // index page.
        .fallback(|| async { crate::error::ApiError::not_found("no such API route") })
        // Idempotency-Key on mutations (issue #5, D-074).
        .layer(axum::middleware::from_fn_with_state(state.clone(), idempotency::guard))
        .layer(SetResponseHeaderLayer::overriding(header::CACHE_CONTROL, HeaderValue::from_static("no-store")));

    // Tools at /mcp dispatch through this same router in process (issue #6).
    let mcp_dispatch = mcp::McpRouter(v1.clone().with_state(state.clone()));
    let mut app = Router::new()
        .route("/mcp", post(mcp::post).get(mcp::not_allowed).delete(mcp::not_allowed))
        .route("/api/version", get(meta::version))
        .route("/api/capabilities", get(meta::capabilities))
        .route("/api/openapi.json", get(openapi::spec))
        .route("/api/health", get(meta::health))
        // Seq-compatible log ingest (D-128): authenticated by its ingest key.
        .route("/api/events/raw", post(logs::ingest))
        // Deploy webhooks (D-159): the secret URL is the credential.
        .route("/hooks/{token}", post(hooks::fire).layer(axum::extract::DefaultBodyLimit::max(hooks::MAX_BODY)))
        .route("/hooks/{token}/jobs/{job}", get(hooks::job))
        .route("/install.sh", get(nodes::install_script))
        .route("/install.ps1", get(nodes::install_ps1))
        .route("/agent/download/{os}/{arch}/{binary}", get(nodes::download))
        .route("/oauth/authorize", get(session::oauth_authorize))
        .route("/oauth/token", post(device::token))
        .route("/oauth/device_authorization", post(device::authorize))
        .route("/oauth/register", post(connectors::register))
        .route("/.well-known/oauth-authorization-server", get(connectors::authorization_server_metadata))
        .route("/.well-known/oauth-protected-resource", get(connectors::protected_resource_metadata))
        .route("/.well-known/oauth-protected-resource/mcp", get(connectors::protected_resource_metadata))
        .nest("/api/v1", v1);

    if let Some(dir) = &state.config.ui_dir {
        use tower_http::services::{ServeDir, ServeFile};

        // Hashed build assets never change; everything else (index.html and
        // client-side routes) must be revalidated so a new build is picked up.
        let assets = ServeDir::new(dir.join("assets"));
        let pages = ServeDir::new(dir).fallback(ServeFile::new(dir.join("index.html")));
        let ui = Router::new()
            .nest_service(
                "/assets",
                tower::ServiceBuilder::new()
                    .layer(SetResponseHeaderLayer::overriding(
                        header::CACHE_CONTROL,
                        HeaderValue::from_static("public, max-age=31536000, immutable"),
                    ))
                    .service(assets),
            )
            .fallback_service(
                tower::ServiceBuilder::new()
                    .layer(SetResponseHeaderLayer::overriding(
                        header::CACHE_CONTROL,
                        HeaderValue::from_static("no-cache"),
                    ))
                    .service(pages),
            );
        app = app.fallback_service(ui);
    } else if crate::ui_embed::available() {
        // Release builds carry the console; no files to install.
        app = app.fallback(crate::ui_embed::serve);
    }

    // The /api/v1 router for in-process dispatch: /mcp tools and approvals.
    app.layer(axum::Extension(mcp_dispatch))
        .layer(compression())
        .layer(axum::middleware::from_fn(no_compression_for_secrets))
        .layer(DefaultBodyLimit::max(MAX_BODY))
        .layer(SetResponseHeaderLayer::overriding(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff")))
        .layer(SetResponseHeaderLayer::overriding(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY")))
        .layer(SetResponseHeaderLayer::overriding(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer")))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(
                "default-src 'self'; connect-src 'self'; img-src 'self' data:; style-src 'self' 'unsafe-inline'; frame-ancestors 'none'; base-uri 'none'; object-src 'none'; form-action 'self'",
            ),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("cross-origin-opener-policy"),
            HeaderValue::from_static("same-origin"),
        ))
        // Allowed hosts (421) and HSTS on HTTPS responses only (D-167).
        .layer(axum::middleware::from_fn_with_state(state.clone(), crate::request_origin::guard))
        // Address lists and rate limits before anything else runs (D-161).
        .layer(axum::middleware::from_fn_with_state(state.clone(), crate::net_access::guard))
        // Paths only: query strings may carry OAuth codes and must not be
        // logged; a webhook path is its secret and is redacted too.
        .layer(TraceLayer::new_for_http().make_span_with(|req: &axum::http::Request<_>| {
            tracing::info_span!("http", method = %req.method(), path = %log_path(req.uri().path()))
        }))
        .with_state(state)
}

/// A request path for the log: a webhook's URL token is a secret.
fn log_path(path: &str) -> std::borrow::Cow<'_, str> {
    match path.strip_prefix("/hooks/") {
        Some(rest) => {
            let tail = rest.split_once('/').map_or(String::new(), |(_, t)| format!("/{t}"));
            std::borrow::Cow::Owned(format!("/hooks/<redacted>{tail}"))
        }
        None => std::borrow::Cow::Borrowed(path),
    }
}

#[cfg(test)]
mod log_path_tests {
    #[test]
    fn webhook_tokens_never_reach_the_log() {
        assert_eq!(super::log_path("/hooks/whk_secret"), "/hooks/<redacted>");
        assert_eq!(super::log_path("/hooks/whk_secret/jobs/1"), "/hooks/<redacted>/jobs/1");
        assert_eq!(super::log_path("/api/v1/me"), "/api/v1/me");
    }
}
