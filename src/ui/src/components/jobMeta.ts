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
  store: (id: string) => string | undefined;
}

/** Name lookups for job labels. Apps and stores come from cache only (no extra requests). */
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
    store: (id) => fromCache("object-stores", id),
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
      const op = s(cmd.op);
      if (op === "pull") return `Pull image ${s(cmd.image) ?? ""}`.trim();
      if (op === "list" || op === "info" || op === "images") return `List containers`;
      const id = s(cmd.id);
      return `${verb(op, "Container")} container${id ? ` ${shortId(id)}` : ""}`;
    }
    case "probe":
      return "Connectivity probe";
    case "fabric": {
      const op = s(rec(cmd.op).op) ?? s(cmd.op);
      return op ? `Cluster network: ${op}` : "Cluster network";
    }
    case undefined:
      return "Agent command";
    default:
      return cap(type);
  }
}

/** "Install redis 7.4 on lab-c", "Onboard 203.0.113.10", "Ping lab-c". */
export function jobTitle(j: Job, names: JobNames): string {
  const p = rec(j.payload);
  const target = jobTarget(j, names);
  const on = target.kind === "node" ? ` on ${target.label}` : "";
  switch (j.kind) {
    case "package.action": {
      const pkg = s(p.package) ?? "package";
      const v = s(p.action) === "install" && s(p.version) ? ` ${s(p.version)}` : "";
      return `${verb(s(p.action), "Run")} ${pkg}${v}${on}`;
    }
    case "node.bootstrap": {
      const host = s(p.hostname) ?? s(rec(p.target).host) ?? "node";
      const into = target.kind === "cluster" ? ` into ${target.label}` : "";
      return `Onboard ${host}${into}`;
    }
    case "agent.command":
      return `${commandLabel(rec(p.command))}${on}`;
    case "agent.update":
      return `Update agent${on}`;
    case "agent.rollout": {
      const ids = Array.isArray(p.server_ids) ? p.server_ids.length : 0;
      const to = s(p.target) ?? s(rec(j.state).target);
      const auto = s(p.origin) === "auto" ? " (automatic)" : "";
      return `Roll out agent${to ? ` ${to}` : ""} to ${ids || "outdated"} node${ids === 1 ? "" : "s"}${auto}`;
    }
    case "app.action": {
      const id = s(p.service_id);
      const name = (id && names.app(id)) ?? "service";
      return `${verb(s(p.action), "Run")} ${name}${on}`;
    }
    case "storage.action": {
      const id = s(p.store_id);
      const name = id ? names.store(id) : undefined;
      return `${verb(s(p.action), "Run")} object store${name ? ` ${name}` : ""}${on}`;
    }
    default:
      return j.kind ? cap(j.kind.replace(/\./g, " ")) : "Job";
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
