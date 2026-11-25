// Docker Swarm of a Swarm-mode cluster (D-154): members, live nodes and
// quorum. Server: `src/apps/forge-server/src/api/swarm.rs`. Every reader
// normalizes the JSON so an older server or a missing field never crashes.
import { useQuery } from "@tanstack/react-query";

import { api, get, post } from "./client";
import { fleetKeys, useAction } from "./hooks";
import type { ClusterMode, Job } from "./types";

const enc = encodeURIComponent;
const base = (cluster: string) => `/api/v1/clusters/${enc(cluster)}/swarm`;

export type SwarmRole = "manager" | "worker";
export type SwarmAvailability = "active" | "pause" | "drain";
export type SwarmMemberState = "joining" | "joined" | "leaving" | "failed";
export type QuorumStatus = "ok" | "single" | "even";

export interface SwarmInfo {
  cluster_id: string;
  swarm_id: string;
  port?: number;
  default_addr_pool: string[];
  autolock: boolean;
  updated_at?: number;
}

/** Forge's record of a node's membership. */
export interface SwarmMember {
  server_id: string;
  cluster_id: string;
  node_id?: string;
  role: SwarmRole;
  advertise_addr?: string;
  manager_addr?: string;
  state: SwarmMemberState;
  error?: string;
  updated_at?: number;
  hostname?: string;
}

/** A node as a manager reports it right now. */
export interface SwarmNode {
  id: string;
  hostname: string;
  role: SwarmRole;
  availability: SwarmAvailability;
  state: string;
  addr?: string;
  manager_status?: string;
  leader: boolean;
  engine_version?: string;
  labels: Record<string, string>;
}

export interface SwarmQuorum {
  managers: number;
  tolerates: number;
  status: QuorumStatus;
}

export interface SwarmView {
  cluster_id: string;
  mode: ClusterMode;
  swarm: SwarmInfo | null;
  members: SwarmMember[];
  /** `null` when no manager answered (see `nodes_error`) or there is no swarm. */
  nodes: SwarmNode[] | null;
  nodes_error: string | null;
  quorum: SwarmQuorum;
}

export interface SwarmJoinBody {
  server_id: string;
  role: SwarmRole;
  advertise_addr?: string;
  default_addr_pool?: string[];
  autolock?: boolean;
}

export interface SwarmNodePatch {
  role?: SwarmRole;
  availability?: SwarmAvailability;
  labels?: Record<string, string>;
}

export interface SwarmJoinStarted {
  job: Job;
  installs: Job[];
}

export const swarmKeys = {
  all: ["swarm"] as const,
  view: (cluster: string) => ["swarm", cluster] as const,
  service: (id: string) => ["swarm", "service", id] as const,
  serviceLogs: (id: string, tail: number) => ["swarm", "service", id, "logs", tail] as const,
  stacks: (cluster: string) => ["swarm", cluster, "stacks"] as const,
  networks: (cluster: string) => ["swarm", cluster, "networks"] as const,
  objects: (cluster: string, kind: SwarmObjectKind) => ["swarm", cluster, "objects", kind] as const,
  addresses: (cluster: string, server: string) => ["swarm", cluster, "addresses", server] as const,
  convert: (cluster: string, to: ClusterMode) => ["swarm", "convert", cluster, to] as const,
  stack: (id: string) => ["swarm", "stack", id] as const,
  stackLogs: (id: string, service: string, tail: number) => ["swarm", "stack", id, "logs", service, tail] as const,
};

type Rec = Record<string, unknown>;
const rec = (v: unknown): Rec => (v && typeof v === "object" && !Array.isArray(v) ? (v as Rec) : {});
const str = (v: unknown) => (typeof v === "string" && v ? v : undefined);
const num = (v: unknown) => (typeof v === "number" && Number.isFinite(v) ? v : undefined);
const arr = (v: unknown): unknown[] => (Array.isArray(v) ? v : []);
const oneOf = <T extends string>(v: unknown, all: readonly T[], d: T): T => (all.includes(v as T) ? (v as T) : d);
const ROLES = ["manager", "worker"] as const;

