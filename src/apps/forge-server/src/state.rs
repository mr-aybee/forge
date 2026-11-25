//! Composition root: the only place concrete adapters are chosen and wired
//! into services. Everything else depends on ports.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use forge_audit::{AuditLog, AuditSigner};
use forge_authz::{CedarAuthorizer, PolicyEngine};
use forge_core::model::RoleBinding;
use forge_core::ports::{Authorizer, RoleBindingRepository};
use forge_core::{Backoff, Clock, DomainResult, RoleBindingId, SystemClock};
use forge_identity::oidc::{OidcConfig, OidcProvider, OpenIdConnectProvider, PendingLogins};
use forge_identity::{IdentityConfig, IdentityService};
use forge_jobs::{EngineConfig, JobEngine, JobRegistry};
use forge_pki::CertificateAuthority;
use forge_plugin_host::{ForgeHostFunctions, PluginHost, PluginHostConfig, ProcessRunner};
use forge_proto::CommandSigner;
use forge_sdk::signing::TrustStore;
use forge_secrets::{Envelope, LocalKeyProvider};
use forge_store::Store;
use tokio::sync::broadcast;

use crate::agent_hub::CommandQueue;
use crate::config::ServerConfig;
use crate::events::ServerEvent;
use crate::install::AgentDist;
use crate::jobs::{AgentCommandJob, DiagnosticsJob, NodeBootstrapJob};
use crate::throttle::LoginThrottle;

pub type AppState = Arc<Services>;

