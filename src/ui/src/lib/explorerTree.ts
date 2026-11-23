/**
 * Pure tree logic for the Service Explorer (issue #74): build the
 * Service → Environment → Service hierarchy from the flat API lists,
 * filter it while keeping ancestry, and flatten the visible rows for
 * keyboard navigation. No React, no I/O.
 */
import type { Project, Service, ServiceInstance, Environment, Server } from "../api/types";
import { liveState, type LiveState } from "../components/InstanceState";

/** Running state of a service or placement, kept apart from rollout state. */
export type RunHealth = "ok" | "warn" | "bad" | "stopped" | "unknown";
/** Health-check status of one placement, from `GET /services/health-summary`. */
export type CheckStatus = "healthy" | "degraded" | "unhealthy" | "unknown";
/** `serviceId → serverId → status`. */
export type HealthMap = ReadonlyMap<string, ReadonlyMap<string, CheckStatus>>;
/** Rollout state: whether a deploy is in flight or failed. Never merged into `RunHealth`. */
export type Rollout = "idle" | "deploying" | "failed";

export interface TreePlacement {
  key: string;
  serverId: string;
  nodeName: string;
  instance: ServiceInstance;
  /** What the node last reported (running, stopped, …), the same as the service page's State column. */
  live: LiveState;
  /** Health-check status, when the summary has one for this node. */
  check?: CheckStatus;
  health: RunHealth;
}
export interface TreeService {
  kind: "service";
  key: string;
  id: string;
  name: string;
  runtime: string;
  service: Service;
  placements: TreePlacement[];
  health: RunHealth;
  rollout: Rollout;
}
export interface TreeEnv {
  kind: "env";
  key: string;
  id: string;
  name: string;
  clusterId: string | null;
  protected: boolean;
  services: TreeService[];
  health: RunHealth;
}
export interface TreeProject {
  kind: "project";
  key: string;
  /** `null` for the synthetic **Unassigned** bucket of legacy environments. */
  id: string | null;
  name: string;
  description: string;
  envs: TreeEnv[];
  health: RunHealth;
}
export type TreeNode = TreeProject | TreeEnv | TreeService;

export const UNASSIGNED_KEY = "project:unassigned";

/**
 * One dot per placement. A stopped service is `stopped`, not a failure, even
 * though its checks fail; a failed unit is `bad`. While it runs, health checks
 * decide when they have evidence, otherwise the reported state.
 */
export function placementHealth(i: ServiceInstance, check?: CheckStatus, live: LiveState = liveState(i, undefined)): RunHealth {
  if (live.kind === "stopped" || live.kind === "not-deployed") return "stopped";
  if (live.kind === "failed") return "bad";
  if (live.kind === "node-offline") return "unknown";
  if (check === "healthy") return "ok";
  if (check === "degraded") return "warn";
  if (check === "unhealthy") return "bad";
  if (i.state === "failed") return "bad";
  if (i.state === "active") return "ok";
  return "unknown";
}

export function placementRollout(i: ServiceInstance): Rollout {
  if (i.status === "failed") return "failed";
  if (i.status.endsWith("ing")) return "deploying";
  return "idle";
}

/** Worst child wins; no children is `unknown`. */
export function rollupHealth(hs: RunHealth[]): RunHealth {
  if (hs.includes("bad")) return "bad";
  if (hs.includes("warn")) return "warn";
  if (hs.length > 0 && hs.every((h) => h === "ok")) return "ok";
  if (hs.length > 0 && hs.every((h) => h === "stopped")) return "stopped";
  return "unknown";
}

function rollupRollout(rs: Rollout[]): Rollout {
  if (rs.includes("failed")) return "failed";
  if (rs.includes("deploying")) return "deploying";
  return "idle";
}

const byName = <T extends { name: string }>(a: T, b: T) => a.name.localeCompare(b.name);

export function buildTree(projects: Project[], environments: Environment[], services: Service[], servers: Server[], health: HealthMap = new Map()): TreeProject[] {
  const nodeName = new Map(servers.map((s) => [s.id, s.display_name || s.hostname]));
  const nodeById = new Map(servers.map((s) => [s.id, s]));
  const servicesByEnv = new Map<string, Service[]>();
  for (const a of services) servicesByEnv.set(a.environment_id, [...(servicesByEnv.get(a.environment_id) ?? []), a]);

  const service = (a: Service): TreeService => {
    // A service appears once; each instance is a placement on that one row.
    const placements = a.instances
      .map((i): TreePlacement => {
        const live = liveState(i, nodeById.get(i.server_id));
        const check = health.get(a.id)?.get(i.server_id);
        return { key: `placement:${a.id}:${i.server_id}`, serverId: i.server_id, nodeName: nodeName.get(i.server_id) ?? i.server_id.slice(0, 8), instance: i, live, check, health: placementHealth(i, check, live) };
      })
      .sort((x, y) => (x.instance.role === y.instance.role ? x.nodeName.localeCompare(y.nodeName) : x.instance.role === "active" ? -1 : 1));
    return {
      kind: "service",
      key: `service:${a.id}`,
      id: a.id,
      name: a.name,
      runtime: a.runtime,
      service: a,
      placements,
      health: rollupHealth(placements.filter((p) => p.instance.role === "active").map((p) => p.health)),
      rollout: rollupRollout(a.instances.map(placementRollout)),
    };
  };
  const env = (n: Environment): TreeEnv => {
    const services = (servicesByEnv.get(n.id) ?? []).map(service).sort(byName);
    return { kind: "env", key: `env:${n.id}`, id: n.id, name: n.name, clusterId: n.cluster_id, protected: n.protected, services, health: rollupHealth(services.map((s) => s.health)) };
  };

  const envsByProject = new Map<string | null, Environment[]>();
  for (const n of environments) {
    const g = n.project_id && projects.some((x) => x.id === n.project_id) ? n.project_id : null;
    envsByProject.set(g, [...(envsByProject.get(g) ?? []), n]);
  }
  const mk = (id: string | null, key: string, name: string, description: string): TreeProject => {
    const envs = (envsByProject.get(id) ?? []).map(env).sort(byName);
    return { kind: "project", key, id, name, description, envs, health: rollupHealth(envs.map((e) => e.health)) };
  };
  const tree = projects.map((g) => mk(g.id, `project:${g.id}`, g.name, g.description)).sort(byName);
  if (envsByProject.has(null)) tree.push(mk(null, UNASSIGNED_KEY, "Unassigned", "Environments without a project"));
  return tree;
}

