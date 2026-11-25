//! Orbyn Forge control plane.
//!
//! [`App::start`] wires everything (see [`state`]) and runs three loops:
//! the HTTP API, the agent hub and the job engine, plus small maintenance
//! tasks. It is used by `main` and by the integration tests.

pub mod access;
pub mod agent_hub;
pub mod agent_keys;
pub mod agent_update_overview;
pub mod agent_update_policy;
pub mod alerting;
pub mod allowed_hosts;
pub mod analytics_control;
pub mod analytics_install;
pub mod analytics_tuning;
pub mod analytics_worker;
pub mod api;
pub mod app_health_status;
pub mod app_roots;
pub mod app_storage;
pub mod auth;
pub mod bootstrap;
pub mod bucket_grants;
pub mod captcha;
pub mod challenge;
pub mod config;
pub mod console_access;
pub mod container_exec;
pub mod container_runtime;
pub mod container_telemetry;
pub mod deploy;
pub mod docker_engine;
pub mod docker_resources;
pub mod endpoints;
pub mod error;
pub mod events;
pub mod facts;
pub mod file_transport;
pub mod health_ingest;
pub mod health_secrets;
pub mod install;
pub mod jobs;
pub mod logs;
pub mod measures;
pub mod metrics;
pub mod mfa_config;
pub mod names;
pub mod net_access;
pub mod node_rpc;
pub mod notify_mail;
pub mod notifying;
pub mod objstore;
pub mod perms;
pub mod pins;
pub mod public_address;
pub mod rate_limit;
pub mod registries;
pub mod request_origin;
pub mod service_ports;
pub mod stacks;
pub mod state;
pub mod swarm;
pub mod swarm_stacks;
mod tasks;
mod throttle;
pub mod tunnels;
pub mod ui_embed;
pub mod update;
pub mod webhooks;

use std::net::SocketAddr;

use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::task::JoinHandle;

pub use config::ServerConfig;
pub use state::{AppState, Services};

/// Version of this build: `FORGE_BUILD_VERSION` at compile time (local test
/// release feeds, `scripts/setup_testing/`), otherwise the crate version.
pub const VERSION: &str = match option_env!("FORGE_BUILD_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

/// A running control plane.
#[derive(Debug)]
pub struct App {
    pub state: AppState,
    pub api_addr: SocketAddr,
    pub agent_addr: SocketAddr,
    /// SFTP gateway address, when enabled.
    pub sftp_addr: Option<SocketAddr>,
    shutdown: watch::Sender<bool>,
    tasks: Vec<JoinHandle<()>>,
}

impl App {
    /// Builds services, binds both listeners and starts all loops.
    pub async fn start(config: ServerConfig) -> anyhow::Result<Self> {
        let api_listener = TcpListener::bind(config.api_listen).await?;
        let agent_listener = TcpListener::bind(config.agent_listen).await?;
        let state = Services::build(config).await?;

        // One deployment is one workspace (D-016): requests fail closed
        // (`503 multiple_workspaces`) if the store holds more.
        match forge_core::ports::TenancyRepository::list_workspaces(&state.store).await {
            Ok(w) if w.len() > 1 => tracing::error!(
                count = w.len(),
                "the database holds more than one workspace; Forge supports one (D-016) and refuses workspace requests"
            ),
            _ => {}
        }

        Self::start_with(state, api_listener, agent_listener)
    }

    /// Starts on pre-bound listeners (tests bind port 0).
    pub fn start_with(state: AppState, api: TcpListener, agent: TcpListener) -> anyhow::Result<Self> {
        let api_addr = api.local_addr()?;
        let agent_addr = agent.local_addr()?;
        let (shutdown, rx) = watch::channel(false);
        let mut tasks = Vec::new();

        let router = api::router(state.clone());
        let mut srx = rx.clone();
        tasks.push(tokio::spawn(async move {
            tracing::info!(addr = %api_addr, "API listening");

            // The peer address reaches handlers (per-address sign-in limits).
            let app = router.into_make_service_with_connect_info::<std::net::SocketAddr>();
            let serve = axum::serve(api, app).with_graceful_shutdown(async move {
                let _ = srx.changed().await;
            });

            if let Err(e) = serve.await {
                tracing::error!(error = %e, "API server failed");
            }
        }));

        let (hub_state, hub_rx) = (state.clone(), rx.clone());
        tasks.push(tokio::spawn(async move {
            if let Err(e) = agent_hub::serve(hub_state, agent, hub_rx).await {
                tracing::error!(error = %e, "agent hub failed");
            }
        }));

        tasks.push(tokio::spawn(state.jobs.clone().run(rx.clone())));
        tasks.push(tokio::spawn(api::fabric_reconcile::run(state.clone(), rx.clone())));
        tasks.push(tokio::spawn(names::run(state.clone(), rx.clone())));
        tasks.push(tokio::spawn(alerting::run(state.clone(), rx.clone())));
        tasks.push(tokio::spawn(notifying::run(state.clone(), rx.clone())));
        tasks.push(tokio::spawn(access::run(state.clone(), rx.clone())));

        // The analytics worker (D-141) and the maps it is fed.
        tasks.push(tokio::spawn(state.analytics_worker.clone().run(rx.clone())));
        tasks.push(tokio::spawn(analytics_worker::maps::run(
            state.analytics_maps.clone(),
            state.analytics_client.clone(),
            rx.clone(),
        )));
        tasks.push(tokio::spawn(api::ingress_dns::run(state.clone(), rx.clone())));
        tasks.push(tokio::spawn(api::cert_renewal::run(state.clone(), rx.clone())));
        tasks.push(tokio::spawn(api::package_updates::run(state.clone(), rx.clone())));
        tasks.push(tokio::spawn(api::system_update::run(state.clone(), rx.clone())));
        tasks.push(tokio::spawn(api::agent_updates::auto_run(state.clone(), rx.clone())));

        // Forge-managed SFTP gateway (temporary file access).
        let mut sftp_addr = None;

        if let (true, Some(key)) = (state.config.sftp.enabled, state.sftp_host_key.clone()) {
            let std_listener = std::net::TcpListener::bind(state.config.sftp.listen)?;
            std_listener.set_nonblocking(true)?;
            let listener = TcpListener::from_std(std_listener)?;
            let addr = listener.local_addr()?;
            sftp_addr = Some(addr);
            let _ = state.sftp_bound.set(addr);
            tracing::info!(addr = %addr, fingerprint = %key.fingerprint(), "SFTP gateway listening");
            tasks.push(tokio::spawn(forge_file_gateway::serve(listener, key, state.files.clone(), rx.clone())));
        }

        tasks.push(tokio::spawn(tasks::agent_key_upkeep(state.clone(), rx.clone())));
        tasks.push(tokio::spawn(tasks::offline_monitor(state.clone(), rx)));

        Ok(Self { state, api_addr, agent_addr, sftp_addr, shutdown, tasks })
    }

    /// Signals shutdown and waits for all loops to stop.
    pub async fn stop(self) {
        let _ = self.shutdown.send(true);

        for t in self.tasks {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), t).await;
        }
    }
}