/// All services, constructed once at start-up.
#[derive(Debug)]
pub struct Services {
    pub config: ServerConfig,
    pub store: Store,
    pub policy: Arc<PolicyEngine>,
    pub authz: Arc<dyn Authorizer>,
    pub audit: Arc<AuditLog>,
    pub identity: IdentityService,
    /// Passkeys (WebAuthn) for sign-in and step-up.
    pub passkeys: forge_identity::passkeys::PasskeyService,
    pub oidc: Option<Arc<dyn OidcProvider>>,
    pub oidc_pending: PendingLogins,
    /// Connector sign-ins waiting for consent (issue #6).
    pub consents: crate::api::connectors::ConsentRequests,
    /// Device sign-ins waiting for approval (RFC 8628).
    pub device_requests: crate::api::device::DeviceRequests,
    /// Per-connection limits on MCP tool calls.
    pub mcp_limits: crate::api::mcp::McpLimits,
    /// Open terminal and exec sessions (`forge sessions`, issue #5).
    pub remote_sessions: crate::api::remote_sessions::RemoteSessions,
    /// Serializes assigning, publishing and archiving access profiles, so an
    /// assignment never stays linked to a revision or profile that changed
    /// under it (D-086).
    pub access_lock: tokio::sync::Mutex<()>,
    /// Users whose every open stream (events) must end now, e.g. a disabled
    /// service account.
    pub signed_out: broadcast::Sender<forge_core::UserId>,
    /// Open port tunnels (D-091).
    pub tunnels: crate::tunnels::Tunnels,
    /// Nodes' Docker Engine API over mutual TLS (D-125).
    pub docker: Arc<crate::docker_engine::DockerEngines>,
    /// Releases and deployments of services (#73).
    pub deploy: Arc<crate::deploy::DeployService>,
    /// Container shells waiting for their node to dial back (D-114).
    pub exec_streams: crate::container_exec::ExecStreams,
    pub jobs: Arc<JobEngine>,
    pub plugins: Arc<PluginHost>,
    pub ca: Arc<CertificateAuthority>,
    /// The control-plane key agents pin and its rotation (D-167).
    pub agent_keys: Arc<crate::agent_keys::AgentKeys>,
    pub signer: Arc<CommandSigner>,
    pub hub: Arc<CommandQueue>,
    pub clock: Arc<dyn Clock>,
    pub events: broadcast::Sender<ServerEvent>,
    pub login_throttle: LoginThrottle,
    /// Proof-of-work challenges for password sign-in (D-064).
    pub login_challenges: crate::challenge::LoginChallenges,
    /// Image CAPTCHA after failed sign-ins (T01).
    pub captchas: crate::captcha::Captchas,
    pub envelope: Envelope,
    pub ssh: Arc<dyn forge_core::remote::RemoteConnector>,
    /// Signs short-lived user certificates for browser terminal sessions.
    pub ssh_ca: Arc<forge_ssh::SshUserCa>,
    pub dist: Arc<AgentDist>,
    /// What an agent update would do on each node.
    pub agent_updates: Arc<crate::agent_update_overview::AgentUpdateOverview>,
    /// Live node metrics and connectivity results (in memory).
    pub telemetry: crate::metrics::Telemetry,
    /// Reads from the analytics database, through the analytics worker
    /// (D-127, D-141).
    pub analytics: Arc<dyn forge_core::analytics::AnalyticsReader>,
    /// Forwards service logs and heartbeat telemetry to the analytics worker.
    pub telemetry_sink: Arc<dyn forge_core::analytics::TelemetrySink>,
    /// `[analytics] engine` is not `off`: a worker runs.
    pub analytics_enabled: bool,
    /// The analytics worker's control-socket client.
    pub analytics_client: Arc<crate::analytics_worker::WorkerClient>,
    /// Runs and restarts the analytics worker.
    pub analytics_worker: Arc<crate::analytics_worker::WorkerSupervisor>,
    /// Keeps the worker's node, service and key maps current.
    pub analytics_maps: Arc<crate::analytics_worker::MapsPublisher>,
    /// Service logs: ingest keys and the log console (D-128).
    pub logs: Arc<crate::logs::Logs>,
    /// Per-user pins in the console sidebar.
    pub pins: Arc<crate::pins::PinService>,
    /// Logs and metrics of every Docker container on the nodes.
    pub container_telemetry: Arc<crate::container_telemetry::ContainerTelemetry>,
    /// Analytics settings changed at runtime (D-136).
    pub analytics_control: Arc<crate::analytics_control::AnalyticsControl>,
    /// Observations: validation, freshness and transitions (D-085).
    pub health: forge_health::ObservationRecorder,
    /// File explorer: roots, authorization, uploads and audit (API and SFTP).
    pub files: Arc<forge_files::FileService>,
    /// Docker Swarm in Swarm clusters (D-154).
    pub swarm: Arc<crate::swarm::SwarmService>,
    /// `[security.access]`, parsed once (D-161).
    pub access_lists: crate::net_access::Lists,
    /// API-wide rate limits per client address (D-161).
    /// Agent downloads streaming at once (public, large).
    pub downloads: Arc<tokio::sync::Semaphore>,
    /// Agent enrollment attempts per client address.
    pub enroll_attempts: crate::rate_limit::Windows<std::net::IpAddr>,
    pub limiter: crate::net_access::Limiter,
    /// Deploy webhooks (D-159).
    pub webhooks: Arc<crate::webhooks::WebhookService>,
    /// Container registries and their sealed credentials (D-158).
    pub registries: Arc<crate::registries::RegistryService>,
    /// Docker images, volumes and networks on nodes (D-157).
    pub docker_resources: Arc<crate::docker_resources::DockerResources>,
    /// Ingress endpoint checks: static roots and basic-auth users (D-169).
    pub endpoints: Arc<crate::endpoints::EndpointService>,
    /// Bucket grants: one person's access to one bucket of a store (D-108).
    pub bucket_grants: crate::bucket_grants::BucketGrants,
    /// Vault-backed header values for health checks, handed to nodes (D-111).
    pub health_secrets: crate::health_secrets::HealthSecrets,
    /// Alarm notifications: channels, policies and delivery (D-110), and
    /// silences (`notifier.silences`, D-112).
    pub notifier: forge_alerts::notify::Notifier,
    /// SFTP gateway host key (`None` when the gateway is disabled).
    pub sftp_host_key: Option<Arc<forge_file_gateway::HostKey>>,
    /// Address the SFTP gateway actually listens on (set at start).
    pub sftp_bound: std::sync::OnceLock<std::net::SocketAddr>,
    /// The console address used in links people follow: the runtime setting
    /// `server.public_address`, else `public_url` (D-120).
    pub public_address: forge_core::public_address::PublicAddress,
    /// Hosts the console answers on (`server.allowed_hosts`, D-167).
    pub allowed_hosts: forge_core::allowed_hosts::AllowedHosts,
}

