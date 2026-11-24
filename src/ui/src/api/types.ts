// Types mirroring the /api/v1 JSON shapes (see forge-core::model).
import type { SwarmSettings } from "./swarm";

export type AgentStatus = "pending" | "online" | "offline" | "revoked";
export type JobStatus = "queued" | "running" | "succeeded" | "failed" | "cancelled";
export type Role = "platform_admin" | "workspace_admin" | "operator" | "developer" | "dba" | "auditor" | "customer";

export interface EntityRef {
  kind: string;
  id: string;
}

export interface User {
  id: string;
  /** Sign-in name; at least one of username and email is set. */
  username: string | null;
  email: string | null;
  display_name: string;
  disabled: boolean;
  oidc_subject: string | null;
  created_at: number;
}

export interface RoleBinding {
  id: string;
  user_id: string;
  role: Role;
  scope: EntityRef;
  created_at: number;
}

export interface Me {
  user: User;
  mfa: boolean;
  mfa_enrolled: boolean;
  session_kind: "web" | "bearer";
  bindings: RoleBinding[];
}

export interface Workspace {
  id: string;
  name: string;
  slug: string;
  created_at: number;
}

export interface Tenant {
  id: string;
  workspace_id: string;
  name: string;
  created_at: number;
}

/** A **Project** (product, e.g. "shop"): a group of environments and the services in them. */
export interface ProjectBase {
  id: string;
  workspace_id: string;
  name: string;
  description: string;
  created_at: number;
}

/** One environment as the project view embeds it. */
export interface ProjectEnvironment {
  id: string;
  name: string;
  cluster_id: string | null;
  protected: boolean;
}

/** `GET /api/v1/projects` row: the project plus the environments the caller may view and its service count. */
export interface Project extends ProjectBase {
  environments: ProjectEnvironment[];
  service_count: number;
}

/**
 * An **Environment**: it lives in one cluster (D-056) and in one project.
 * `cluster_id` / `project_id` are `null` only for rows created before either
 * was required: the UI shows them as **Unassigned**.
 */
export interface Environment {
  id: string;
  workspace_id: string;
  cluster_id: string | null;
  project_id: string | null;
  name: string;
  protected: boolean;
  created_at: number;
}

/** `default`: Forge orchestration (system and container services). `swarm`: Docker Swarm (containers only). */
export type ClusterMode = "default" | "swarm";

export interface Cluster {
  id: string;
  workspace_id: string;
  name: string;
  description: string;
  environment_id: string | null;
  abbr?: string;
  icon?: string | null;
  /** Orchestration mode, fixed at creation (D-154). */
  mode?: ClusterMode;
  created_at: number;
  node_count?: number;
  nodes_online?: number;
}

export type NodeRole = "control" | "worker";

export interface Server {
  id: string;
  workspace_id: string;
  environment_id: string | null;
  cluster_id: string | null;
  /** `control` hosts forge-server itself; every other node is a `worker`. */
  role: NodeRole;
  /** Console name set by users; falls back to `hostname`. */
  display_name?: string | null;
  hostname: string;
  os: string;
  arch: string;
  agent_version: string;
  status: AgentStatus;
  last_seen_at: number | null;
  cert_serial: string | null;
  cert_not_after: number | null;
  created_at: number;
  /** Collected system facts; absent on older servers. Read via `nodeFacts`. */
  facts?: NodeFacts | null;
  /** Tags such as `active` or `dc:fra1`; absent on older servers. Read via `nodeTags`. */
  tags?: string[] | null;
  /** Local name (D-084); only on `GET /servers/{id}`. */
  dns?: NodeDns | null;
}

/** A node's local name and whether its hosts file is up to date. */
export interface NodeDns {
  label: string | null;
  /** A user set the label (otherwise it comes from the node's name). */
  label_set: boolean;
  name: string | null;
  sync?: NameSync | null;
}

export interface NameSync {
  names: number;
  in_sync: boolean;
  synced_at_ms?: number;
  error?: string;
}

export interface PublishedPort {
  port: number;
  proto: string;
  target: number;
  bind?: string;
}

export interface LocalName {
  name: string;
  kind: "node" | "service";
  server_id: string;
  node: string;
  cluster_id?: string;
  address?: string;
  mesh_address?: string;
  app?: string;
  ports?: PublishedPort[];
  workspace_id: string;
}