function normMember(v: unknown): SwarmMember | null {
  const o = rec(v);
  const server_id = str(o.server_id);
  if (!server_id) return null;
  return {
    server_id,
    cluster_id: str(o.cluster_id) ?? "",
    node_id: str(o.node_id),
    role: oneOf(o.role, ROLES, "worker"),
    advertise_addr: str(o.advertise_addr),
    manager_addr: str(o.manager_addr),
    state: oneOf(o.state, ["joining", "joined", "leaving", "failed"] as const, "failed"),
    error: str(o.error),
    updated_at: num(o.updated_at),
    hostname: str(o.hostname),
  };
}

function normNode(v: unknown): SwarmNode | null {
  const o = rec(v);
  const id = str(o.id);
  if (!id) return null;
  const labels: Record<string, string> = {};
  for (const [k, val] of Object.entries(rec(o.labels))) if (typeof val === "string") labels[k] = val;
  return {
    id,
    hostname: str(o.hostname) ?? id,
    role: oneOf(o.role, ROLES, "worker"),
    availability: oneOf(o.availability, ["active", "pause", "drain"] as const, "active"),
    state: str(o.state) ?? "unknown",
    addr: str(o.addr),
    manager_status: str(o.manager_status),
    leader: o.leader === true,
    engine_version: str(o.engine_version),
    labels,
  };
}

export function normSwarmView(v: unknown, cluster: string): SwarmView {
  const o = rec(v);
  const s = rec(o.swarm);
  const q = rec(o.quorum);
  const swarm_id = str(s.swarm_id);
  return {
    cluster_id: str(o.cluster_id) ?? cluster,
    mode: o.mode === "swarm" ? "swarm" : "default",
    swarm: swarm_id
      ? {
          cluster_id: str(s.cluster_id) ?? cluster,
          swarm_id,
          port: num(s.port),
          default_addr_pool: arr(s.default_addr_pool).filter((x): x is string => typeof x === "string"),
          autolock: s.autolock === true,
          updated_at: num(s.updated_at),
        }
      : null,
    members: arr(o.members).map(normMember).filter((m): m is SwarmMember => m !== null),
    nodes: Array.isArray(o.nodes) ? o.nodes.map(normNode).filter((n): n is SwarmNode => n !== null) : null,
    nodes_error: str(o.nodes_error) ?? null,
    quorum: {
      managers: num(q.managers) ?? 0,
      tolerates: num(q.tolerates) ?? 0,
      status: oneOf(q.status, ["ok", "single", "even"] as const, "ok"),
    },
  };
}

const busy = (v: SwarmView | undefined) => !!v?.members.some((m) => m.state === "joining" || m.state === "leaving");

/** The cluster's swarm; polls every 3 s while a member is joining or leaving. */
export const useSwarm = (cluster: string, enabled = true) =>
  useQuery({
    queryKey: swarmKeys.view(cluster),
    queryFn: () => get<unknown>(base(cluster)).then((v) => normSwarmView(v, cluster)),
    enabled: enabled && !!cluster,
    retry: false,
    staleTime: 10_000,
    refetchInterval: (q) => (busy(q.state.data) ? 3_000 : 30_000),
  });

/** Joins a node (starts `swarm.member`, plus Docker installs when missing). MFA step-up via the client. */
export const useJoinSwarm = (cluster: string) =>
  useAction((b: SwarmJoinBody) => post<unknown>(`${base(cluster)}/members`, b).then(normJoin), [swarmKeys.view(cluster), ["jobs"]]);

function normJoin(v: unknown): SwarmJoinStarted {
  const o = rec(v);
  return { job: rec(o.job) as unknown as Job, installs: arr(o.installs) as Job[] };
}

/** Changes a node's role, availability or labels (labels replace all). */
export const useUpdateSwarmNode = (cluster: string) =>
  useAction(
    ({ server, ...b }: SwarmNodePatch & { server: string }) => api<unknown>("PATCH", `${base(cluster)}/members/${enc(server)}`, b).then(normNode),
    [swarmKeys.view(cluster)],
  );

/** Leaves the swarm (starts a job); `force` also for a failed or unreachable node. */
export const useLeaveSwarm = (cluster: string) =>
  useAction(
    ({ server, force }: { server: string; force: boolean }) => api<Job>("DELETE", `${base(cluster)}/members/${enc(server)}?force=${force}`),
    [swarmKeys.view(cluster), ["jobs"]],
  );