impl Services {
    pub async fn build(config: ServerConfig) -> anyhow::Result<AppState> {
        let access_lists = config.access_lists();
        let data = config.data_dir.clone();
        std::fs::create_dir_all(data.join("keys")).with_context(|| format!("creating {}", data.display()))?;

        // Only the service user may read or write the database, its WAL, the
        // keys, the CA, backups and staged updates (also mitigates
        // RUSTSEC-2026-0459, see DECISIONS.md D-032). The data directory
        // itself must work; anything else that can't be changed is reported.
        let unfixed = crate::perms::harden(&data).map_err(anyhow::Error::msg)?;

        for (path, err) in &unfixed {
            tracing::warn!(
                path = %path.display(),
                error = %err,
                "could not make this file owner-only; fix it with `chown forge:forge <path> && chmod go-rwx <path>`"
            );
        }

        let clock: Arc<dyn Clock> = Arc::new(SystemClock);

        let store = Store::connect(&config.database_url()).await.context("opening database")?;

        // Analytics settings: forge.toml plus the administrators' overrides (D-136).
        if config.analytics.removed_memory_limit_mb.is_some() {
            tracing::warn!(
                "[analytics] memory_limit_mb is no longer used and is ignored; embedded_memory_mb caps the engine's memory"
            );
        }

        let tuning_defaults = crate::analytics_tuning::from_config(&config);
        let tuning_overrides = crate::analytics_tuning::load(&store).await?;
        let (tuning_tx, tuning_rx) = tokio::sync::watch::channel(tuning_defaults.with(&tuning_overrides));
        let worker_config = analytics_worker_config(&config, *tuning_rx.borrow()).context("analytics settings")?;
        let analytics_enabled = worker_config.is_some();
        let analytics_client =
            Arc::new(crate::analytics_worker::WorkerClient::new(analytics_enabled, tuning_rx.clone(), clock.clone()));
        let logs = Arc::new(crate::logs::Logs::new(crate::logs::LogsDeps {
            keys: Arc::new(store.clone()),
            apps: Arc::new(store.clone()),
            servers: Arc::new(store.clone()),
            sink: analytics_client.clone(),
            clock: clock.clone(),
        }));
        let pins = Arc::new(crate::pins::PinService::new(crate::pins::PinDeps {
            pins: Arc::new(store.clone()),
            projects: Arc::new(store.clone()),
            services: Arc::new(store.clone()),
            tenancy: Arc::new(store.clone()),
            servers: Arc::new(store.clone()),
            clusters: Arc::new(store.clone()),
            clock: clock.clone(),
        }));
        let container_telemetry =
            Arc::new(crate::container_telemetry::ContainerTelemetry::new(crate::container_telemetry::TelemetryDeps {
                settings: Arc::new(store.clone()),
                clock: clock.clone(),
                analytics_enabled,
            }));
        let analytics_maps = Arc::new(crate::analytics_worker::MapsPublisher::new(crate::analytics_worker::MapsDeps {
            servers: Arc::new(store.clone()),
            services: Arc::new(store.clone()),
            keys: Arc::new(store.clone()),
        }));

        // Tunnel sockets live in memory: rows still open are from before this
        // start and are closed as such (D-100).
        match forge_core::tunnels::TunnelRepository::close_open_tunnels(&store, clock.now(), "server_restart").await {
            Ok(0) => {}
            Ok(n) => tracing::info!(count = n, "tunnels open at the last stop are recorded as closed"),
            Err(e) => tracing::warn!(error = %e, "could not close stale tunnel rows"),
        }

        let public_address =
            crate::public_address::load(&store, config.public_url.as_deref().unwrap_or_default()).await?;
        let allowed_hosts = crate::allowed_hosts::load(&store).await?;

        // Authorization: load every persisted role binding as a template link.
        let policy = Arc::new(PolicyEngine::with_clock(clock.clone())?);

        for b in store.list_bindings().await? {
            policy.link_binding(&b)?;
        }

        // Access profiles (D-086): built-ins equal to their roles, then every
        // active profile's latest revision and every unexpired assignment.
        crate::access::load(&store, &policy, clock.now()).await?;
        let authz: Arc<dyn Authorizer> = Arc::new(CedarAuthorizer::new(policy.clone(), Arc::new(store.clone())));

        let audit = Arc::new(AuditLog::new(
            Arc::new(store.clone()),
            Arc::new(AuditSigner::load_or_create(&data.join("keys/audit.key"))?),
            clock.clone(),
        ));

        let ca = Arc::new(CertificateAuthority::load_or_create(&data.join("pki"), clock.now())?);
        let envelope = Envelope::new(Arc::new(LocalKeyProvider::load_or_create(&data.join("keys/master.key"))?));
        let signer = Arc::new(CommandSigner::load_or_create(&data.join("keys/command.key"))?);

        // The control-plane key agents pin (D-167): made by `init` or on the
        // first start after an upgrade.
        let (agent_keys, keys_created) = crate::agent_keys::AgentKeys::load_or_create(
            crate::agent_keys::AgentKeysDeps {
                settings: Arc::new(store.clone()),
                envelope: envelope.clone(),
                ca: ca.clone(),
                signer: signer.clone(),
                hostnames: config.agent_hostnames.clone(),
                policy: config.agent_tls,
                online_window: Duration::from_secs(u64::from(config.offline_after_secs.max(30))),
            },
            clock.now(),
        )
        .await?;
        let agent_keys = Arc::new(agent_keys);

        if keys_created {
            let ev = forge_core::model::AuditEvent::new(
                "system:forge",
                crate::agent_keys::AUDIT_KEY_CREATE,
                forge_core::EntityRef::platform(),
                forge_core::model::AuditOutcome::Success,
            )
            .with_details(serde_json::json!({ "keys": agent_keys.keys() }));
            audit.append(ev).await?;
        }

        let mut worker_config = worker_config;
        if let Some(w) = &mut worker_config {
            attach_agent_listener(w, &config, &ca, &agent_keys);
        }

        let analytics_worker =
            Arc::new(crate::analytics_worker::WorkerSupervisor::new(crate::analytics_worker::SupervisorDeps {
                config: worker_config,
                client: analytics_client.clone(),
                maps: analytics_maps.clone(),
                audit: audit.clone(),
                clock: clock.clone(),
                tuning: tuning_rx,
            }));
        let analytics_control =
            Arc::new(crate::analytics_control::AnalyticsControl::new(crate::analytics_control::ControlDeps {
                engine: if analytics_enabled { config.analytics.engine } else { crate::config::AnalyticsEngine::Off },
                defaults: tuning_defaults,
                overrides: tuning_overrides,
                tx: tuning_tx,
                settings: Arc::new(store.clone()),
                clock: clock.clone(),
                worker: analytics_client.clone(),
                supervisor: analytics_worker.clone(),
            }));

        let identity_config = IdentityConfig {
            oidc_auto_provision: config.oidc.as_ref().is_some_and(|o| o.auto_provision),
            refresh_token_ttl: Duration::from_secs(u64::from(config.security.refresh_token_days) * 24 * 3600),
            ..IdentityConfig::default()
        };
        let identity = IdentityService::new(
            Arc::new(store.clone()),
            Arc::new(store.clone()),
            envelope.clone(),
            clock.clone(),
            identity_config,
        )
        .with_connectors(Arc::new(store.clone()), config.mcp.enabled);
        let passkeys = forge_identity::passkeys::PasskeyService::new(
            Arc::new(store.clone()),
            Arc::new(store.clone()),
            clock.clone(),
        );

        let oidc = match &config.oidc {
            Some(o) => {
                // The redirect URL must be stable: OIDC needs a pinned address.
                let pinned = public_address.pinned().ok_or_else(|| {
                    anyhow::anyhow!(
                        "[oidc] needs a pinned console address: set public_url in forge.toml or run `forge-server public-address set <url>`"
                    )
                })?;
                Some(Self::oidc_provider(&pinned, o).await?)
            }
            None => None,
        };

        let ssh_ca = Arc::new(forge_ssh::SshUserCa::load_or_create(&data.join("keys/ssh_user_ca"))?);
        let hub = Arc::new(CommandQueue::new());

        let dist = Arc::new(AgentDist::scan(&config.agent_dist_dir()));
        let mut registry = JobRegistry::new();
        registry.register(Arc::new(AgentCommandJob {
            hub: hub.clone(),
            signer: signer.clone(),
            servers: Arc::new(store.clone()),
            certificates: Arc::new(store.clone()),
            envelope: envelope.clone(),
            clock: clock.clone(),
            sites: Arc::new(store.clone()),
            authz: authz.clone(),
            ttl: Duration::from_secs(120),
        }));
        registry.register(Arc::new(DiagnosticsJob));
        let rollout_engine = Arc::new(std::sync::OnceLock::new());
        registry.register(Arc::new(crate::jobs::agent_rollout::AgentRolloutJob {
            engine: rollout_engine.clone(),
            jobs: Arc::new(store.clone()),
            poll: Duration::from_millis(config.jobs.poll_ms.max(200) * 4),
            dist: dist.clone(),
            clock: clock.clone(),
            servers: Arc::new(store.clone()),
            batch_deadline: config.update.agent_batch_timeout(),
        }));
        registry.register(Arc::new(crate::jobs::agent_update::AgentUpdateJob {
            hub: hub.clone(),
            signer: signer.clone(),
            servers: Arc::new(store.clone()),
            dist: dist.clone(),
            clock: clock.clone(),
            download: crate::jobs::agent_update::DownloadBase {
                node_public_url: config.node_public_url.clone(),
                address: public_address.clone(),
            },
            come_back: Duration::from_secs(6 * 60),
            jobs: Arc::new(store.clone()),
            facts: Arc::new(store.clone()),
        }));
        let app_job = Arc::new(crate::jobs::AppJob {
            hub: hub.clone(),
            signer: signer.clone(),
            servers: Arc::new(store.clone()),
            apps: Arc::new(store.clone()),
            settings: Arc::new(store.clone()),
            installations: Arc::new(store.clone()),
            files: Arc::new(store.clone()),
            vault: Arc::new(store.clone()),
            stores: Arc::new(store.clone()),
            audit: audit.clone(),
            envelope: envelope.clone(),
            clock: clock.clone(),
            deploys: Arc::new(store.clone()),
        });
        registry.register(app_job.clone());
        let registries = Arc::new(crate::registries::RegistryService {
            repo: Arc::new(store.clone()),
            envelope: envelope.clone(),
            clock: clock.clone(),
        });
        let node_rpc = crate::node_rpc::NodeRpc { hub: hub.clone(), signer: signer.clone(), clock: clock.clone() };
        let endpoints = Arc::new(crate::endpoints::EndpointService { files: Arc::new(store.clone()) });
        let docker_resources = Arc::new(crate::docker_resources::DockerResources {
            rpc: node_rpc.clone(),
            servers: Arc::new(store.clone()),
        });
        let webhooks = Arc::new(crate::webhooks::WebhookService {
            repo: Arc::new(store.clone()),
            jobs: Arc::new(store.clone()),
            services: Arc::new(store.clone()),
            envelope: envelope.clone(),
            clock: clock.clone(),
            limits: crate::webhooks::Limits::default(),
            per_hook: crate::webhooks::Windows::default(),
            per_ip: crate::webhooks::Windows::default(),
        });
        let swarm = Arc::new(crate::swarm::SwarmService {
            swarms: Arc::new(store.clone()),
            servers: Arc::new(store.clone()),
            clusters: Arc::new(store.clone()),
            tenancy: Arc::new(store.clone()),
            files: Arc::new(store.clone()),
            deploys: Arc::new(store.clone()),
            services: Arc::new(store.clone()),
            rpc: node_rpc.clone(),
            envelope: envelope.clone(),
            clock: clock.clone(),
            node_cache: std::sync::Mutex::default(),
        });
        let node_ops = crate::jobs::node_op::NodeOps {
            hub: hub.clone(),
            signer: signer.clone(),
            servers: Arc::new(store.clone()),
            clock: clock.clone(),
        };
        let deploy = Arc::new(crate::deploy::DeployService {
            apps: Arc::new(store.clone()),
            tenancy: Arc::new(store.clone()),
            servers: Arc::new(store.clone()),
            tags: Arc::new(store.clone()),
            sites: Arc::new(store.clone()),
            releases: Arc::new(store.clone()),
            deploys: Arc::new(store.clone()),
            swarm: swarm.clone(),
            clock: clock.clone(),
        });
        let rollout_effects: Arc<std::sync::OnceLock<Arc<dyn crate::deploy::RolloutEffects>>> =
            Arc::new(std::sync::OnceLock::new());
        registry.register(Arc::new(crate::jobs::deploy::DeployJob {
            apps: Arc::new(store.clone()),
            releases: Arc::new(store.clone()),
            deploys: Arc::new(store.clone()),
            runner: app_job,
            service: deploy.clone(),
            effects: rollout_effects.clone(),
            engine: rollout_engine.clone(),
            audit: audit.clone(),
            clock: clock.clone(),
            tick: Duration::from_millis(config.jobs.poll_ms.clamp(200, 3_000) * 5),
            swarm: swarm.clone(),
            swarms: Arc::new(store.clone()),
            ops: node_ops.clone(),
            registries: registries.clone(),
        }));
        let docker = Arc::new(crate::docker_engine::DockerEngines::new(
            Arc::new(store.clone()),
            Arc::new(store.clone()),
            envelope.clone(),
            clock.clone(),
        ));
        registry.register(Arc::new(crate::jobs::swarm_service::SwarmServiceJob {
            ops: node_ops.clone(),
            swarm: swarm.clone(),
            swarms: Arc::new(store.clone()),
            services: Arc::new(store.clone()),
            deploys: Arc::new(store.clone()),
            envelope: envelope.clone(),
            registries: registries.clone(),
        }));
        registry.register(Arc::new(crate::jobs::docker_op::DockerOpJob {
            ops: node_ops.clone(),
            registries: registries.clone(),
        }));
        registry.register(Arc::new(crate::jobs::swarm::SwarmMemberJob {
            ops: node_ops.clone(),
            swarms: Arc::new(store.clone()),
            envelope: envelope.clone(),
        }));

        for enable in [true, false] {
            let link = crate::jobs::docker_endpoint::NodeLink { ops: node_ops.clone(), engines: docker.clone() };

            if enable {
                registry.register(Arc::new(crate::jobs::docker_endpoint::DockerEndpointEnableJob { link }));
            } else {
                registry.register(Arc::new(crate::jobs::docker_endpoint::DockerEndpointDisableJob { link }));
            }
        }

        registry.register(Arc::new(crate::jobs::StackJob {
            hub: hub.clone(),
            signer: signer.clone(),
            servers: Arc::new(store.clone()),
            clusters: Arc::new(store.clone()),
            fabrics: Arc::new(store.clone()),
            files: Arc::new(store.clone()),
            tenancy: Arc::new(store.clone()),
            stacks: Arc::new(store.clone()),
            apps: Arc::new(store.clone()),
            envelope: envelope.clone(),
            clock: clock.clone(),
        }));
        registry.register(Arc::new(crate::jobs::PackageJob {
            hub: hub.clone(),
            signer: signer.clone(),
            servers: Arc::new(store.clone()),
            installations: Arc::new(store.clone()),
            clock: clock.clone(),
        }));
        registry.register(Arc::new(crate::jobs::StorageJob {
            hub: hub.clone(),
            signer: signer.clone(),
            servers: Arc::new(store.clone()),
            stores: Arc::new(store.clone()),
            vault: Arc::new(store.clone()),
            installations: Arc::new(store.clone()),
            audit: audit.clone(),
            envelope: envelope.clone(),
            clock: clock.clone(),
        }));
        let ssh: Arc<dyn forge_core::remote::RemoteConnector> = Arc::new(forge_ssh::SshConnector::default());
        registry.register(Arc::new(NodeBootstrapJob {
            connector: ssh.clone(),
            secrets: Arc::new(store.clone()),
            servers: Arc::new(store.clone()),
            enrollment: Arc::new(store.clone()),
            clusters: Arc::new(store.clone()),
            ssh_endpoints: Arc::new(store.clone()),
            logins: Arc::new(store.clone()),
            audit: audit.clone(),
            envelope: envelope.clone(),
            dist: dist.clone(),
            ca_fingerprint: ca.fingerprint().to_owned(),
            agent_keys: agent_keys.clone(),
            ssh_user_ca: ssh_ca.public_openssh(),
            node_forge_url: config.node_forge_url().to_owned(),
            node_agent_url: config.node_agent_url().to_owned(),
            clock: clock.clone(),
            online_timeout: Duration::from_secs(120),
        }));
        let jobs = Arc::new(JobEngine::new(
            Arc::new(store.clone()),
            authz.clone(),
            audit.clone(),
            registry,
            clock.clone(),
            EngineConfig {
                lease: Duration::from_secs(config.jobs.lease_secs),
                concurrency: config.jobs.concurrency,
                poll_interval: Duration::from_millis(config.jobs.poll_ms),
                retry: Backoff::new(Duration::from_secs(1), Duration::from_secs(300)),
                ..EngineConfig::default()
            },
        ));
        let _ = rollout_engine.set(jobs.clone());

        let mut trust = TrustStore::new();

        for p in &config.trusted_publishers {
            trust.trust(&p.publisher, &p.public_key).with_context(|| format!("trusted publisher {}", p.publisher))?;
        }

        let plugins = Arc::new(PluginHost::new(
            Arc::new(store.clone()),
            authz.clone(),
            audit.clone(),
            Arc::new(ProcessRunner::new(config.plugin_runner_path())),
            ForgeHostFunctions::new(Arc::new(store.clone()), authz.clone()),
            trust,
            clock.clone(),
            PluginHostConfig {
                artifacts_dir: data.join("plugins"),
                api_version: forge_sdk::PLUGIN_API_VERSION.into(),
                fault_threshold: 3,
            },
        ));

        let sftp_host_key = if config.sftp.enabled {
            Some(Arc::new(
                forge_file_gateway::HostKey::load_or_create(&data.join("keys/sftp_host_ed25519"))
                    .map_err(anyhow::Error::msg)?,
            ))
        } else {
            None
        };

        let files = forge_files::FileService::new(
            Arc::new(store.clone()),
            Arc::new(store.clone()),
            authz.clone(),
            audit.clone(),
            Arc::new(crate::file_transport::HubFileTransport {
                signer: signer.clone(),
                hub: hub.clone(),
                clock: clock.clone(),
            }),
            clock.clone(),
        );
        let files = Arc::new(if config.sftp.enabled {
            files
                .with_grants(Arc::new(store.clone()), config.sftp.max_hours)
                .with_never_expiry(config.sftp.allow_never_expiry)
        } else {
            files
        });

        let bucket_grants = crate::bucket_grants::BucketGrants::new(
            Arc::new(store.clone()),
            Arc::new(store.clone()),
            Arc::new(store.clone()),
            clock.clone(),
        );

        let health_secrets = crate::health_secrets::HealthSecrets::new(crate::health_secrets::Ports {
            apps: Arc::new(store.clone()),
            authority: Arc::new(store.clone()),
            vault: Arc::new(store.clone()),
            users: Arc::new(store.clone()),
            bindings: Arc::new(store.clone()),
            authz: authz.clone(),
            audit: audit.clone(),
            envelope: envelope.clone(),
            clock: clock.clone(),
        });
        let health = forge_health::ObservationRecorder::new(
            Arc::new(store.clone()),
            Arc::new(store.clone()),
            clock.clone(),
            forge_health::RecorderConfig::default(),
        );
        let notifier = forge_alerts::notify::Notifier {
            store: Arc::new(store.clone()),
            alerts: Arc::new(store.clone()),
            transport: Arc::new(
                crate::notifying::HttpTransport::new(crate::notifying::target_rules(&config.alerts))
                    .context("creating the notification HTTP client")?,
            ),
            mail: Arc::new(crate::notify_mail::SmtpMailer::new(
                crate::notifying::target_rules(&config.alerts),
                &public_address.get(),
            )),
            secrets: Arc::new(crate::notifying::VaultSecrets {
                vault: Arc::new(store.clone()),
                envelope: envelope.clone(),
            }),
            names: Arc::new(crate::notifying::StoreNames {
                servers: Arc::new(store.clone()),
                apps: Arc::new(store.clone()),
            }),
            clock: clock.clone(),
            config: crate::notifying::notify_config(&config.alerts, public_address.clone()),
            pauses: forge_alerts::notify::ChannelPauses::default(),
            silences: forge_alerts::silence::Silences {
                store: Arc::new(store.clone()),
                alerts: Arc::new(store.clone()),
                resolver: Arc::new(store.clone()),
                clock: clock.clone(),
            },
            people: forge_alerts::notify::people::People {
                users: Arc::new(store.clone()),
                bindings: Arc::new(store.clone()),
                authorizer: authz.clone(),
                resolver: Arc::new(store.clone()),
            },
        };
        let (events, _) = broadcast::channel(1024);
        let agent_updates = Arc::new(crate::agent_update_overview::AgentUpdateOverview::new(
            Arc::new(store.clone()),
            Arc::new(store.clone()),
            dist.clone(),
        ));
        let state = Arc::new(Self {
            config,
            store,
            policy,
            authz,
            audit,
            identity,
            passkeys,
            oidc,
            oidc_pending: PendingLogins::default(),
            consents: crate::api::connectors::ConsentRequests::default(),
            device_requests: crate::api::device::DeviceRequests::default(),
            mcp_limits: crate::api::mcp::McpLimits::default(),
            remote_sessions: crate::api::remote_sessions::RemoteSessions::default(),
            access_lock: tokio::sync::Mutex::new(()),
            signed_out: broadcast::channel(64).0,
            tunnels: crate::tunnels::Tunnels::default(),
            docker,
            deploy,
            exec_streams: crate::container_exec::ExecStreams::default(),
            jobs,
            plugins,
            ca,
            agent_keys,
            signer,
            hub,
            clock,
            events,
            login_throttle: LoginThrottle::default(),
            login_challenges: crate::challenge::LoginChallenges::default(),
            captchas: crate::captcha::Captchas::default(),
            envelope,
            ssh,
            ssh_ca,
            agent_updates,
            dist,
            telemetry: crate::metrics::Telemetry::default(),
            analytics: analytics_client.clone(),
            telemetry_sink: analytics_client.clone(),
            analytics_enabled,
            analytics_client,
            analytics_worker,
            analytics_maps,
            logs,
            pins,
            container_telemetry,
            analytics_control,
            health,
            files,
            swarm,
            docker_resources,
            endpoints,
            registries,
            webhooks,
            access_lists,
            limiter: crate::net_access::Limiter::default(),
            downloads: Arc::new(tokio::sync::Semaphore::new(16)),
            enroll_attempts: crate::rate_limit::Windows::default(),
            bucket_grants,
            health_secrets,
            notifier,
            sftp_host_key,
            sftp_bound: std::sync::OnceLock::new(),
            public_address,
            allowed_hosts,
        });

        // The rollout job reaches endpoints and health through the API (#73).
        let _ = rollout_effects.set(crate::api::deployments::effects(&state));

        Ok(state)
    }

