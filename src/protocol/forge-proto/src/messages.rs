//! Wire messages. Kept in sync with `proto/forge_agent_v1.proto`.

#[derive(Clone, PartialEq, prost::Message)]
pub struct EnrollRequest {
    #[prost(string, tag = "1")]
    pub token_secret: String,
    #[prost(string, tag = "2")]
    pub csr_pem: String,
    #[prost(string, tag = "3")]
    pub hostname: String,
    #[prost(string, tag = "4")]
    pub os: String,
    #[prost(string, tag = "5")]
    pub arch: String,
    #[prost(string, tag = "6")]
    pub agent_version: String,
    /// Terminal account the installer created on this node (D-078); empty
    /// when none. Forge saves it to the node's login list.
    #[prost(string, tag = "7")]
    pub terminal_account: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct EnrollResponse {
    #[prost(string, tag = "1")]
    pub server_id: String,
    #[prost(string, tag = "2")]
    pub cert_pem: String,
    #[prost(string, tag = "3")]
    pub ca_pem: String,
    #[prost(bytes = "vec", tag = "4")]
    pub command_public_key: Vec<u8>,
    #[prost(uint32, tag = "5")]
    pub heartbeat_interval_secs: u32,
    /// Where the node uploads its spool files (D-142): the analytics
    /// worker's agent listener. Empty when analytics is off.
    #[prost(string, tag = "6")]
    pub analytics_url: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct Heartbeat {
    #[prost(string, tag = "1")]
    pub agent_version: String,
    #[prost(uint64, tag = "2")]
    pub uptime_secs: u64,
    #[prost(uint64, tag = "3")]
    pub rss_bytes: u64,
    #[prost(message, repeated, tag = "4")]
    pub results: Vec<CommandResult>,
    /// Resource sample taken just before this heartbeat (absent on old agents).
    #[prost(message, optional, tag = "5")]
    pub metrics: Option<NodeMetrics>,
    /// Live output of running commands since the last heartbeat.
    #[prost(message, repeated, tag = "6")]
    pub progress: Vec<CommandProgress>,
    /// A command is streaming output: the server answers quickly.
    #[prost(bool, tag = "7")]
    pub streaming: bool,
    /// Health check results since the last heartbeat: changes, plus a
    /// keep-alive per check at half its freshness (D-085). At most
    /// [`MAX_HEALTH_REPORTS`]; the rest waits for the next heartbeat.
    #[prost(message, repeated, tag = "8")]
    pub health: Vec<HealthReport>,
    /// Lines the node collected from its Docker containers since the last
    /// acknowledged batch (absent on old agents and when collection is off).
    #[prost(message, optional, tag = "9")]
    pub container_logs: Option<ContainerLogBatch>,
    /// The node's spool (D-142). Present from agents that write node
    /// history, container output and samples to their spool instead of
    /// this heartbeat; the server then forwards nothing to the worker.
    #[prost(message, optional, tag = "10")]
    pub spool: Option<SpoolStatus>,
}

/// Most container lines in one heartbeat.
pub const MAX_CONTAINER_LOG_LINES: usize = 2_000;
/// Longest container line kept (bytes); longer ones are cut.
pub const MAX_CONTAINER_LINE_BYTES: usize = 16 * 1024;
/// Most container samples in one heartbeat.
pub const MAX_CONTAINER_SAMPLES: usize = 256;
/// Most exclude entries in [`ContainerTelemetryConfig`].
pub const MAX_CONTAINER_EXCLUDES: usize = 64;
/// Longest exclude entry or label value (bytes).
pub const MAX_CONTAINER_FIELD_BYTES: usize = 256;

/// Output stream of a container line.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, prost::Enumeration)]
#[repr(i32)]
pub enum ContainerStream {
    Unspecified = 0,
    Stdout = 1,
    Stderr = 2,
}

/// The labels Forge uses to attribute a container.
#[derive(Clone, PartialEq, Eq, prost::Message)]
pub struct ContainerLabels {
    /// `forge.app`: `<environment>/<service>`.
    #[prost(string, tag = "1")]
    pub forge_app: String,
    /// `forge.stack`: `<environment>/<stack>`.
    #[prost(string, tag = "2")]
    pub forge_stack: String,
    /// `com.docker.compose.project`.
    #[prost(string, tag = "3")]
    pub compose_project: String,
    /// `com.docker.compose.service`.
    #[prost(string, tag = "4")]
    pub compose_service: String,
}

/// One line a container wrote.
#[derive(Clone, PartialEq, Eq, prost::Message)]
pub struct ContainerLogLine {
    /// Short (12 hex digits) container id.
    #[prost(string, tag = "1")]
    pub container_id: String,
    #[prost(string, tag = "2")]
    pub container_name: String,
    #[prost(string, tag = "3")]
    pub image: String,
    #[prost(message, optional, tag = "4")]
    pub labels: Option<ContainerLabels>,
    #[prost(enumeration = "ContainerStream", tag = "5")]
    pub stream: i32,
    /// Docker's timestamp of the line, epoch milliseconds.
    #[prost(int64, tag = "6")]
    pub ts_ms: i64,
    /// The line without its newline, at most [`MAX_CONTAINER_LINE_BYTES`].
    #[prost(string, tag = "7")]
    pub line: String,
}

/// Container lines collected on a node. The agent re-sends a batch until a
/// heartbeat carrying it succeeds, so the server may see one twice.
#[derive(Clone, PartialEq, Eq, prost::Message)]
pub struct ContainerLogBatch {
    /// Batch number, increasing per helper run.
    #[prost(uint64, tag = "1")]
    pub seq: u64,
    /// At most [`MAX_CONTAINER_LOG_LINES`].
    #[prost(message, repeated, tag = "2")]
    pub lines: Vec<ContainerLogLine>,
    /// Lines dropped on the node since the last batch because the buffer
    /// was full.
    #[prost(uint64, tag = "3")]
    pub dropped: u64,
}

/// One container's resources, sampled once a minute.
#[derive(Clone, PartialEq, prost::Message)]
pub struct ContainerSample {
    #[prost(string, tag = "1")]
    pub container_id: String,
    #[prost(string, tag = "2")]
    pub container_name: String,
    #[prost(string, tag = "3")]
    pub image: String,
    #[prost(message, optional, tag = "4")]
    pub labels: Option<ContainerLabels>,
    /// CPU percent of one core (may exceed 100 on several cores).
    #[prost(float, tag = "5")]
    pub cpu_percent: f32,
    #[prost(uint64, tag = "6")]
    pub mem_used_bytes: u64,
    #[prost(uint64, tag = "7")]
    pub mem_limit_bytes: u64,
    #[prost(uint64, tag = "8")]
    pub net_rx_bytes_per_sec: u64,
    #[prost(uint64, tag = "9")]
    pub net_tx_bytes_per_sec: u64,
    #[prost(uint64, tag = "10")]
    pub block_read_bytes_per_sec: u64,
    #[prost(uint64, tag = "11")]
    pub block_write_bytes_per_sec: u64,
    #[prost(uint32, tag = "12")]
    pub restarts: u32,
}

/// What a node collects from its Docker containers (node settings).
#[derive(Clone, PartialEq, Eq, prost::Message)]
pub struct ContainerTelemetryConfig {
    #[prost(bool, tag = "1")]
    pub enabled: bool,
    /// Containers left out: a name, or `key=value` for a label. At most
    /// [`MAX_CONTAINER_EXCLUDES`].
    #[prost(string, repeated, tag = "2")]
    pub exclude: Vec<String>,
}

/// Most health reports in one heartbeat.
pub const MAX_HEALTH_REPORTS: usize = 256;

/// One health check result of a service instance on this node. The
/// server maps `(environment, app)` to the service placed on the node and
/// drops anything else.
#[derive(Clone, PartialEq, Eq, prost::Message)]
pub struct HealthReport {
    /// Environment slug.
    #[prost(string, tag = "1")]
    pub environment: String,
    /// Service slug.
    #[prost(string, tag = "2")]
    pub app: String,
    /// `startup`, `readiness` or `liveness`.
    #[prost(string, tag = "3")]
    pub purpose: String,
    /// The check's name; `default` for Forge's default check.
    #[prost(string, tag = "4")]
    pub check: String,
    /// `passing`, `failing`, `unknown`, `pending` or `disabled`; for startup
    /// checks `waiting`, `succeeded` or `exhausted`.
    #[prost(string, tag = "5")]
    pub state: String,
    /// Why it isn't passing (`app_failure`, `timeout`, `ambiguous_port`, …).
    #[prost(string, tag = "6")]
    pub reason: String,
    /// Sanitized detail, at most 256 bytes.
    #[prost(string, tag = "7")]
    pub detail: String,
    /// Workload generation: systemd InvocationID or container id.
    #[prost(string, tag = "8")]
    pub generation: String,
    /// Agent clock, epoch milliseconds.
    #[prost(int64, tag = "9")]
    pub observed_at_ms: i64,
    #[prost(uint64, tag = "10")]
    pub fresh_for_ms: u64,
    /// The health policy revision the check ran under (0 = defaults only).
    #[prost(int64, tag = "11")]
    pub policy_revision: i64,
}

/// A chunk of a running command's output (whole lines).
#[derive(Clone, PartialEq, Eq, prost::Message)]
pub struct CommandProgress {
    #[prost(string, tag = "1")]
    pub operation_id: String,
    #[prost(string, tag = "2")]
    pub text: String,
}

/// Host resource usage at one point in time. Rates are averaged since the
/// previous sample.
#[derive(Clone, PartialEq, prost::Message)]
pub struct NodeMetrics {
    /// Whole-machine CPU busy, 0–100.
    #[prost(float, tag = "1")]
    pub cpu_percent: f32,
    #[prost(uint64, tag = "2")]
    pub mem_total_bytes: u64,
    #[prost(uint64, tag = "3")]
    pub mem_used_bytes: u64,
    #[prost(uint64, tag = "4")]
    pub swap_total_bytes: u64,
    #[prost(uint64, tag = "5")]
    pub swap_used_bytes: u64,
    /// Sum over local disks.
    #[prost(uint64, tag = "6")]
    pub disk_total_bytes: u64,
    #[prost(uint64, tag = "7")]
    pub disk_used_bytes: u64,
    #[prost(uint64, tag = "8")]
    pub net_rx_bytes_per_sec: u64,
    #[prost(uint64, tag = "9")]
    pub net_tx_bytes_per_sec: u64,
    #[prost(float, tag = "10")]
    pub load1: f32,
    #[prost(uint64, tag = "11")]
    pub host_uptime_secs: u64,
    #[prost(uint32, tag = "12")]
    pub cpu_count: u32,
    /// Every mounted local volume.
    #[prost(message, repeated, tag = "13")]
    pub volumes: Vec<DiskVolume>,
    /// Edge nodes: last minute of proxied traffic per endpoint.
    #[prost(message, repeated, tag = "14")]
    pub traffic: Vec<SiteTraffic>,
    /// At most once a minute: the top processes by CPU and by memory (the
    /// union, up to 20). Empty in the heartbeats in between.
    #[prost(message, repeated, tag = "15")]
    pub top_processes: Vec<TopProcess>,
    /// At most once a minute: every running Docker container, up to
    /// [`MAX_CONTAINER_SAMPLES`]. Empty in the heartbeats in between.
    #[prost(message, repeated, tag = "16")]
    pub containers: Vec<ContainerSample>,
}

/// One process in a node's per-minute top list.
#[derive(Clone, PartialEq, prost::Message)]
pub struct TopProcess {
    #[prost(uint32, tag = "1")]
    pub pid: u32,
    #[prost(string, tag = "2")]
    pub name: String,
    #[prost(string, tag = "3")]
    pub user: String,
    /// CPU percent of one core (may exceed 100 on several cores).
    #[prost(float, tag = "4")]
    pub cpu_percent: f32,
    #[prost(uint64, tag = "5")]
    pub mem_bytes: u64,
    /// Command line, cut to 256 characters.
    #[prost(string, tag = "6")]
    pub cmd: String,
}

/// Last minute of traffic through one endpoint on an edge.
#[derive(Clone, PartialEq, prost::Message)]
pub struct SiteTraffic {
    /// The endpoint's first domain (`$server_name`).
    #[prost(string, tag = "1")]
    pub site: String,
    #[prost(uint64, tag = "2")]
    pub requests: u64,
    /// Clients → edge.
    #[prost(uint64, tag = "3")]
    pub in_bytes_per_sec: u64,
    /// Edge → clients.
    #[prost(uint64, tag = "4")]
    pub out_bytes_per_sec: u64,
    #[prost(message, repeated, tag = "5")]
    pub upstreams: Vec<UpstreamTraffic>,
}

/// Traffic between an edge and one upstream (`ip:port`).
#[derive(Clone, PartialEq, prost::Message)]
pub struct UpstreamTraffic {
    #[prost(string, tag = "1")]
    pub address: String,
    #[prost(uint64, tag = "2")]
    pub requests: u64,
    /// Edge → upstream.
    #[prost(uint64, tag = "3")]
    pub sent_bytes_per_sec: u64,
    /// Upstream → edge.
    #[prost(uint64, tag = "4")]
    pub received_bytes_per_sec: u64,
    /// Attempts answered with a 5xx status, or not answered.
    #[prost(uint64, tag = "5")]
    pub errors: u64,
    /// 95th percentile of the upstream's response time, ms.
    #[prost(uint64, tag = "6")]
    pub p95_ms: u64,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct DiskVolume {
    #[prost(string, tag = "1")]
    pub mount_point: String,
    #[prost(string, tag = "2")]
    pub device: String,
    #[prost(string, tag = "3")]
    pub file_system: String,
    #[prost(uint64, tag = "4")]
    pub total_bytes: u64,
    #[prost(uint64, tag = "5")]
    pub available_bytes: u64,
    #[prost(bool, tag = "6")]
    pub removable: bool,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct HeartbeatResponse {
    #[prost(message, repeated, tag = "1")]
    pub commands: Vec<SignedCommand>,
    #[prost(uint32, tag = "2")]
    pub next_interval_secs: u32,
    #[prost(bool, tag = "3")]
    pub renew_certificate: bool,
    /// Container log and metric collection for this node (absent from old
    /// servers: the agent keeps its default, on).
    #[prost(message, optional, tag = "4")]
    pub container_telemetry: Option<ContainerTelemetryConfig>,
    /// The `seq` of the container log batch the server stored (0: none).
    /// The agent re-sends a batch until it is acknowledged.
    #[prost(uint64, tag = "5")]
    pub container_logs_ack: u64,
    /// The spool, the local log listener and where to upload (D-142).
    /// Absent from old servers and while analytics is off.
    #[prost(message, optional, tag = "6")]
    pub analytics: Option<AnalyticsAgentConfig>,
}

/// What a node needs to collect analytics data (D-142).
#[derive(Clone, PartialEq, Eq, prost::Message)]
pub struct AnalyticsAgentConfig {
    /// `https://host:port` of the worker's agent listener; empty: keep
    /// files until one is known.
    #[prost(string, tag = "1")]
    pub url: String,
    #[prost(message, optional, tag = "2")]
    pub spool: Option<SpoolTuning>,
    /// Ingest keys of the services placed on this node. At most
    /// [`MAX_INGEST_KEYS`].
    #[prost(message, repeated, tag = "3")]
    pub keys: Vec<IngestKey>,
    #[prost(message, optional, tag = "4")]
    pub listener: Option<ListenerTuning>,
}

/// Most ingest keys sent to one node.
pub const MAX_INGEST_KEYS: usize = 4_096;

/// One ingest key a node accepts on its listener.
#[derive(Clone, PartialEq, Eq, prost::Message)]
pub struct IngestKey {
    /// SHA-256 of the key; the key itself never leaves the server.
    #[prost(bytes = "vec", tag = "1")]
    pub hash: Vec<u8>,
    #[prost(string, tag = "2")]
    pub service_id: String,
}

/// Spool limits (node settings). `0` keeps the agent's default.
#[derive(Clone, Copy, PartialEq, Eq, prost::Message)]
pub struct SpoolTuning {
    #[prost(uint64, tag = "1")]
    pub max_bytes: u64,
    #[prost(uint64, tag = "2")]
    pub min_free_bytes: u64,
    #[prost(uint64, tag = "3")]
    pub max_age_secs: u64,
    #[prost(uint64, tag = "4")]
    pub seal_bytes: u64,
    #[prost(uint64, tag = "5")]
    pub seal_secs: u64,
    #[prost(uint32, tag = "6")]
    pub zstd_level: u32,
}

/// The local log listener (node settings). `0` keeps the agent's default.
#[derive(Clone, PartialEq, Eq, prost::Message)]
pub struct ListenerTuning {
    /// Most bytes of one request body.
    #[prost(uint64, tag = "1")]
    pub max_body_bytes: u64,
    /// Requests per second per key on addresses other than loopback.
    #[prost(uint32, tag = "2")]
    pub rate_per_sec: u32,
    /// Also apply the rate cap on loopback.
    #[prost(bool, tag = "3")]
    pub rate_on_loopback: bool,
    /// An extra address to listen on (a mesh address, when one exists);
    /// never a wildcard.
    #[prost(string, tag = "4")]
    pub extra_address: String,
}

/// The state of a node's spool, reported in each heartbeat.
#[derive(Clone, Copy, PartialEq, Eq, prost::Message)]
pub struct SpoolStatus {
    /// Sealed files waiting for upload.
    #[prost(uint64, tag = "1")]
    pub files: u64,
    #[prost(uint64, tag = "2")]
    pub bytes: u64,
    /// Files dropped since the agent started (cap, disk floor, age).
    #[prost(uint64, tag = "3")]
    pub dropped_files: u64,
    #[prost(uint64, tag = "4")]
    pub dropped_bytes: u64,
    /// Age of the oldest waiting file, seconds.
    #[prost(uint64, tag = "5")]
    pub oldest_age_secs: u64,
    /// Files the worker acknowledged since the agent started.
    #[prost(uint64, tag = "6")]
    pub uploaded_files: u64,
    /// Listener requests refused with 429 since the agent started.
    #[prost(uint64, tag = "7")]
    pub refused_requests: u64,
}

/// The worker's answer to a spool upload, sent once the rows are stored.
#[derive(Clone, PartialEq, Eq, prost::Message)]
pub struct SpoolAck {
    #[prost(string, tag = "1")]
    pub file_id: String,
    /// Rows stored from the file.
    #[prost(uint64, tag = "2")]
    pub rows: u64,
    /// The file was stored before; nothing was written again.
    #[prost(bool, tag = "3")]
    pub duplicate: bool,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct RenewRequest {
    #[prost(string, tag = "1")]
    pub csr_pem: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct RenewResponse {
    #[prost(string, tag = "1")]
    pub cert_pem: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct SignedCommand {
    #[prost(bytes = "vec", tag = "1")]
    pub body: Vec<u8>,
    #[prost(bytes = "vec", tag = "2")]
    pub signature: Vec<u8>,
    #[prost(string, tag = "3")]
    pub key_id: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct CommandBody {
    #[prost(string, tag = "1")]
    pub operation_id: String,
    #[prost(string, tag = "2")]
    pub server_id: String,
    #[prost(int64, tag = "3")]
    pub issued_at_ms: i64,
    #[prost(int64, tag = "4")]
    pub expires_at_ms: i64,
    #[prost(bytes = "vec", tag = "5")]
    pub nonce: Vec<u8>,
    #[prost(string, tag = "6")]
    pub principal: String,
    #[prost(
        oneof = "command_body::Command",
        tags = "10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 30, 31, 32, 33, 34, 38, 39, 40, 41, 42"
    )]
    pub command: Option<command_body::Command>,
}

pub mod command_body {
    #[derive(Clone, PartialEq, prost::Oneof)]
    pub enum Command {
        #[prost(message, tag = "10")]
        Ping(super::Ping),
        #[prost(message, tag = "11")]
        CollectFacts(super::CollectFacts),
        #[prost(message, tag = "12")]
        ServiceStatus(super::ServiceStatus),
        #[prost(message, tag = "13")]
        PackageAction(super::PackageAction),
        #[prost(message, tag = "14")]
        ContainerAction(super::ContainerAction),
        #[prost(message, tag = "15")]
        AppAction(super::AppAction),
        #[prost(message, tag = "16")]
        SiteAction(super::SiteAction),
        #[prost(message, tag = "17")]
        DnsAction(super::DnsAction),
        #[prost(message, tag = "18")]
        NetProbe(super::NetProbe),
        #[prost(message, tag = "19")]
        IngressAction(super::IngressAction),
        #[prost(message, tag = "20")]
        FabricAction(super::FabricAction),
        #[prost(message, tag = "21")]
        StorageAction(super::StorageAction),
        #[prost(message, tag = "22")]
        FileAction(super::FileAction),
        // Tag 23 (`AccessAction`, WireGuard remote access) was retired with
        // D-100 and is never reused.
        #[prost(message, tag = "24")]
        AgentUpdate(super::AgentUpdate),
        #[prost(message, tag = "25")]
        CancelAction(super::CancelAction),
        #[prost(message, tag = "30")]
        ProcessList(super::ProcessList),
        #[prost(message, tag = "31")]
        StackAction(super::StackAction),
        #[prost(message, tag = "32")]
        TerminalAccount(super::TerminalAccount),
        #[prost(message, tag = "33")]
        ProcessAction(super::ProcessAction),
        #[prost(message, tag = "34")]
        HostNames(super::HostNames),
        #[prost(message, tag = "38")]
        TunnelOpen(super::TunnelOpen),
        // Tag 35 was reserved for a dropped health command; 36 and 37 are
        // skipped to be safe.
        #[prost(message, tag = "39")]
        ContainerExecOpen(super::ContainerExecOpen),
        #[prost(message, tag = "40")]
        DockerTlsAction(super::DockerTlsAction),
        #[prost(message, tag = "41")]
        SwarmAction(super::SwarmAction),
        #[prost(message, tag = "42")]
        DockerAction(super::DockerAction),
    }

    impl Command {
        /// Stable name used by the agent's local allowlist.
        #[must_use]
        pub const fn name(&self) -> &'static str {
            match self {
                Command::Ping(_) => "ping",
                Command::CollectFacts(_) => "collect_facts",
                Command::ServiceStatus(_) => "service_status",
                Command::PackageAction(_) => "package",
                Command::ContainerAction(_) => "container",
                Command::AppAction(_) => "app",
                Command::SiteAction(_) => "site",
                Command::DnsAction(_) => "dns",
                Command::NetProbe(_) => "probe",
                Command::IngressAction(_) => "ingress",
                Command::FabricAction(_) => "fabric",
                Command::StorageAction(_) => "storage",
                Command::FileAction(_) => "files",
                Command::CancelAction(_) => "cancel",
                Command::AgentUpdate(_) => "update",
                Command::ProcessList(_) => "processes",
                Command::StackAction(_) => "stack",
                Command::TerminalAccount(_) => "terminal_account",
                Command::ProcessAction(_) => "process_action",
                Command::HostNames(_) => "names",
                Command::TunnelOpen(_) => "tunnel",
                // Its own allow-list entry: an existing agent opts in by
                // adding it (D-114).
                Command::ContainerExecOpen(_) => "container_exec",
                Command::DockerTlsAction(_) => "docker_tls",
                Command::SwarmAction(_) => "swarm",
                Command::DockerAction(_) => "docker",
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, prost::Message)]
pub struct Ping {}

#[derive(Clone, Copy, PartialEq, Eq, prost::Message)]
pub struct CollectFacts {}

#[derive(Clone, PartialEq, prost::Message)]
pub struct ServiceStatus {
    #[prost(string, tag = "1")]
    pub name: String,
}

/// Runs a catalog recipe action through the node's root helper. Only ids,
/// versions and typed parameters travel; the recipe is compiled into the helper.
#[derive(Clone, PartialEq, prost::Message)]
pub struct PackageAction {
    #[prost(string, tag = "1")]
    pub package: String,
    #[prost(string, tag = "2")]
    pub version: String,
    #[prost(string, tag = "3")]
    pub action: String,
    #[prost(btree_map = "string, string", tag = "4")]
    pub params: std::collections::BTreeMap<String, String>,
}

/// A Docker Engine operation run by the root helper (closed set; no exec/run).
#[derive(Clone, PartialEq, prost::Message)]
pub struct ContainerAction {
    /// info | list | images | inspect | start | stop | restart | remove | logs | stats | pull
    #[prost(string, tag = "1")]
    pub op: String,
    #[prost(string, tag = "2")]
    pub id: String,
    #[prost(string, tag = "3")]
    pub image: String,
    #[prost(uint32, tag = "4")]
    pub tail: u32,
    /// `logs`: only lines after this Unix time (decimal seconds); live follow.
    #[prost(string, tag = "5")]
    pub since: String,
    /// `inspect`: also return environment values (`env`), not only the
    /// names. Absent on the wire from older servers and ignored by older
    /// agents, which keep answering with `env_keys` only.
    #[prost(bool, tag = "6")]
    pub reveal_env: bool,
}

/// Deploys or operates a service (`forge_catalog::AppSpec`, JSON-encoded;
/// validated again by the node helper). May carry resolved secret values: it
/// lives only in memory and on the mTLS link.
#[derive(Clone, PartialEq, prost::Message)]
pub struct AppAction {
    /// deploy | start | stop | restart | status | logs | run | remove |
    /// health. `health` replaces the app's health policy on the node:
    /// `spec_json` is then a `forge_catalog::health::ApplySpec`, and the
    /// agent handles it without the helper (D-105).
    #[prost(string, tag = "1")]
    pub action: String,
    #[prost(string, tag = "2")]
    pub spec_json: String,
    #[prost(uint32, tag = "3")]
    pub tail: u32,
    /// `follow`: where the previous segment stopped (journal cursor, byte
    /// offset or Docker timestamp); empty = start with the last lines.
    #[prost(string, tag = "4")]
    pub cursor: String,
}

/// Applies or removes an Nginx site (`forge_catalog::SiteSpec`, JSON).
#[derive(Clone, PartialEq, prost::Message)]
pub struct SiteAction {
    /// apply | remove | status
    #[prost(string, tag = "1")]
    pub action: String,
    #[prost(string, tag = "2")]
    pub spec_json: String,
}

/// A PowerDNS operation on the node's local API (`DnsOp`, JSON).
#[derive(Clone, PartialEq, prost::Message)]
pub struct DnsAction {
    #[prost(string, tag = "1")]
    pub op_json: String,
}

/// Cluster ingress (global Nginx, keepalived) and overlay network operations
/// (`forge_catalog::ingress::IngressOp`, JSON).
#[derive(Clone, PartialEq, prost::Message)]
pub struct IngressAction {
    #[prost(string, tag = "1")]
    pub op_json: String,
}

/// Forge network fabric operations (`forge_catalog::fabric::FabricOp`, JSON).
#[derive(Clone, PartialEq, prost::Message)]
pub struct FabricAction {
    #[prost(string, tag = "1")]
    pub op_json: String,
}

/// Object store instance operation (`forge_catalog::storage::StoreOp`, JSON).
/// An install carries the store's bootstrap credential: it lives only in
/// memory and on the mTLS link, like resolved service secrets.
#[derive(Clone, PartialEq, prost::Message)]
pub struct StorageAction {
    #[prost(string, tag = "1")]
    pub op_json: String,
}

/// File explorer operation (`forge_core::files::FileOp`, JSON). The node
/// confines it to the root and applies the protected-path policy.
#[derive(Clone, PartialEq, prost::Message)]
pub struct FileAction {
    #[prost(string, tag = "1")]
    pub op_json: String,
}

/// dockerd's TCP listener with mutual TLS (`forge_catalog::docker_tls::DockerTlsOp`,
/// JSON; D-125). Carries certificates only, never a private key: dockerd's
/// key is made on the node.
#[derive(Clone, PartialEq, prost::Message)]
pub struct DockerTlsAction {
    #[prost(string, tag = "1")]
    pub op_json: String,
}

/// Docker Swarm membership or node operation (`forge_catalog::swarm::SwarmOp`,
/// JSON; D-154). Join tokens in it or its result are secrets.
#[derive(Clone, PartialEq, prost::Message)]
pub struct SwarmAction {
    #[prost(string, tag = "1")]
    pub op_json: String,
}

/// Docker image, volume or network operation
/// (`forge_catalog::docker_ops::DockerOp`, JSON; D-157). Registry
/// credentials in it are secrets.
#[derive(Clone, PartialEq, prost::Message)]
pub struct DockerAction {
    #[prost(string, tag = "1")]
    pub op_json: String,
}

/// Compose stack operation (`forge_catalog::stacks::StackOp`, JSON; issue #7).
/// The node checks the stack policy again before running `docker compose`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct StackAction {
    #[prost(string, tag = "1")]
    pub op_json: String,
}

/// Creates (or keeps) a terminal account on the node, run by the root helper
/// (D-078): no password, login only with Forge's node-bound SSH certificates,
/// passwordless sudo through a `visudo`-checked drop-in only when `sudo`.
#[derive(Clone, PartialEq, Eq, prost::Message)]
pub struct TerminalAccount {
    #[prost(string, tag = "1")]
    pub name: String,
    #[prost(bool, tag = "2")]
    pub sudo: bool,
}

/// Installs a new agent and helper (GitHub issue #4). The binaries come from
/// `download_base` (`/agent/download/linux/<arch>/<binary>`) and must match the
/// SHA-256 values carried in this signed command; the helper keeps the old
/// binaries and restores them if the new agent does not reconnect.
#[derive(Clone, PartialEq, prost::Message)]
pub struct AgentUpdate {
    #[prost(string, tag = "1")]
    pub version: String,
    #[prost(string, tag = "2")]
    pub download_base: String,
    #[prost(string, tag = "3")]
    pub arch: String,
    #[prost(string, tag = "4")]
    pub agent_sha256: String,
    #[prost(string, tag = "5")]
    pub helper_sha256: String,
    /// Install even when `version` is older than the node's helper; older
    /// helpers ignore it (and never refuse a downgrade).
    #[prost(bool, tag = "6")]
    pub allow_downgrade: bool,
}

/// An htop-like snapshot of the node's processes (read-only).
#[derive(Clone, PartialEq, prost::Message)]
pub struct ProcessList {
    /// `cpu` (default) | `mem` | `pid` | `time` | `name`.
    #[prost(string, tag = "1")]
    pub sort: String,
    /// Most processes returned (1–500).
    #[prost(uint32, tag = "2")]
    pub limit: u32,
    /// Case-insensitive substring of name, command line or user.
    #[prost(string, tag = "3")]
    pub filter: String,
}

/// Ends a process (or its process tree) or changes its priority (D-080).
/// The node refuses when the process with `pid` did not start at
/// `start_time`, so a reused PID is never hit, and refuses protected
/// processes (init, kernel threads, Windows system processes, Forge itself).
/// Linux: the root helper runs it; Windows: the agent (LocalSystem).
#[derive(Clone, PartialEq, Eq, prost::Message)]
pub struct ProcessAction {
    /// `signal` | `nice` (Linux) | `priority_class` (Windows).
    #[prost(string, tag = "1")]
    pub op: String,
    #[prost(uint32, tag = "2")]
    pub pid: u32,
    /// Start time as the process list reports it (seconds since the epoch).
    #[prost(uint64, tag = "3")]
    pub start_time: u64,
    /// `signal`: `term` | `kill` | `hup` | `int` (Windows: `kill` only).
    #[prost(string, tag = "4")]
    pub signal: String,
    /// `signal`: the process's descendants too.
    #[prost(bool, tag = "5")]
    pub tree: bool,
    /// `nice`: -20 (highest priority) to 19.
    #[prost(sint32, tag = "6")]
    pub nice: i32,
    /// `priority_class`: `idle` | `below_normal` | `normal` | `above_normal` | `high`.
    #[prost(string, tag = "7")]
    pub class: String,
}

/// Stops a background operation the agent is still running (a recipe, an
/// object-store install…): the agent drops it and the helper kills the
/// operation's process group.
#[derive(Clone, PartialEq, prost::Message)]
pub struct CancelAction {
    #[prost(string, tag = "1")]
    pub operation_id: String,
}

/// Reachability checks run *from* the node: ICMP ping, falling back to a TCP
/// connect when ping is unavailable.
#[derive(Clone, PartialEq, prost::Message)]
pub struct NetProbe {
    /// `host` or `host:port` (the port is used by the TCP fallback).
    #[prost(string, repeated, tag = "1")]
    pub targets: Vec<String>,
    /// Echo requests per target (1–10).
    #[prost(uint32, tag = "2")]
    pub count: u32,
}

/// Opens one port-tunnel connection (D-091, D-100): the agent dials
/// `<dial_ip>:<port>` on this node (`127.0.0.1` or `[::1]` when `dial_ip` is
/// empty; otherwise one of the node's own addresses, such as its cluster
/// network address) and then opens a data stream to the control plane at
/// `/agent/v1/tunnel/<connection_id>`, presenting `stream_token`. Bytes flow
/// raw in both directions until either side closes or the connection is idle
/// for `idle_secs`. Runs in the agent itself (no root needed).
#[derive(Clone, PartialEq, Eq, prost::Message)]
pub struct TunnelOpen {
    #[prost(string, tag = "1")]
    pub connection_id: String,
    #[prost(uint32, tag = "2")]
    pub port: u32,
    /// Ports this node allows tunnels to (signed copy of the allow-list); the
    /// agent refuses `port` when it is not listed.
    #[prost(uint32, repeated, tag = "3")]
    pub allowed_ports: Vec<u32>,
    /// Single-use secret for the data stream (hex); the server keeps its hash.
    #[prost(string, tag = "4")]
    pub stream_token: String,
    #[prost(uint32, tag = "5")]
    pub idle_secs: u32,
    /// Hard end of the connection, epoch ms.
    #[prost(int64, tag = "6")]
    pub expires_at_ms: i64,
    /// Address to dial on the node; empty means loopback. The agent accepts
    /// loopback and its own interface addresses only (D-100). Agents from
    /// before this field ignore it and dial loopback.
    #[prost(string, tag = "7")]
    pub dial_ip: String,
}

/// Opens a shell inside a container on this node (D-114). The agent asks the
/// root helper, which verifies this command itself and runs a Docker exec
/// with a TTY as the image's default user (bash when the image has it, else
/// sh). The agent then dials the control plane at
/// `/agent/v1/exec/<session_id>` with `stream_token` and relays the bytes,
/// framed with [`crate::exec_frame`]. Agents from before this message
/// ignore it (the server checks the node's `container_exec` fact first).
#[derive(Clone, PartialEq, Eq, prost::Message)]
pub struct ContainerExecOpen {
    /// 32 lower-case hex characters.
    #[prost(string, tag = "1")]
    pub session_id: String,
    /// Container id or name (`[A-Za-z0-9][A-Za-z0-9_.-]{0,127}`).
    #[prost(string, tag = "2")]
    pub container: String,
    #[prost(uint32, tag = "3")]
    pub cols: u32,
    #[prost(uint32, tag = "4")]
    pub rows: u32,
    /// Single-use secret for the data stream (64 lower-case hex); the
    /// server keeps its hash.
    #[prost(string, tag = "5")]
    pub stream_token: String,
    /// The node ends a shell with no traffic for this long.
    #[prost(uint32, tag = "6")]
    pub idle_secs: u32,
    /// The stream must be dialled by then (epoch ms).
    #[prost(int64, tag = "7")]
    pub expires_at_ms: i64,
    /// Hard end of the shell, seconds after it opened.
    #[prost(uint32, tag = "8")]
    pub lifetime_secs: u32,
}

/// The fleet's names for this node (D-084): the complete list, which
/// replaces the node's `names` block in its hosts file (and in the hosts
/// files of its Forge containers). An empty list removes the block.
/// Linux: the root helper writes it; Windows: the agent (LocalSystem).
#[derive(Clone, PartialEq, Eq, prost::Message)]
pub struct HostNames {
    /// JSON array of `forge_catalog::names::NameEntry` (`{"name", "ip"}`).
    #[prost(string, tag = "1")]
    pub entries_json: String,
    /// Also update the hosts files of the node's Forge-managed containers.
    #[prost(bool, tag = "2")]
    pub containers: bool,
    /// The suffix every name ends in (`local`); the node refuses names
    /// outside it and suffixes that could be public domains.
    #[prost(string, tag = "3")]
    pub suffix: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct CommandResult {
    #[prost(string, tag = "1")]
    pub operation_id: String,
    #[prost(bool, tag = "2")]
    pub ok: bool,
    #[prost(string, tag = "3")]
    pub output: String,
    #[prost(string, tag = "4")]
    pub error: String,
    /// Why the node refused, when it is a condition the control plane acts
    /// on ([`RefusalCode`]); empty otherwise and from older agents.
    #[prost(string, tag = "5")]
    pub error_code: String,
}

/// A refusal the control plane tells apart from a failure
/// (`CommandResult::error_code`). Rollouts skip a node that cannot take an
/// update (`unsupported`, `downgrade`) instead of stopping, and wait for a
/// `busy` one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefusalCode {
    /// The node cannot do this here (no systemd, no `curl`, the command is
    /// not allowed, wrong OS): it needs a reinstall or a configuration change.
    Unsupported,
    /// The node is doing the same kind of work already (another update).
    Busy,
    /// The command would install an older version, and it did not allow that.
    Downgrade,
}

impl RefusalCode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unsupported => "unsupported",
            Self::Busy => "busy",
            Self::Downgrade => "downgrade",
        }
    }

    /// The code in `error_code`; `None` for an empty or unknown one.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        [Self::Unsupported, Self::Busy, Self::Downgrade].into_iter().find(|c| c.as_str() == s)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod oneof_tests {
    use prost::Message;

    use super::*;

    #[test]
    fn refusal_codes_round_trip_and_old_results_have_none() {
        for c in [RefusalCode::Unsupported, RefusalCode::Busy, RefusalCode::Downgrade] {
            assert_eq!(RefusalCode::parse(c.as_str()), Some(c));
        }
        assert_eq!(RefusalCode::parse(""), None);
        assert_eq!(RefusalCode::parse("other"), None);
        // A result from an older agent (no field 5) decodes with no code.
        let old = CommandResult { operation_id: "op".into(), ok: false, error: "no".into(), ..Default::default() };
        let back = CommandResult::decode(old.encode_to_vec().as_slice()).unwrap();
        assert_eq!(RefusalCode::parse(&back.error_code), None);
        let coded = CommandResult { error_code: RefusalCode::Busy.as_str().into(), ..old };
        let back = CommandResult::decode(coded.encode_to_vec().as_slice()).unwrap();
        assert_eq!(RefusalCode::parse(&back.error_code), Some(RefusalCode::Busy));
    }

    /// Every command variant must survive encode → decode: a tag missing from
    /// the `oneof` attribute is silently dropped by the decoder.
    #[test]
    fn every_command_round_trips() {
        let cmds = vec![
            command_body::Command::IngressAction(IngressAction { op_json: "{}".into() }),
            command_body::Command::FabricAction(FabricAction { op_json: "{}".into() }),
            command_body::Command::StorageAction(StorageAction { op_json: "{}".into() }),
            command_body::Command::FileAction(FileAction { op_json: "{}".into() }),
            command_body::Command::CancelAction(CancelAction { operation_id: "op-1".into() }),
            command_body::Command::NetProbe(NetProbe::default()),
            command_body::Command::AgentUpdate(AgentUpdate {
                version: "1.2.3".into(),
                arch: "x86_64".into(),
                ..Default::default()
            }),
            command_body::Command::ProcessList(ProcessList::default()),
            command_body::Command::StackAction(StackAction { op_json: "{}".into() }),
            command_body::Command::DockerTlsAction(DockerTlsAction { op_json: "{}".into() }),
            command_body::Command::SwarmAction(SwarmAction { op_json: "{}".into() }),
            command_body::Command::DockerAction(DockerAction { op_json: "{}".into() }),
            command_body::Command::TerminalAccount(TerminalAccount { name: "forge-ops".into(), sudo: true }),
            command_body::Command::ProcessAction(ProcessAction {
                op: "signal".into(),
                pid: 4242,
                start_time: 1_700_000_000,
                signal: "term".into(),
                tree: true,
                nice: -5,
                class: "high".into(),
            }),
            command_body::Command::HostNames(HostNames {
                entries_json: "[]".into(),
                containers: true,
                suffix: "local".into(),
            }),
            command_body::Command::TunnelOpen(TunnelOpen {
                connection_id: "c1".into(),
                port: 5432,
                allowed_ports: vec![5432],
                stream_token: "ab".into(),
                idle_secs: 900,
                expires_at_ms: 1,
                dial_ip: "10.64.0.17".into(),
            }),
            command_body::Command::ContainerExecOpen(ContainerExecOpen {
                session_id: "a".repeat(32),
                container: "web-1".into(),
                cols: 120,
                rows: 32,
                stream_token: "b".repeat(64),
                idle_secs: 1800,
                expires_at_ms: 1_800_000_000_000,
                lifetime_secs: 43_200,
            }),
        ];
        for c in cmds {
            let body = CommandBody { operation_id: "op".into(), command: Some(c.clone()), ..Default::default() };
            let back = CommandBody::decode(body.encode_to_vec().as_slice()).unwrap();
            assert_eq!(back.command, Some(c));
        }
    }
}

/// Most headers one [`HealthSecretsResponse`] carries (the policy's limit
/// per check).
pub const MAX_HEALTH_SECRET_HEADERS: usize = 20;

/// A node asks for the vault-backed header values of one HTTP health check
/// it is about to run (D-111). POST [`crate::paths::HEALTH_SECRETS`] over
/// the agent's mTLS link. The check is named by reference; the server reads
/// the vault references from its own stored policy, never from the node.
#[derive(Clone, PartialEq, Eq, prost::Message)]
pub struct HealthSecretsRequest {
    /// Environment slug.
    #[prost(string, tag = "1")]
    pub environment: String,
    /// Service slug.
    #[prost(string, tag = "2")]
    pub app: String,
    /// `startup`, `readiness` or `liveness`.
    #[prost(string, tag = "3")]
    pub purpose: String,
    #[prost(string, tag = "4")]
    pub check: String,
    /// The policy revision the node runs; another one than the stored
    /// policy's is refused (`stale_policy`).
    #[prost(int64, tag = "5")]
    pub policy_revision: i64,
}

/// One resolved header. `Debug` never prints the value.
#[derive(Clone, PartialEq, Eq, prost::Message)]
#[prost(skip_debug)]
pub struct HealthSecretHeader {
    #[prost(string, tag = "1")]
    pub name: String,
    #[prost(string, tag = "2")]
    pub value: String,
}

impl std::fmt::Debug for HealthSecretHeader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HealthSecretHeader").field("name", &self.name).field("value", &"<redacted>").finish()
    }
}

/// The answer to a [`HealthSecretsRequest`]. The node keeps granted values
/// in memory only, for at most `ttl_secs`, then asks again.
#[derive(Clone, PartialEq, Eq, prost::Message)]
pub struct HealthSecretsResponse {
    #[prost(bool, tag = "1")]
    pub granted: bool,
    /// The check's vault-backed headers, in policy order (granted only).
    #[prost(message, repeated, tag = "2")]
    pub headers: Vec<HealthSecretHeader>,
    /// How long the node may reuse the answer, granted or not.
    #[prost(uint32, tag = "3")]
    pub ttl_secs: u32,
    /// Why not: a code of [`health_secret_denial`]. Empty when granted.
    #[prost(string, tag = "4")]
    pub denial: String,
}

/// The codes of [`HealthSecretsResponse::denial`]: why a node gets no values.
pub mod health_secret_denial {
    /// No such service placed on the asking node.
    pub const NOT_PLACED: &str = "not_placed";
    /// The node runs another policy revision than the stored one.
    pub const STALE_POLICY: &str = "stale_policy";
    /// The stored policy has no such enabled check with a vault reference.
    pub const NO_SUCH_CHECK: &str = "no_such_check";
    /// Nobody is on record as having saved the policy.
    pub const NO_AUTHORITY: &str = "no_authority";
    /// The person who saved the policy no longer exists.
    pub const SAVER_GONE: &str = "saver_gone";
    pub const SAVER_DISABLED: &str = "saver_disabled";
    /// They may no longer use vault items.
    pub const NOT_ALLOWED: &str = "not_allowed";
    /// A referenced item no longer exists.
    pub const ITEM_MISSING: &str = "item_missing";
    /// The node asked too often.
    pub const RATE_LIMITED: &str = "rate_limited";
    pub const INTERNAL: &str = "internal";
}
