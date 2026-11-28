// Human labels for jobs: "Install redis 7.4 on lab-c", target node or
// cluster, timings. Every field of a job's payload is treated as optional.

import { useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";

import { useClusters, useServers } from "../api/hooks";
import { type Job, type JobStep, nodeName } from "../api/types";

export const isActive = (j?: Pick<Job, "status"> | null) => !!j && (j.status === "queued" || j.status === "running");

type Rec = Record<string, unknown>;
const rec = (v: unknown): Rec => (v && typeof v === "object" && !Array.isArray(v) ? (v as Rec) : {});
const s = (v: unknown): string | undefined => (typeof v === "string" && v ? v : undefined);
const cap = (v: string) => (v ? v[0]!.toUpperCase() + v.slice(1).replace(/_/g, " ") : v);
const shortId = (v: string) => (v.length > 12 ? v.slice(0, 12) : v);

const VERB: Record<string, string> = {
  install: "Install",
  uninstall: "Uninstall",
  remove: "Remove",
  start: "Start",
  stop: "Stop",
  restart: "Restart",
  status: "Check",
  update: "Update",
  upgrade: "Update",
  deploy: "Deploy",
  adopt: "Adopt",
  logs: "Read logs of",
};
const verb = (a: string | undefined, fallback: string) => (a ? (VERB[a] ?? cap(a)) : fallback);

/** Names for ids, from the queries the console already holds. */
export interface JobNames {
  node: (id: string) => string | undefined;
  cluster: (id: string) => string | undefined;
  app: (id: string) => string | undefined;
}

/** Name lookups for job labels. Apps come from cache only (no extra requests). */
export function useJobNames(): JobNames {
  const servers = useServers().data;
  const clusters = useClusters().data;
  const qc = useQueryClient();
  const fromCache = (key: string, id: string) => {
    for (const [, data] of qc.getQueriesData<unknown>({ queryKey: [key] })) {
      if (!Array.isArray(data)) continue;
      const hit = data.find((x) => rec(x).id === id);
      if (hit) return s(rec(hit).name);
    }
    return undefined;
  };
  return {
    node: (id) => {
      const n = (Array.isArray(servers) ? servers : []).find((x) => x?.id === id);
      return n ? nodeName(n) : undefined;
    },
    cluster: (id) => (Array.isArray(clusters) ? clusters : []).find((c) => c?.id === id)?.name,
    app: (id) => fromCache("services", id),
  };
}

export interface JobTarget {
  kind: "node" | "cluster" | "host" | "other";
  id?: string;
  label: string;
}

/** The node or cluster a job acts on. */
export function jobTarget(j: Job, names: JobNames): JobTarget {
  const p = rec(j.payload);
  if (j.kind === "node.bootstrap") {
    const server = s(rec(j.state).server_id);
    if (server) return { kind: "node", id: server, label: names.node(server) ?? s(p.hostname) ?? shortId(server) };
    const cluster = s(p.cluster_id);
    if (cluster) return { kind: "cluster", id: cluster, label: names.cluster(cluster) ?? shortId(cluster) };
    return { kind: "host", label: s(rec(p.target).host) ?? "—" };
  }
  const server = s(p.server_id) ?? (j.resource?.kind === "server" ? s(j.resource.id) : undefined);
  if (server) return { kind: "node", id: server, label: names.node(server) ?? shortId(server) };
  const cluster = s(p.cluster_id) ?? (j.resource?.kind === "cluster" ? s(j.resource.id) : undefined);
  if (cluster) return { kind: "cluster", id: cluster, label: names.cluster(cluster) ?? shortId(cluster) };
  if (j.resource?.kind && j.resource.id) return { kind: "other", label: `${j.resource.kind}:${shortId(j.resource.id)}` };
  return { kind: "other", label: "—" };
}

/** Container operations that only read state (mirrors the server's `CONTAINER_READ`). */
const CONTAINER_READ = ["info", "list", "images", "inspect", "logs", "stats"];
const op = (cmd: Rec) => s(rec(cmd.op).op) ?? s(cmd.op);

function commandLabel(cmd: Rec): string {
  const type = s(cmd.type);
  switch (type) {
    case "ping":
      return "Ping";
    case "collect_facts":
      return "Collect facts";
    case "service_status":
      return `Check service ${s(cmd.name) ?? ""}`.trim();
    case "container": {
      const o = s(cmd.op);
      if (o === "pull") return `Pull image ${s(cmd.image) ?? ""}`.trim();
      if (o === "list" || o === "info" || o === "images") return "List containers";
      if (o === "logs") return "Read container logs";
      if (o === "stats") return "Container stats";
      if (o === "inspect") return "Inspect container";
      const id = s(cmd.id);
      return `${verb(o, "Container")} container${id ? ` ${shortId(id)}` : ""}`;
    }
    case "probe":
      return "Connectivity probe";
    case "ingress": {
      switch (op(cmd)) {
        case "status":
          return "Ingress status check";
        case "apply":
          return "Apply ingress config";
        case "remove":
          return "Remove ingress";
        case "network_inspect":
          return "Inspect ingress network";
        case "network_ensure":
          return "Set up ingress network";
        default:
          return "Ingress";
      }
    }
    case "site": {
      const name = s(rec(cmd.spec).name);
      const n = name ? ` ${name}` : "";
      switch (s(cmd.action)) {
        case "apply":
          return `Apply site${n}`;
        case "remove":
          return `Remove site${n}`;
        case "status":
          return `Site status check${n}`;
        case "renew":
          return `Renew certificate${n ? ` for${n}` : ""}`;
        default:
          return `Site${n}`;
      }
    }
    case "package": {
      const pkg = s(cmd.package) ?? "package";
      const v = s(cmd.action) === "install" && s(cmd.version) ? ` ${s(cmd.version)}` : "";
      return `${verb(s(cmd.action), "Run")} ${pkg}${v}`;
    }
    case "dns": {
      const o = op(cmd);
      return o === "zones" || o === "records" ? "List DNS records" : o ? `DNS: ${o.replace(/_/g, " ")}` : "DNS";
    }
    case "fabric": {
      const o = op(cmd);
      return o ? `Cluster network: ${o.replace(/_/g, " ")}` : "Cluster network";
    }
    case undefined:
      return "Agent command";
    default:
      return cap(type);
  }
}

/** What the job does, without its target: "Install redis 7.4", "Ingress status check". */
export function jobAction(j: Job, names: JobNames): string {
  const p = rec(j.payload);
  switch (j.kind) {
    case "package.action": {
      const pkg = s(p.package) ?? "package";
      const v = s(p.action) === "install" && s(p.version) ? ` ${s(p.version)}` : "";
      return `${verb(s(p.action), "Run")} ${pkg}${v}`;
    }
    case "node.bootstrap": {
      const host = s(p.hostname) ?? s(rec(p.target).host) ?? "node";
      const target = jobTarget(j, names);
      const into = target.kind === "cluster" ? ` into ${target.label}` : "";
      return `Onboard ${host}${into}`;
    }
    case "agent.command":
      return commandLabel(rec(p.command));
    case "agent.update":
      return "Update agent";
    case "agent.rollout": {
      const ids = Array.isArray(p.server_ids) ? p.server_ids.length : 0;
      const to = s(p.target) ?? s(rec(j.state).target);
      const auto = s(p.origin) === "auto" ? " (automatic)" : "";
      return `Roll out agent${to ? ` ${to}` : ""} to ${ids || "outdated"} node${ids === 1 ? "" : "s"}${auto}`;
    }
    case "app.action": {
      const id = s(p.service_id);
      const name = (id && names.app(id)) ?? "service";
      return `${verb(s(p.action), "Run")} ${name}`;
    }
    case "swarm.stack": {
      const what = s(p.stack_name) ?? "stack";
      const act = p.action && typeof p.action === "object" ? s((p.action as { kind?: unknown }).kind) : undefined;
      return `${act === "remove" ? "Remove" : "Deploy"} Swarm stack ${what}`;
    }
    case "swarm.member":
      return `${s(p.action) === "leave" ? "Leave" : "Join"} swarm`;
    case "swarm.service":
      return `${verb(p.action && typeof p.action === "object" ? s((p.action as { kind?: unknown }).kind) : undefined, "Deploy")} Swarm service`;
    default:
      return j.kind ? cap(j.kind.replace(/\./g, " ")) : "Job";
  }
}

/** "Install redis 7.4 · lab-c", "Ingress status check · lab-c", "Onboard 203.0.113.10". */
export function jobTitle(j: Job, names: JobNames): string {
  const target = jobTarget(j, names);
  const action = jobAction(j, names);
  return target.kind === "node" ? `${action} · ${target.label}` : action;
}

/** Colour family of a job in lists: what it touches. */
export type JobCategory = "ingress" | "site" | "package" | "container" | "health" | "deploy" | "agent" | "node" | "network" | "other";

export function jobCategory(j: Job): JobCategory {
  const p = rec(j.payload);
  switch (j.kind) {
    case "package.action":
      return "package";
    case "node.bootstrap":
      return "node";
    case "agent.update":
    case "agent.rollout":
      return "agent";
    case "app.action":
      return "deploy";
    case "swarm.stack":
    case "swarm.service":
    case "swarm.member":
      return "container";
    case "agent.command":
      break;
    default:
      return j.kind?.startsWith("deploy") || j.kind?.startsWith("app") ? "deploy" : "other";
  }
  switch (s(rec(p.command).type)) {
    case "ingress":
      return "ingress";
    case "site":
      return "site";
    case "package":
      return "package";
    case "container":
    case "docker":
    case "stack":
    case "swarm":
      return "container";
    case "ping":
    case "collect_facts":
    case "service_status":
      return "health";
    case "probe":
    case "fabric":
    case "dns":
      return "network";
    default:
      return "agent";
  }
}

/**
 * Read-only checks the console runs on its own (status polls, fact
 * collection, container lists). The API has no "background" flag, so this
 * classifies by operation, mirroring the server's read-only actions.
 */
export function isBackground(j: Job): boolean {
  if (j.kind !== "agent.command") return false;
  const cmd = rec(rec(j.payload).command);
  switch (s(cmd.type)) {
    case "ping":
    case "collect_facts":
    case "service_status":
    case "probe":
    case "processes":
    case "names":
      return true;
    case "container":
      return CONTAINER_READ.includes(s(cmd.op) ?? "");
    case "ingress":
      return ["status", "network_inspect"].includes(op(cmd) ?? "");
    case "site":
      return s(cmd.action) === "status";
    case "dns":
    case "fabric":
      return ["zones", "records", "status", "key"].includes(op(cmd) ?? "");
    default:
      return false;
  }
}

/** When the job started running: first step start, else creation. */
export function startedAt(j: Job, steps?: JobStep[] | null): number | undefined {
  const first = (Array.isArray(steps) ? steps : []).map((x) => x?.started_at).filter((t): t is number => typeof t === "number" && t > 0);
  return first.length ? Math.min(...first) : j.created_at || undefined;
}

/** Elapsed ms: until now while active, else until the last update. */
export function elapsed(j: Job, now: number, steps?: JobStep[] | null): number | undefined {
  const start = j.status === "queued" ? j.created_at : startedAt(j, steps);
  if (!start) return undefined;
  return (isActive(j) ? now : j.updated_at || now) - start;
}

/** Current time, ticking every `ms` while `on`. */
export function useNow(on: boolean, ms = 1000): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!on) return;
    setNow(Date.now());
    const t = setInterval(() => setNow(Date.now()), ms);
    return () => clearInterval(t);
  }, [on, ms]);
  return now;
}