    async fn oidc_provider(
        public_address: &str,
        o: &crate::config::OidcSettings,
    ) -> anyhow::Result<Arc<dyn OidcProvider>> {
        let secret = std::env::var(&o.client_secret_env)
            .with_context(|| format!("OIDC client secret env var {} is not set", o.client_secret_env))?;
        let provider = OpenIdConnectProvider::discover(OidcConfig {
            issuer_url: o.issuer_url.clone(),
            client_id: o.client_id.clone(),
            client_secret: secret,
            redirect_url: format!("{public_address}/api/v1/auth/oidc/callback"),
            scopes: vec!["openid".into(), "email".into(), "profile".into()],
        })
        .await?;

        Ok(Arc::new(provider))
    }

    /// Persists a role binding and links its Cedar template.
    pub async fn grant(&self, binding: &RoleBinding) -> DomainResult<()> {
        self.store.create_binding(binding).await?;
        self.policy.link_binding(binding).map_err(forge_core::DomainError::internal)
    }

    /// Deletes a role binding and unlinks it.
    pub async fn revoke(&self, id: RoleBindingId) -> DomainResult<()> {
        self.store.delete_binding(id).await?;
        self.policy.unlink_binding(&id.to_string()).map_err(forge_core::DomainError::internal)
    }

    /// Audits something Forge did on its own (no user caller), e.g. an update.
    pub async fn audit_system(&self, action: &str, details: serde_json::Value) {
        self.audit_system_outcome(action, forge_core::model::AuditOutcome::Success, details).await;
    }