export type SuffixSource = "setting" | "config" | "default";

/** `GET/PUT /api/v1/names/suffix`. */
export interface NameSuffix {
  suffix: string;
  source: SuffixSource;
  config_suffix: string;
  default_suffix: string;
}

export interface LocalNames {
  enabled: boolean;
  suffix: string;
  /** Where the suffix comes from (older servers leave it out). */
  suffix_source?: SuffixSource;
  names: LocalName[];
  nodes: ({ server_id: string } & NodeDns)[];
}

export interface Job {
  id: string;
  kind: string;
  workspace_id: string | null;
  status: JobStatus;
  current_step: number;
  attempts: number;
  max_attempts: number;
  resource: EntityRef;
  principal: { type: string; id: string };
  payload: Record<string, unknown>;
  state: Record<string, unknown>;
  last_error: string | null;
  created_at: number;
  updated_at: number;
}

/** One console line (onboarding console, `state.log`, or split job output). */
export interface ConsoleLine {
  /** Unix ms, when known. */
  t?: number;
  /** Job step (`connect`, `install`…) or `installer`. */
  step?: string;
  line: string;
}

/** `GET /api/v1/onboarding/console`, normalized. */
export interface OnboardingConsole {
  status: string;
  error: string | null;
  server_id: string | null;
  lines: ConsoleLine[];
  /** The endpoint failed or does not exist on this server. */
  unavailable?: boolean;
}

/** `GET /api/v1/onboarding/nodes` entry, normalized. */
export interface OnboardingNode {
  kind: "assisted" | "manual";
  job_id: string | null;
  token_id: string | null;
  status: string;
  error: string | null;
  host: string | null;
  hostname: string | null;
  cluster_id: string | null;
  server_id: string | null;
  created_at: number | null;
  updated_at: number | null;
  active: boolean;
}

export interface JobStep {
  index: number;
  name: string;
  status: string;
  started_at: number;
  finished_at: number | null;
  error: string | null;
}

export interface Plugin {
  plugin_id: string;
  version: string;
  publisher: string;
  state: "installed" | "enabled" | "disabled" | "quarantined";
  pinned: boolean;
  approved_permissions: string[];
  fault_count: number;
  quarantine_reason: string | null;
  updated_at: number;
}

export interface SecretMeta {
  id: string;
  workspace_id: string;
  kind: string;
  label: string;
  expires_at: number | null;
  created_at: number;
}

export interface HostKey {
  algorithm: string;
  fingerprint: string;
}

export interface Capabilities {
  capabilities: { id: string; version: number; enabled: boolean }[];
}

export interface VersionInfo {
  product: string;
  version: string;
  api_versions: string[];
  /** Git commit the server was built from, when known. */
  build?: string | null;
}

export type LiveEvent =
  | { type: "job"; job_id: string; kind: string; status: JobStatus; step: string | null }
  | { type: "server"; server_id: string; status: AgentStatus };

// ------------------------------------------------------------------ App Store

export type PackageCategory = "runtimes" | "web" | "databases" | "storage" | "containers" | "dns" | "tools" | "messaging" | "monitoring" | "devops";

export type ParamKind =
  | { type: "port" }
  | { type: "choice"; options: string[] }
  | { type: "bool" }
  | { type: "list" }
  | { type: "address" };

export interface PackageParam {
  key: string;
  label: string;
  kind: ParamKind;
  default: string;
  help: string;
  must_accept?: boolean;
}

export interface CatalogPackage {
  id: string;
  name: string;
  category: PackageCategory;
  kind: "native" | "container" | "plugin";
  summary: string;
  homepage: string;
  license: string;
  versions: string[];
  params: PackageParam[];
  requires: string[];
  ports: number[];
  os: string[];
  notes: string;
  upstream: string;
  release_notes: string;
  integration?: PackageIntegration | null;
}

export interface PackageIntegration {
  role: string;
  exports: { name: string; meaning: string; secret: boolean }[];
  services: string[];
  paths: [string, string][];
  used_by: string[];
  usage: string[];
}

export interface Catalog {
  packages: CatalogPackage[];
  roadmap: { name: string; description: string }[];
}