/** Rotates the worker and/or manager join token. */
export const useRotateSwarmTokens = (cluster: string) =>
  useAction((b: { worker: boolean; manager: boolean }) => post<void>(`${base(cluster)}/tokens/rotate`, b), [swarmKeys.view(cluster)]);

// ------------------------------------------------------------------ services in a swarm

export type SwarmServiceMode = "replicated" | "global";
export type SwarmRestartCondition = "any" | "on-failure" | "none";
export type SwarmUpdateOrder = "start-first" | "stop-first";
export type SwarmFailureAction = "rollback" | "pause" | "continue";
export type SwarmPublishMode = "ingress" | "host";

/** `swarm` of a container service in a Swarm cluster (server: `forge_catalog::swarm_service::SwarmSettings`). */
export interface SwarmSettings {
  /** Docker service name; empty: `<environment>-<service>`. Set when the service is first created. */
  name: string;
  mode: SwarmServiceMode;
  /** Replicated mode only; 1 to 1000. */
  replicas: number;
  /** `node.role==worker`, `node.labels.zone!=a`, … */
  constraints: string[];
  /** `node.labels.<x>`, at most 4. */
  spread: string[];
  /** 0 = no limit. */
  max_per_node: number;
  restart: { condition: SwarmRestartCondition; max_attempts: number; delay_secs: number };
  update: { parallelism: number; delay_secs: number; order: SwarmUpdateOrder; failure_action: SwarmFailureAction; monitor_secs: number; max_failure_ratio: number };
  publish: SwarmPublishMode;
}

export const defaultSwarmSettings = (): SwarmSettings => ({
  name: "",
  mode: "replicated",
  replicas: 1,
  constraints: [],
  spread: [],
  max_per_node: 0,
  restart: { condition: "any", max_attempts: 0, delay_secs: 5 },
  update: { parallelism: 1, delay_secs: 0, order: "stop-first", failure_action: "pause", monitor_secs: 10, max_failure_ratio: 0 },
  publish: "ingress",
});

const strs = (v: unknown) => arr(v).filter((x): x is string => typeof x === "string");

/** Settings with every field defaulted, so an older or partial spec never breaks the editor. */
export function normSwarmSettings(v: unknown): SwarmSettings {
  const o = rec(v);
  const d = defaultSwarmSettings();
  const r = rec(o.restart);
  const u = rec(o.update);
  return {
    name: typeof o.name === "string" ? o.name : "",
    mode: oneOf(o.mode, ["replicated", "global"] as const, d.mode),
    replicas: num(o.replicas) ?? d.replicas,
    constraints: strs(o.constraints),
    spread: strs(o.spread),
    max_per_node: num(o.max_per_node) ?? 0,
    restart: {
      condition: oneOf(r.condition, ["any", "on-failure", "none"] as const, d.restart.condition),
      max_attempts: num(r.max_attempts) ?? 0,
      delay_secs: num(r.delay_secs) ?? d.restart.delay_secs,
    },
    update: {
      parallelism: num(u.parallelism) ?? d.update.parallelism,
      delay_secs: num(u.delay_secs) ?? 0,
      order: oneOf(u.order, ["start-first", "stop-first"] as const, d.update.order),
      failure_action: oneOf(u.failure_action, ["rollback", "pause", "continue"] as const, d.update.failure_action),
      monitor_secs: num(u.monitor_secs) ?? d.update.monitor_secs,
      max_failure_ratio: num(u.max_failure_ratio) ?? 0,
    },
    publish: oneOf(o.publish, ["ingress", "host"] as const, d.publish),
  };
}

export interface SwarmTask {
  id: string;
  node_id?: string;
  slot?: number;
  state: string;
  desired_state?: string;
  error?: string;
  image?: string;
  /** RFC 3339 from Docker, or epoch ms. */
  updated_at?: string | number;
}

export interface SwarmServiceStatus {
  exists: boolean;
  id?: string;
  /** Docker service name; task containers are `<name>.<replica>.<task id>`. */
  name?: string;
  mode?: string;
  desired: number;
  running: number;
  /** `updating`, `completed`, `paused`, `rollback_started`, …; empty before the first update. */
  update_state?: string;
  update_message?: string;
  image?: string;
  tasks: SwarmTask[];
}