    /// [`Services::audit_system`] with an explicit outcome (failures).
    pub async fn audit_system_outcome(
        &self,
        action: &str,
        outcome: forge_core::model::AuditOutcome,
        details: serde_json::Value,
    ) {
        let ev = forge_core::model::AuditEvent::new("system:forge", action, forge_core::EntityRef::platform(), outcome)
            .with_details(details);

        if let Err(e) = self.audit.append(ev).await {
            tracing::error!(error = %e, "audit write failed");
        }
    }

    pub fn publish(&self, event: ServerEvent) {
        let _ = self.events.send(event);
    }
}

impl Services {
    /// Resolves a resource's ancestry; fails if any part does not exist.
    pub async fn authz_resolve(&self, entity: &forge_core::EntityRef) -> DomainResult<forge_core::ResourceRef> {
        use forge_core::ports::ResourceResolver;
        self.store.resolve(entity).await
    }
}

/// Gives the worker its agent upload listener (D-142): the bind address,
/// the CA, and certificates for the control-plane key agents pin (D-167),
/// issued at each worker start and renewed while it runs (D-145) or when
/// the key switches, handed over over stdin and the control socket.
fn attach_agent_listener(
    w: &mut crate::analytics_worker::SupervisorConfig,
    config: &ServerConfig,
    ca: &Arc<CertificateAuthority>,
    keys: &Arc<crate::agent_keys::AgentKeys>,
) {
    w.config.agent_listener = Some(forge_analytics_proto::config::AgentListener {
        listen: config.analytics.worker_agent_listen,
        ca_pem: ca.cert_pem().to_owned(),
        state_dir: config.data_dir.join("analytics-worker").join("spool-ids"),
        max_uploads: config.analytics.max_uploads(),
    });
    w.listener_certs = Some(Arc::new(crate::agent_keys::WorkerListenerCerts(keys.clone())));
}