export interface Installation {
  id: string;
  workspace_id: string;
  server_id: string;
  package: string;
  version: string;
  params: Record<string, string>;
  status: string;
  state: string | null;
  env: Record<string, string>;
  last_job_id: string | null;
  last_error: string | null;
  versions?: PackageVersions | null;
  update_policy?: UpdatePolicy;
  created_at: number;
  updated_at: number;
}

export type UpdatePolicy = "notify" | "auto_patch";
export type UpdateKind = "revision" | "patch" | "minor" | "major";

export interface PackageVersions {
  manager: string;
  package: string;
  installed: string;
  candidate: string;
  running: string;
  checked_at: number;
}

export interface PackageRelease {
  product: string;
  line: string;
  latest: string;
  lts: boolean;
  released_at: string | null;
  latest_at: string | null;
  eol: string | null;
  support_until: string | null;
}

export interface UpdateRow {
  installation_id: string;
  server_id: string;
  package: string;
  name: string;
  line: string;
  status: string;
  state: string | null;
  manager: string;
  os_package: string;
  installed: string;
  candidate: string;
  running: string;
  checked_at: number | null;
  kind: UpdateKind | null;
  update_policy: UpdatePolicy;
  release_notes: string;
  upstream: {
    line: string;
    latest: string;
    lts: boolean;
    eol: string | null;
    support_until: string | null;
    newest_line: string | null;
    ahead_of_node: boolean;
  } | null;
}

// ------------------------------------------------------------------ services

/** `compose-service` is a service registered from a compose stack: read-only here, managed on its stack. */
export type ServiceRuntime = "systemd" | "container" | "compose-service";
/** The kinds a user creates and edits. */
export type EditableRuntime = Exclude<ServiceRuntime, "compose-service">;

export interface ServiceEnvVar {
  key: string;
  value: string | null;
  secret: boolean;
}

export interface ServiceSpec {
  environment: string;
  /** `compose-service` only: the stack it comes from. */
  stack?: string;
  stack_id?: string;
  service?: string;
  command: string;
  working_dir: string;
  /** Native service: the TCP port it listens on (default health check). */
  listen_port?: number | null;
  image: string;
  ports: string[];
  volumes: string[];
  env_files: string[];
  restart: boolean;
  dependencies?: ServiceDependency[];
  cpu_millis?: number;
  memory_mb?: number;
  entrypoint?: string;
  user?: string;
  hostname?: string;
  labels?: [string, string][];
  extra_hosts?: string[];
  healthcheck?: HealthCheck | null;
  stop_timeout_secs?: number;
  read_only?: boolean;
  log_max_mb?: number;
  /** Container services in a Swarm cluster: replicas, placement, restart and rollout. */
  swarm?: SwarmSettings | null;
}

export interface ServiceDependency {
  package: string;
  version: string;
  params: Record<string, string>;
}

export interface HealthCheck {
  command: string;
  interval_secs: number;
  timeout_secs: number;
  retries: number;
  start_period_secs: number;
}

export interface ServiceInstance {
  service_id: string;
  server_id: string;
  role: "active" | "standby";
  status: string;
  state: string | null;
  last_job_id: string | null;
  last_error: string | null;
  updated_at: number;
}

export interface Service {
  id: string;
  workspace_id: string;
  environment_id: string;
  name: string;
  runtime: ServiceRuntime;
  spec: ServiceSpec;
  env: ServiceEnvVar[];
  created_by?: string;
  created_at: number;
  updated_at: number;
  instances: ServiceInstance[];
  /** The caller may deploy, start, stop and restart it (a hint; the server still authorizes). Absent on older servers. */
  can_deploy?: boolean;
}

/** Per-node health status of one service (`GET /services/health-summary`). */
export interface ServiceHealthSummary {
  service_id: string;
  instances: { server_id: string; status: "healthy" | "degraded" | "unhealthy" | "unknown" }[];
}

export interface HealthSummaryList {
  items: ServiceHealthSummary[];
  /** More services were visible than one response covers. */
  truncated: boolean;
}