export interface ServiceSwarmView {
  cluster_id: string;
  /** Docker service name. */
  name: string;
  settings: SwarmSettings;
  status: SwarmServiceStatus | null;
  status_error: string | null;
  /** High-availability readiness of the settings and swarm; older servers send none (score 0, no checks). */
  ha: SwarmHa;
}

export interface SwarmHaCheck {
  name: string;
  ok: boolean;
  /** What to change when the check fails. */
  hint: string;
}

export interface SwarmHa {
  /** 0 to 100. */
  score: number;
  checks: SwarmHaCheck[];
}

function normHa(v: unknown): SwarmHa {
  const o = rec(v);
  const score = num(o.score) ?? 0;
  return {
    score: Math.max(0, Math.min(100, Math.round(score))),
    checks: arr(o.checks)
      .map((c) => {
        const x = rec(c);
        const name = str(x.name);
        return name ? { name, ok: x.ok === true, hint: str(x.hint) ?? "" } : null;
      })
      .filter((c): c is SwarmHaCheck => c !== null),
  };
}

export type ServiceSwarmAction = { kind: "deploy" } | { kind: "scale"; replicas: number } | { kind: "restart" } | { kind: "rollback" } | { kind: "remove" };

function normTask(v: unknown): SwarmTask | null {
  const o = rec(v);
  const id = str(o.id);
  if (!id) return null;
  const at = o.updated_at;
  return {
    id,
    node_id: str(o.node_id),
    slot: num(o.slot),
    state: str(o.state) ?? "unknown",
    desired_state: str(o.desired_state),
    error: str(o.error),
    image: str(o.image),
    updated_at: typeof at === "string" || typeof at === "number" ? at : undefined,
  };
}

export function normServiceSwarm(v: unknown): ServiceSwarmView {
  const o = rec(v);
  const s = o.status && typeof o.status === "object" ? rec(o.status) : null;
  return {
    cluster_id: str(o.cluster_id) ?? "",
    name: str(o.name) ?? "",
    settings: normSwarmSettings(o.settings),
    status: s
      ? {
          exists: s.exists === true,
          id: str(s.id),
          mode: str(s.mode),
          desired: num(s.desired) ?? 0,
          running: num(s.running) ?? 0,
          update_state: str(s.update_state),
          update_message: str(s.update_message),
          image: str(s.image),
          tasks: arr(s.tasks).map(normTask).filter((t): t is SwarmTask => t !== null),
        }
      : null,
    status_error: str(o.status_error) ?? null,
    ha: normHa(o.ha),
  };
}

/** The service is converging: a rollout or rollback runs, or fewer tasks run than asked for. */
export const swarmServiceBusy = (v: ServiceSwarmView | undefined) => {
  const s = v?.status;
  if (!s?.exists) return false;
  return s.update_state === "updating" || s.update_state === "rollback_started" || s.running < s.desired;
};

const svcBase = (id: string) => `/api/v1/services/${enc(id)}/swarm`;

/** A service's Docker Swarm service; polls every 3 s while converging, else every 30 s. */
export const useServiceSwarm = (id: string, enabled = true) =>
  useQuery({
    queryKey: swarmKeys.service(id),
    queryFn: () => get<unknown>(svcBase(id)).then(normServiceSwarm),
    enabled: enabled && !!id,
    retry: false,
    staleTime: 5_000,
    refetchInterval: (q) => (swarmServiceBusy(q.state.data) ? 3_000 : 30_000),
  });

/** Deploy, scale, restart, roll back or remove the Swarm service (starts a `swarm.service` job). */
export const useServiceSwarmAction = (id: string) =>
  useAction((b: ServiceSwarmAction) => post<Job>(svcBase(id), b), [swarmKeys.service(id), fleetKeys.services, fleetKeys.serviceAny, ["jobs"]]);