/// Worker memory beyond DuckDB's own limit (its buffer manager does not
/// count the worker's buffers, connections and import work).
const DUCKDB_HEADROOM_MB: u64 = 256;

/// How the analytics worker runs for `config`, or `None` when analytics is
/// off (D-141). Validates the engine settings, so a bad `[analytics]`
/// section stops start-up.
fn analytics_worker_config(
    config: &ServerConfig,
    tuning: crate::analytics_tuning::Tuning,
) -> anyhow::Result<Option<crate::analytics_worker::SupervisorConfig>> {
    use crate::config::AnalyticsEngine;
    use forge_analytics_proto::config::{Endpoint, StoreSelection, WorkerConfig};

    if config.analytics.removed_memory_limit_mb.is_some() {
        tracing::warn!(
            "[analytics] memory_limit_mb is no longer used and is ignored; embedded_memory_mb caps the engine's memory"
        );
    }

    let store = match config.analytics.engine {
        AnalyticsEngine::Off => return Ok(None),
        AnalyticsEngine::Postgres => {
            tracing::warn!("the PostgreSQL analytics engine is not available in this build; analytics is off");
            return Ok(None);
        }
        AnalyticsEngine::Clickhouse => {
            let base =
                config.analytics.url.as_deref().context("[analytics] url is required for engine = \"clickhouse\"")?;
            StoreSelection::Clickhouse { url: crate::config::compose_url(base, &config.analytics_name(), None)? }
        }
        AnalyticsEngine::Embedded => {
            if cfg!(windows) {
                anyhow::bail!(
                    "the embedded analytics engine runs on Linux only; use engine = \"clickhouse\" or \"off\""
                );
            }

            StoreSelection::Embedded {
                paths: Box::new(crate::analytics_install::paths(config)),
                port: config.analytics.embedded_port,
                database: config.analytics_name(),
            }
        }
        AnalyticsEngine::Duckdb => {
            let need = u64::from(config.analytics.duckdb_memory_mb) + DUCKDB_HEADROOM_MB;
            let limit = u64::from(config.analytics.worker_memory_limit_mb);
            if limit > 0 && limit < need {
                anyhow::bail!(
                    "[analytics] worker_memory_limit_mb ({limit}) must be at least duckdb_memory_mb plus {DUCKDB_HEADROOM_MB} MiB ({need}): DuckDB runs inside the worker"
                );
            }

            StoreSelection::Duckdb { dir: config.data_dir.join("analytics").join("duckdb") }
        }
        AnalyticsEngine::Memory => StoreSelection::Memory,
    };

    let dir = config.data_dir.join("analytics-worker");
    let endpoint = if cfg!(unix) { Endpoint::Unix { path: dir.join("control.sock") } } else { Endpoint::Loopback };
    let limit = config.analytics.worker_memory_limit_mb;

    Ok(Some(crate::analytics_worker::SupervisorConfig {
        binary: config.analytics_worker_path(),
        dir,
        config: WorkerConfig { store, endpoint, tuning, agent_listener: None },
        listener_certs: None,
        password: config.analytics.password.clone(),
        memory_limit_bytes: (limit > 0).then(|| u64::from(limit) * 1024 * 1024),
    }))
}