export type HealthFilter = "all" | "unhealthy" | "deploying";

export interface TreeFilter {
  query: string;
  health: HealthFilter;
  runtime: string;
  cluster: string;
}

export const EMPTY_FILTER: TreeFilter = { query: "", health: "all", runtime: "", cluster: "" };

function serviceMatchesFilters(s: TreeService, f: TreeFilter, env: TreeEnv): boolean {
  if (f.runtime && s.runtime !== f.runtime) return false;
  if (f.cluster && env.clusterId !== f.cluster) return false;
  if (f.health === "unhealthy" && s.health !== "bad" && s.health !== "warn" && s.rollout !== "failed") return false;
  if (f.health === "deploying" && s.rollout !== "deploying") return false;
  return true;
}

function serviceMatchesText(s: TreeService, q: string): boolean {
  return s.name.toLowerCase().includes(q) || s.runtime.toLowerCase().includes(q) || s.placements.some((p) => p.nodeName.toLowerCase().includes(q));
}

export interface FilterResult {
  tree: TreeProject[];
  /** Keys to expand so every match is visible with its ancestors. */
  expand: Set<string>;
  serviceCount: number;
}

/**
 * Filter the tree without flattening it: a matching service keeps its
 * environment and project; a matching environment or project keeps
 * all of its descendants (that pass the attribute filters).
 */
export function filterTree(tree: TreeProject[], f: TreeFilter): FilterResult {
  const q = f.query.trim().toLowerCase();
  const attrFilter = f.health !== "all" || !!f.runtime || !!f.cluster;
  const expand = new Set<string>();
  let serviceCount = 0;
  const out: TreeProject[] = [];
  for (const app of tree) {
    const appHit = !!q && app.name.toLowerCase().includes(q);
    const envs: TreeEnv[] = [];
    for (const env of app.envs) {
      if (f.cluster && env.clusterId !== f.cluster) continue;
      const envHit = appHit || (!!q && env.name.toLowerCase().includes(q));
      const services = env.services.filter((s) => serviceMatchesFilters(s, f, env) && (!q || envHit || serviceMatchesText(s, q)));
      const keepEmptyEnv = !attrFilter && (!q || envHit);
      if (services.length === 0 && !keepEmptyEnv) continue;
      envs.push({ ...env, services });
      serviceCount += services.length;
      if (q || attrFilter) {
        expand.add(app.key);
        if (services.length) expand.add(env.key);
      }
    }
    if (envs.length === 0 && !(!attrFilter && (!q || appHit))) continue;
    out.push({ ...app, envs });
  }
  return { tree: out, expand, serviceCount };
}

export interface VisibleRow {
  node: TreeNode;
  depth: number;
  parentKey: string | null;
  hasChildren: boolean;
}

/** The rows shown for the current expansion, in display order. */
export function flattenVisible(tree: TreeProject[], expanded: ReadonlySet<string>): VisibleRow[] {
  const rows: VisibleRow[] = [];
  for (const app of tree) {
    rows.push({ node: app, depth: 0, parentKey: null, hasChildren: app.envs.length > 0 });
    if (!expanded.has(app.key)) continue;
    for (const env of app.envs) {
      rows.push({ node: env, depth: 1, parentKey: app.key, hasChildren: env.services.length > 0 });
      if (!expanded.has(env.key)) continue;
      for (const s of env.services) rows.push({ node: s, depth: 2, parentKey: env.key, hasChildren: false });
    }
  }
  return rows;
}

/** Find a node and its ancestor keys by key. */
export function locate(tree: TreeProject[], key: string): { node: TreeNode; path: TreeNode[] } | null {
  for (const app of tree) {
    if (app.key === key) return { node: app, path: [app] };
    for (const env of app.envs) {
      if (env.key === key) return { node: env, path: [app, env] };
      for (const s of env.services) if (s.key === key) return { node: s, path: [app, env, s] };
    }
  }
  return null;
}