export interface ServiceInput {
  environment_id?: string;
  name?: string;
  runtime: EditableRuntime;
  command: string;
  working_dir: string;
  /** Native service: the TCP port it listens on (default health check). */
  listen_port?: number | null;
  image: string;
  ports: string[];
  volumes: string[];
  env_files: string[];
  restart: boolean;
  env: { key: string; value: string | null; secret: boolean }[];
  dependencies: ServiceDependency[];
  cpu_millis: number;
  memory_mb: number;
  entrypoint: string;
  user: string;
  hostname: string;
  labels: [string, string][];
  extra_hosts: string[];
  healthcheck: HealthCheck | null;
  stop_timeout_secs: number;
  read_only: boolean;
  log_max_mb: number;
  /** Set only for services in a Swarm cluster; omitted otherwise. */
  swarm?: SwarmSettings;
}

/** Name to show for a node. */
export const nodeName = (n: Pick<Server, "hostname" | "display_name">) => n.display_name || n.hostname;

// ------------------------------------------------------------------ object storage

export type StoreMode = "managed" | "external";

export interface StoreMember {
  server_id: string;
  hostname?: string | null;
  /** One path per drive; standalone stores have one. */
  data_paths: string[];
  /** Host the control plane and other members use to reach this node. */
  endpoint_host?: string | null;
  require_mount?: boolean;
  status?: string | null;
  capacity_bytes?: number | null;
  free_bytes?: number | null;
  /** Data path is its own mount point; null until observed. */
  dedicated?: boolean | null;
  observed_at?: number | null;
}

export interface ObjectStore {
  id: string;
  /** The environment the store belongs to; `null` for a shared store. */
  environment_id: string | null;
  /**
   * The reduced view: the caller reaches the store only through the
   * environment that owns it, or only through bucket grants. Members carry
   * a hostname and a status only, and everything operators use (credential,
   * ports, paths, jobs) is absent.
   */
  limited: boolean;
  /**
   * Reached through bucket grants only: the granted buckets and their
   * objects, nothing else. The owner, endpoint and members are absent too.
   */
  by_grant: boolean;
  name: string;
  slug?: string | null;
  provider: string;
  mode: StoreMode;
  region?: string | null;
  path_style?: boolean;
  version?: string | null;
  deployed_version?: string | null;
  runtime?: string | null;
  topology?: string | null;
  s3_port?: number | null;
  console_port?: number | null;
  endpoint?: string | null;
  credential_id?: string | null;
  status: string;
  last_error?: string | null;
  last_job_id?: string | null;
  created_at?: number;
  updated_at?: number;
  members: StoreMember[];
}

export type StoreTopology = "standalone" | "distributed";

export interface StoreInstallMember {
  server_id: string;
  /** One or more drives; distributed stores need the same count on every node. */
  data_paths: string[];
  endpoint_host?: string;
}

export interface StoreInstall {
  name: string;
  slug: string;
  provider?: string;
  version: string;
  runtime: "native" | "docker";
  topology: StoreTopology;
  require_mount: boolean;
  s3_port: number;
  console_port?: number;
  members: StoreInstallMember[];
  /** The environment the store belongs to; omit for a shared store. */
  environment_id?: string;
}

export interface StoreConnect {
  name: string;
  provider: string;
  endpoint: string;
  region: string;
  path_style: boolean;
  access_key: string;
  secret_key: string;
  bucket?: string;
  /** The environment the store belongs to; omit for a shared store. */
  environment_id?: string;
}

/** Take over the App Store `rustfs` container on a node as a managed store. */
export interface StoreAdopt {
  name: string;
  server_id: string;
  /** Default: the node's hostname. */
  endpoint_host?: string;
  /** The environment the store belongs to; omit for a shared store. */
  environment_id?: string;
}

export type KeyAccess = "read" | "readwrite";

/** An access key (service account) on a managed RustFS store. Never carries a secret. */
export interface StoreKey {
  access_key: string;
  name?: string | null;
  description?: string | null;
  /** RustFS account status, e.g. `on` / `off`. */
  status?: string | null;
  expiration?: string | null;
  /** Vault item holding the secret; only for Forge-issued keys. */
  vault_item_id?: string | null;
  bucket?: string | null;
  access?: string | null;
  managed_by_forge: boolean;
}

export type GrantStatus = "active" | "expired" | "revoked";