/** The last `tail` log lines of every task; fetched on demand, never polled. */
export const useServiceSwarmLogs = (id: string, tail: number, enabled = true) =>
  useQuery({
    queryKey: swarmKeys.serviceLogs(id, tail),
    queryFn: () => get<unknown>(`${svcBase(id)}/logs?tail=${tail}`).then((v) => str(rec(v).logs) ?? ""),
    enabled: enabled && !!id,
    retry: false,
    staleTime: 5_000,
  });

// ------------------------------------------------------------------ cluster-wide swarm resources

const bool = (v: unknown) => v === true;

/** A stack of this cluster's environments, deployed (or to deploy) into the swarm. */
export interface SwarmStackRef {
  id: string;
  name: string;
  environment_id: string;
  environment: string;
  revision: number;
  deployed_revision: number;
}

export interface SwarmNetwork {
  id: string;
  name: string;
  driver: string;
  scope: string;
  subnets: string[];
  encrypted: boolean;
  attachable: boolean;
  /** The routing mesh network. */
  ingress: boolean;
  managed: boolean;
  services: string[];
}

export type SwarmObjectKind = "secret" | "config";

export interface SwarmObject {
  id: string;
  kind: SwarmObjectKind;
  /** Logical name; versions share it. */
  name: string;
  /** Docker object name. */
  docker_name: string;
  version: number;
  created_at?: number;
  used_by: string[];
  managed: boolean;
}

export type SwarmObjectBody = { kind: SwarmObjectKind; name: string; value?: string; vault_item_id?: string };

export type AddressSource = "mesh" | "primary" | "interface" | "agent";
export interface SwarmAddress {
  address: string;
  source: AddressSource;
  interface?: string;
}

export interface ConvertCheck {
  name: string;
  ok: boolean;
  detail: string;
}
export interface ConvertReport {
  from: ClusterMode;
  to: ClusterMode;
  ok: boolean;
  checks: ConvertCheck[];
}

const normStack = (v: unknown): SwarmStackRef | null => {
  const o = rec(v);
  const id = str(o.id);
  return id
    ? {
        id,
        name: str(o.name) ?? id,
        environment_id: str(o.environment_id) ?? "",
        environment: str(o.environment) ?? "",
        revision: num(o.revision) ?? 0,
        deployed_revision: num(o.deployed_revision) ?? 0,
      }
    : null;
};

const normNetwork = (v: unknown): SwarmNetwork | null => {
  const o = rec(v);
  const id = str(o.id);
  return id
    ? {
        id,
        name: str(o.name) ?? id,
        driver: str(o.driver) ?? "",
        scope: str(o.scope) ?? "",
        subnets: strs(o.subnets),
        encrypted: bool(o.encrypted),
        attachable: bool(o.attachable),
        ingress: bool(o.ingress),
        managed: bool(o.managed),
        services: strs(o.services),
      }
    : null;
};

const normObject = (v: unknown): SwarmObject | null => {
  const o = rec(v);
  const docker_name = str(o.docker_name);
  return docker_name
    ? {
        id: str(o.id) ?? docker_name,
        kind: oneOf(o.kind, ["secret", "config"] as const, "secret"),
        name: str(o.name) ?? docker_name,
        docker_name,
        version: num(o.version) ?? 0,
        created_at: num(o.created_at),
        used_by: strs(o.used_by),
        managed: bool(o.managed),
      }
    : null;
};

const normAddress = (v: unknown): SwarmAddress | null => {
  const o = rec(v);
  const address = str(o.address);
  return address ? { address, source: oneOf(o.source, ["mesh", "primary", "interface", "agent"] as const, "agent"), interface: str(o.interface) } : null;
};

export function normConvert(v: unknown): ConvertReport {
  const o = rec(v);
  const mode = (x: unknown): ClusterMode => (x === "swarm" ? "swarm" : "default");
  return {
    from: mode(o.from),
    to: mode(o.to),
    ok: bool(o.ok),
    checks: arr(o.checks)
      .map((c) => {
        const x = rec(c);
        const name = str(x.name);
        return name ? { name, ok: bool(x.ok), detail: str(x.detail) ?? "" } : null;
      })
      .filter((c): c is ConvertCheck => c !== null),
  };
}

const listOf = <T>(path: string, norm: (v: unknown) => T | null) => get<unknown>(path).then((v) => arr(v).map(norm).filter((x): x is T => x !== null));

