// Docker Swarm of a Swarm-mode cluster (D-154): members, live nodes and
// quorum. Server: `src/apps/forge-server/src/api/swarm.rs`. Every reader
// normalizes the JSON so an older server or a missing field never crashes.
import { useQuery } from "@tanstack/react-query";

import { api, get, post } from "./client";
import { useAction } from "./hooks";
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