/** One person's access to one bucket of a store, through Forge only (no S3 key). */
export interface BucketGrant {
  id: string;
  store_id: string;
  /** The store's name. */
  store: string;
  user_id: string;
  /** The name the person signs in with. */
  user: string;
  bucket: string;
  access: KeyAccess;
  /** Principal of the person who issued it (`user:<id>`). */
  created_by: string;
  created_by_name: string | null;
  created_at: number;
  /** `null` when the grant never expires. */
  expires_at: number | null;
  never_expires: boolean;
  revoked_at: number | null;
  note: string;
  status: GrantStatus;
}

/** A grant as its holder sees it: what opens and until when, not who gave it or why. */
export type MyBucketGrant = Pick<BucketGrant, "id" | "store_id" | "store" | "bucket" | "access" | "expires_at" | "status">;

export interface BucketGrantCreate {
  /** User id, username or email. */
  user: string;
  bucket: string;
  access: KeyAccess;
  lifetime_minutes?: number;
  /** Epoch milliseconds. */
  expires_at?: number;
  note?: string;
}

export interface StoreKeyCreate {
  name: string;
  bucket: string;
  access: KeyAccess;
}

/** Response of create and rotate: the secret stays in the vault. */
export interface StoreKeyIssued {
  access_key: string;
  name?: string;
  bucket?: string;
  access?: string;
  vault_item_id?: string;
}

/** Names (never values) of the variables an attached bucket gives the service. */
export interface ServiceStorageEnv {
  endpoint: string;
  region: string;
  bucket: string;
  access_key: string;
  secret_key: string;
}

/** A bucket of a managed object store attached to a service. */
export interface ServiceStorageAttachment {
  /** The key limited to the bucket; identifies the attachment. */
  access_key: string;
  store_id: string;
  bucket: string;
  access: KeyAccess;
  /** Variable name prefix, e.g. `S3`. */
  prefix: string;
  vault_item_id: string;
  env: ServiceStorageEnv;
  /** Null when the store was deleted. */
  store_name: string | null;
  endpoint: string | null;
}

export interface ServiceStorageView {
  attachments: ServiceStorageAttachment[];
  /** May attach and detach (deploy on the environment; the store is checked on attach). */
  can_change: boolean;
}

export interface ServiceStorageAttach {
  store_id: string;
  bucket: string;
  create_bucket?: boolean;
  access?: KeyAccess;
  prefix?: string;
}

export interface StoreTest {
  ok: boolean;
  latency_ms?: number;
  buckets?: number;
  error?: string;
}

export interface Bucket {
  name: string;
  created?: number | string | null;
}

export interface StoreObject {
  key: string;
  size?: number | null;
  last_modified?: number | string | null;
  etag?: string | null;
}

export interface ObjectPage {
  objects: StoreObject[];
  prefixes: string[];
  next_token: string | null;
}

/** `facts` on `GET /api/v1/servers[/{id}]`. Every field may be missing. */
export interface NodeFacts {
  os?: string | null;
  os_version?: string | null;
  kernel?: string | null;
  arch?: string | null;
  cpus?: number | null;
  mem_total_bytes?: number | null;
  disk_total_bytes?: number | null;
  agent_version?: string | null;
  collected_at_ms?: number | null;
}

/** `GET /api/v1/tags?kind=…` entry. */
export interface TagCount {
  tag: string;
  count: number;
}

/** Checked facts: non-empty strings and positive finite numbers, else `undefined`. */
export interface CheckedFacts {
  os?: string;
  os_version?: string;
  kernel?: string;
  arch?: string;
  cpus?: number;
  mem_total_bytes?: number;
  disk_total_bytes?: number;
  agent_version?: string;
  collected_at_ms?: number;
}

/** A node's facts with every field type-checked (older servers send none). */
export function nodeFacts(n: Pick<Server, "facts"> | null | undefined): CheckedFacts {
  const raw: unknown = n?.facts;
  const f = raw && typeof raw === "object" && !Array.isArray(raw) ? (raw as Record<string, unknown>) : {};
  const str = (k: string) => {
    const v = f[k];
    return typeof v === "string" && v.trim() ? v.trim() : undefined;
  };
  const num = (k: string) => {
    const v = f[k];
    return typeof v === "number" && Number.isFinite(v) && v > 0 ? v : undefined;
  };
  return {
    os: str("os"),
    os_version: str("os_version"),
    kernel: str("kernel"),
    arch: str("arch"),
    cpus: num("cpus"),
    mem_total_bytes: num("mem_total_bytes"),
    disk_total_bytes: num("disk_total_bytes"),
    agent_version: str("agent_version"),
    collected_at_ms: num("collected_at_ms"),
  };
}