/** Stacks of this cluster's environments, deployed to the swarm. */
export const useSwarmStacks = (cluster: string, enabled = true) =>
  useQuery({
    queryKey: swarmKeys.stacks(cluster),
    queryFn: () => listOf(`${base(cluster)}/stacks`, normStack),
    enabled: enabled && !!cluster,
    retry: false,
    staleTime: 10_000,
    refetchInterval: 30_000,
  });

export const useSwarmNetworks = (cluster: string, enabled = true) =>
  useQuery({ queryKey: swarmKeys.networks(cluster), queryFn: () => listOf(`${base(cluster)}/networks`, normNetwork), enabled: enabled && !!cluster, retry: false, staleTime: 10_000 });

/** Creates the attachable encrypted overlay `forge-shared`. */
export const useCreateSharedNetwork = (cluster: string) => useAction(() => post<void>(`${base(cluster)}/networks/shared`), [swarmKeys.networks(cluster)]);

export const useSwarmObjects = (cluster: string, kind: SwarmObjectKind, enabled = true) =>
  useQuery({
    queryKey: swarmKeys.objects(cluster, kind),
    queryFn: () => listOf(`${base(cluster)}/objects?kind=${kind}`, normObject),
    enabled: enabled && !!cluster,
    retry: false,
    staleTime: 10_000,
  });

/** Creates version 1 of a secret or config, or rotates it to the next version. */
export const useCreateSwarmObject = (cluster: string) =>
  useAction(
    (b: SwarmObjectBody) =>
      post<unknown>(`${base(cluster)}/objects`, b).then((v) => {
        const o = rec(v);
        return { docker_name: str(o.docker_name) ?? b.name, version: num(o.version) };
      }),
    [swarmKeys.all],
  );

/** Removes unused older versions; returns the removed Docker names. */
export const usePruneSwarmObjects = (cluster: string) =>
  useAction((kind: SwarmObjectKind) => post<unknown>(`${base(cluster)}/objects/prune`, { kind }).then((v) => strs(rec(v).removed)), [swarmKeys.all]);

export const useRemoveSwarmObject = (cluster: string) =>
  useAction(
    ({ kind, docker_name }: { kind: SwarmObjectKind; docker_name: string }) => api<void>("DELETE", `${base(cluster)}/objects/${kind}/${enc(docker_name)}`),
    [swarmKeys.all],
  );

/** Addresses a node can advertise, best first (mesh preferred). */
export const useSwarmAddresses = (cluster: string, server: string) =>
  useQuery({
    queryKey: swarmKeys.addresses(cluster, server),
    queryFn: () => listOf(`${base(cluster)}/addresses?server_id=${enc(server)}`, normAddress),
    enabled: !!cluster && !!server,
    retry: false,
    staleTime: 30_000,
  });

/** The read-only check before a mode conversion; never cached across opens. */
export const useConvertCheck = (cluster: string, to: ClusterMode) =>
  useQuery({
    queryKey: swarmKeys.convert(cluster, to),
    queryFn: () => get<unknown>(`/api/v1/clusters/${enc(cluster)}/convert?to=${to}`).then(normConvert),
    retry: false,
    gcTime: 0,
  });

export const useConvertCluster = (cluster: string) =>
  useAction((to: ClusterMode) => post<unknown>(`/api/v1/clusters/${enc(cluster)}/convert`, { to }).then(normConvert), [["clusters"], swarmKeys.all]);

// ------------------------------------------------------------------ a stack in the swarm

export interface SwarmPort {
  published?: number;
  target?: number;
  protocol: string;
  mode: string;
}

export interface SwarmStackService {
  name: string;
  docker_name: string;
  id?: string;
  image?: string;
  mode?: string;
  desired: number;
  running: number;
  ports: SwarmPort[];
  update_state?: string;
  update_message?: string;
  /** Attached to `forge-shared`. */
  shared: boolean;
  tasks: SwarmTask[];
}

export interface StackFinding {
  service: string;
  severity: "blocked" | "privileged" | "info";
  message: string;
}

