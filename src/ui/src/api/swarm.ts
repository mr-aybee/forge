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