/** A node's tags, always an array of non-empty strings. */
export const nodeTags = (n: Pick<Server, "tags"> | null | undefined): string[] =>
  Array.isArray(n?.tags) ? n.tags.filter((t): t is string => typeof t === "string" && t.trim() !== "") : [];

/** AND match, case-insensitive: the server's `?tag=` semantics. */
export const hasAllTags = (tags: string[], wanted: string[]) =>
  wanted.every((w) => tags.some((t) => t.toLowerCase() === w.trim().toLowerCase()));

// ------------------------------------------------------------------ health policy
// Mirrors `forge_catalog::health` (the server refuses unknown fields).

/** Which section a check is in. */
export type HealthPurpose = "startup" | "readiness" | "liveness";

export interface HealthStatusRange {
  from: number;
  to: number;
}

/** Exactly one of `value` and `secret` (`vault:<id>`). */
export interface HealthHeader {
  name: string;
  value?: string;
  secret?: string;
}

export interface HttpProbe {
  type: "http";
  scheme: "http" | "https";
  /** Empty: the workload itself. */
  host?: string;
  allow_remote_host?: boolean;
  port: number;
  path: string;
  method: "GET" | "HEAD";
  headers?: HealthHeader[];
  expected_statuses: HealthStatusRange[];
  tls_verify: boolean;
  server_name?: string;
}

export interface TcpProbe {
  type: "tcp";
  host?: string;
  allow_remote_host?: boolean;
  port: number;
}

export interface ExecProbe {
  type: "exec";
  /** Program first, then its arguments; never passed to a shell. */
  argv: string[];
  expected_exit_codes: number[];
  /** Report the end of a failed run's output with the result (off by default: it can hold secrets). */
  show_output?: boolean;
}

export interface ShellProbe {
  type: "shell";
  interpreter: "sh" | "bash";
  script: string;
  expected_exit_codes: number[];
  /** Report the end of a failed run's output with the result (off by default: it can hold secrets). */
  show_output?: boolean;
}

export type HealthProbe = HttpProbe | TcpProbe | ExecProbe | ShellProbe;
export type HealthProbeType = HealthProbe["type"];

export interface HealthCheckSpec {
  name: string;
  enabled: boolean;
  required: boolean;
  probe: HealthProbe;
  period_secs: number;
  timeout_secs: number;
  initial_delay_secs: number;
  failure_threshold: number;
  success_threshold: number;
}

export interface HealthPolicy {
  /** Forge's default checks (unit/container running, TCP on the app's port). Absent = true; sent only when false. */
  default_checks?: boolean;
  startup?: HealthCheckSpec[];
  readiness?: HealthCheckSpec[];
  liveness?: HealthCheckSpec[];
}

export interface HealthCheckSummary {
  purpose: HealthPurpose;
  name: string;
  enabled: boolean;
  summary: string;
  /** Forge's effective default check (not stored in the policy). */
  default?: boolean;
}

export type InstanceHealthStatus = "healthy" | "degraded" | "unhealthy" | "unknown";

/** One check's latest result on one node. */
export interface InstanceCheck {
  purpose: HealthPurpose;
  name: string;
  /** passing | failing | unknown | pending | disabled; startup: waiting | succeeded | exhausted. */
  state: string;
  /** app_failure, timeout, connect_failed, ambiguous_port, node_offline, stale, unsupported_executor, permission_denied, location_unavailable, invalid_config, … */
  reason?: string | null;
  detail?: string | null;
  stale?: boolean;
  default?: boolean;
  /** When the node's last report arrived; null before the first one. */
  received_at?: number | null;
}

/** Health of the app on one node it is placed on. */
export interface InstanceHealth {
  server_id: string;
  status: InstanceHealthStatus;
  checks: InstanceCheck[];
}

export interface HealthWarning {
  code: string;
  server_id?: string | null;
  message: string;
}