export interface StackSwarmView {
  cluster_id: string;
  namespace: string;
  revision: number;
  deployed_revision: number;
  status: { namespace: string; services: SwarmStackService[] } | null;
  status_error: string | null;
  findings: StackFinding[];
  findings_error: string | null;
  privileged: boolean;
}

function normStackService(v: unknown): SwarmStackService | null {
  const o = rec(v);
  const name = str(o.name);
  if (!name) return null;
  return {
    name,
    docker_name: str(o.docker_name) ?? name,
    id: str(o.id),
    image: str(o.image),
    mode: str(o.mode),
    desired: num(o.desired) ?? 0,
    running: num(o.running) ?? 0,
    ports: arr(o.ports).map((p) => {
      const x = rec(p);
      return { published: num(x.published), target: num(x.target), protocol: str(x.protocol) ?? "tcp", mode: str(x.mode) ?? "ingress" };
    }),
    update_state: str(o.update_state),
    update_message: str(o.update_message),
    shared: bool(o.shared),
    tasks: arr(o.tasks).map(normTask).filter((t): t is SwarmTask => t !== null),
  };
}

export function normStackSwarm(v: unknown): StackSwarmView {
  const o = rec(v);
  const st = o.status && typeof o.status === "object" ? rec(o.status) : null;
  return {
    cluster_id: str(o.cluster_id) ?? "",
    namespace: str(o.namespace) ?? "",
    revision: num(o.revision) ?? 0,
    deployed_revision: num(o.deployed_revision) ?? 0,
    status: st ? { namespace: str(st.namespace) ?? "", services: arr(st.services).map(normStackService).filter((x): x is SwarmStackService => x !== null) } : null,
    status_error: str(o.status_error) ?? null,
    findings: arr(o.findings)
      .map((f) => {
        const x = rec(f);
        const message = str(x.message);
        return message ? { service: str(x.service) ?? "", severity: oneOf(x.severity, ["blocked", "privileged", "info"] as const, "info"), message } : null;
      })
      .filter((x): x is StackFinding => x !== null),
    findings_error: str(o.findings_error) ?? null,
    privileged: bool(o.privileged),
  };
}

const stackBusy = (v: StackSwarmView | undefined) =>
  !!v?.status?.services.some((s) => s.update_state === "updating" || s.update_state === "rollback_started" || s.running < s.desired);

const stackBase = (id: string) => `/api/v1/stacks/${enc(id)}/swarm`;

/** A stack's live status in the swarm; polls every 3 s while converging. */
export const useStackSwarm = (id: string, enabled = true) =>
  useQuery({
    queryKey: swarmKeys.stack(id),
    queryFn: () => get<unknown>(stackBase(id)).then(normStackSwarm),
    enabled: enabled && !!id,
    retry: false,
    staleTime: 5_000,
    refetchInterval: (q) => (stackBusy(q.state.data) ? 3_000 : 30_000),
  });

/** Deploys (or redeploys) the stack to the swarm; returns the `swarm.stack` job. */
export const useDeploySwarmStack = (id: string) =>
  useAction((revision?: number) => post<Job>(`${stackBase(id)}/deploy`, revision === undefined ? {} : { revision }), [swarmKeys.all, ["stacks"], ["jobs"]]);

/** Removes the stack's services from the swarm; returns the job. */
export const useRemoveSwarmStack = (id: string) => useAction(() => post<Job>(`${stackBase(id)}/remove`), [swarmKeys.all, ["stacks"], ["jobs"]]);

export type StackServiceAction = { kind: "scale"; replicas: number } | { kind: "restart" } | { kind: "rollback" };

export const useStackServiceAction = (id: string) =>
  useAction(({ service, ...b }: StackServiceAction & { service: string }) => post<void>(`${stackBase(id)}/services/${enc(service)}`, b), [swarmKeys.stack(id)]);

/** The last `tail` lines of one service; fetched on demand. */
export const useStackServiceLogs = (id: string, service: string, tail: number) =>
  useQuery({
    queryKey: swarmKeys.stackLogs(id, service, tail),
    queryFn: () => get<unknown>(`${stackBase(id)}/services/${enc(service)}/logs?tail=${tail}`).then((v) => str(rec(v).logs) ?? ""),
    enabled: !!id && !!service,
    retry: false,
    staleTime: 5_000,
  });