export interface HealthFieldError {
  /** A path like `readiness[0].probe.headers[1].value`. */
  field: string;
  message: string;
}

export interface ServiceHealthView {
  policy: HealthPolicy;
  summaries: HealthCheckSummary[];
  /** The container's Docker HEALTHCHECK, run by Docker; offered for import. */
  legacy_healthcheck: HealthCheck | null;
  updated_at: number;
  /** May save HTTP and TCP checks (health.configure). */
  can_configure: boolean;
  /** May save exec and shell checks (health.command; MFA when required). */
  can_command: boolean;
  /** One per placement; empty on older servers. */
  instances: InstanceHealth[];
  warnings: HealthWarning[];
}

export interface HealthValidation {
  valid: boolean;
  errors: HealthFieldError[];
  summaries: HealthCheckSummary[];
  runs_code: boolean;
}

// ------------------------------------------------------------------ tunnels (D-100)

/** What a tunnel or grant points at: a service (app, stack service, database package), SSH on a node or a raw port. */
export interface TunnelTarget {
  /** `app` | `stack` | `package` | `ssh` | `port`. */
  kind: string;
  /** App name, `stack/service`, package id, or the node id for `ssh` and `port`. */
  ref: string;
  /** Human label from the server, e.g. `web/site`; falls back to `ref`. */
  label: string;
}

export interface Tunnel {
  id: string;
  user: string;
  user_id: string;
  server_id: string;
  server: string;
  target: TunnelTarget;
  port: number | null;
  role: string | null;
  bind: string;
  opened_at: number | null;
  expires_at: number | null;
  closed_at: number | null;
  close_reason: string | null;
  bytes_up: number;
  bytes_down: number;
  connections: number;
}

export interface TunnelGrant {
  id: string;
  user: string;
  user_id: string;
  target: TunnelTarget;
  server: string | null;
  server_id: string | null;
  /** `*` or a comma-separated list. */
  ports: string;
  expires_at: number | null;
  revoked_at: number | null;
  created_by: string;
  created_at: number | null;
  note: string | null;
}

/** One port a service listens on, on one node (`GET /api/v1/service-ports`). */
export interface ServicePort {
  kind: string;
  environment: string | null;
  name: string;
  label: string;
  server_id: string;
  server: string;
  port: number;
  role: string | null;
  dial_ip: string | null;
}

// ------------------------------------------------------------------ containers (inspect)

/** One bind mount or volume of a container. */
export interface ContainerMount {
  type?: string;
  source?: string;
  destination?: string;
  mode?: string;
  rw?: boolean;
}

/** A published port, flattened from Docker's `NetworkSettings.Ports` map. */
export interface ContainerPort {
  /** e.g. `80/tcp` */
  container: string;
  /** `0.0.0.0:8080`; empty when the port is exposed but not published. */
  host: string;
}

/** One `stats` sample. Every field is optional: older agents send less. */
export interface ContainerStats {
  cpu_percent?: number;
  memory_bytes?: number;
  memory_limit?: number;
}

/** An environment variable revealed on request (`reveal_env`). Never cached. */
export interface ContainerEnv {
  key: string;
  value: string;
}

/**
 * `inspect` result, normalized: every field is present, lists are arrays and
 * unknowns are `undefined` or `""`, whatever the agent version sent.
 */
export interface ContainerInspect {
  id: string;
  name: string;
  image: string;
  /** Docker status: running, exited, paused, restarting, created, dead… */
  state: string;
  running: boolean;
  /** Docker's health status, when the image has a HEALTHCHECK. */
  health?: string;
  exit_code?: number;
  pid?: number;
  /** Epoch ms. */
  started_at?: number;
  finished_at?: number;
  created_at?: number;
  restart_policy: string;
  ports: ContainerPort[];
  mounts: ContainerMount[];
  networks: string[];
  labels: Record<string, string>;
  env_keys: string[];
  /** Image content id (`sha256:…`), when the agent sends it. */
  image_id?: string;
  /** `Config.Entrypoint` and `Config.Cmd`, when the agent sends them. */
  entrypoint?: string[];
  command?: string[];
  /** IPv4 per network, when the agent sends `networks` as an object. */
  network_ips?: { name: string; ip: string }[];
  /** Only with `reveal_env: true`. */
  env?: ContainerEnv[];
}
