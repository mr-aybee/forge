import { ArrowRight, Boxes, Globe, Layers, Plus, Server, X } from "lucide-react";
import { useState } from "react";

import { useAllEnvironments, useClusters } from "../../api/hooks";
import { useStackSwarm, useSwarmStacks } from "../../api/swarm";
import type { Cluster, Service } from "../../api/types";
import { Input, Select } from "../ui";
import type { Member, Target } from "./types";

type SwarmTarget = Extract<Target, { kind: "swarm" }>;

export function targetLabel(t: Target, members: Member[], apps: Service[] = [], clusters: Cluster[] = []): string {
  switch (t.kind) {
    case "swarm":
      return `${t.service ? `${t.service} · ` : ""}swarm ${clusters.find((c) => c.id === t.cluster_id)?.name ?? t.cluster_id.slice(0, 8)}:${t.port}`;
    case "app":
      return `${apps.find((a) => a.id === t.service_id)?.name ?? `service ${t.service_id.slice(0, 8)}`}:${t.port}`;
    case "node":
      return `${members.find((m) => m.id === t.server_id)?.name ?? t.server_id.slice(0, 8)}:${t.port}`;
    case "external":
      return t.address;
  }
}

/** The port a service listens on: its PORT variable, else its first published port, else 80. */
export const appPort = (a?: Service) => {
  const env = a?.env?.find((e) => e.key === "PORT")?.value;
  return Number(env) || Number(String(a?.spec.ports?.[0] ?? "").split(":")[0]) || 80;
};

const KINDS = {
  app: { label: "Service", icon: Layers, tone: "text-accent bg-accent-soft" },
  node: { label: "Node", icon: Server, tone: "text-muted bg-panel-2" },
  external: { label: "Address", icon: Globe, tone: "text-muted bg-panel-2" },
  swarm: { label: "Swarm port", icon: Boxes, tone: "text-accent bg-accent-soft" },
} as const;

/** Picks a Swarm cluster and, optionally, a published port of a stack service there. */
function SwarmTargetPicker({ t, swarms, onChange }: { t: SwarmTarget; swarms: Cluster[]; onChange: (t: SwarmTarget) => void }) {
  const [stackId, setStackId] = useState("");
  const stacks = useSwarmStacks(t.cluster_id);
  const status = useStackSwarm(stackId);
  const published = (status.data?.status?.services ?? []).flatMap((s) => s.ports.filter((p) => p.published).map((p) => ({ s: s.name, port: p.published as number })));
  return (
    <div className="flex min-w-0 flex-wrap gap-1.5 sm:flex-nowrap">
      <Select value={t.cluster_id} onChange={(e) => onChange({ ...t, cluster_id: e.target.value, service: undefined })} aria-label="Swarm cluster">
        {!swarms.some((c) => c.id === t.cluster_id) && <option value={t.cluster_id}>{t.cluster_id ? "Unknown cluster" : "Choose a Swarm cluster"}</option>}
        {swarms.map((c) => (
          <option key={c.id} value={c.id}>
            {c.name}
          </option>
        ))}
      </Select>
      <Select value={stackId} onChange={(e) => setStackId(e.target.value)} aria-label="Stack" disabled={!t.cluster_id}>
        <option value="">{stacks.error ? "Stacks unavailable" : "Any stack (type a port)"}</option>
        {(stacks.data ?? []).map((s) => (
          <option key={s.id} value={s.id}>
            {s.environment ? `${s.environment}/` : ""}
            {s.name}
          </option>
        ))}
      </Select>
      {stackId && (
        <Select
          value={t.service && published.some((p) => p.s === t.service && p.port === t.port) ? `${t.service}:${t.port}` : ""}
          onChange={(e) => {
            const p = published.find((x) => `${x.s}:${x.port}` === e.target.value);
            onChange(p ? { ...t, service: p.s, port: p.port } : { ...t, service: undefined });
          }}
          aria-label="Published port"
        >
          <option value="">{status.isLoading ? "Loading…" : published.length ? "Choose a published port" : "No published ports"}</option>
          {published.map((p) => (
            <option key={`${p.s}:${p.port}`} value={`${p.s}:${p.port}`}>
              {p.s} :{p.port}
            </option>
          ))}
        </Select>
      )}
    </div>
  );
}

const ADDR_RE = /^[A-Za-z0-9.\-[\]:]+:\d{1,5}$/;

/**
 * Typed upstream targets as a compact list. Each row: what it is (fixed
 * badge), what it points at, the port, and where it resolves now.
 */
export function TargetEditor({
  label,
  hint,
  value,
  onChange,
  members,
  apps = [],
  autoFocusPort = false,
}: {
  label: string;
  hint?: string;
  value: Target[];
  onChange: (t: Target[]) => void;
  members: Member[];
  apps?: Service[];
  /** Focus the first row's port on mount (the caller could not guess it). */
  autoFocusPort?: boolean;
}) {
  const environments = useAllEnvironments().data ?? [];
  const swarms = (useClusters().data ?? []).filter((c) => c.mode === "swarm");
  const nsName = (id: string) => environments.find((n) => n.id === id)?.name ?? "";
  const byNs = new Map<string, Service[]>();
  for (const a of apps) byNs.set(a.environment_id, [...(byNs.get(a.environment_id) ?? []), a]);
  const set = (i: number, t: Target) => onChange(value.map((x, n) => (n === i ? t : x)));
  const add = (kind: Target["kind"]) => {
    const used = new Set(value.filter((t) => t.kind === "app").map((t) => (t as { service_id: string }).service_id));
    const app = apps.find((a) => !used.has(a.id)) ?? apps[0];
    onChange([
      ...value,
      kind === "app"
        ? { kind, service_id: app?.id ?? "", port: appPort(app) }
        : kind === "node"
          ? { kind, server_id: members[0]?.id ?? "", port: 8080 }
          : kind === "swarm"
            ? { kind, cluster_id: swarms[0]?.id ?? "", port: 80 }
            : { kind, address: "" },
    ]);
  };

  const resolves = (t: Target): string => {
    if (t.kind === "app") {
      const a = apps.find((x) => x.id === t.service_id);
      if (!a) return "unknown service";
      const n = a.instances.length;
      if (!n) return "not placed on any node yet";
      const standby = a.instances.filter((i) => i.role === "standby").length;
      const replicas = n - standby;
      return `${replicas} replica${replicas === 1 ? "" : "s"}${standby ? ` + ${standby} standby` : ""}`;
    }
    if (t.kind === "node") {
      const m = members.find((x) => x.id === t.server_id);
      return m ? `${m.address}:${t.port}` : "unknown node";
    }
    if (t.kind === "swarm") return swarms.some((c) => c.id === t.cluster_id) ? `every swarm node :${t.port}` : "unknown cluster";
    return t.address && !ADDR_RE.test(t.address) ? "use host:port" : "";
  };

  return (
    <div className="space-y-1.5">
      <div className="flex items-end justify-between gap-3">
        <div>
          <div className="text-xs font-medium text-muted">{label}</div>
          {hint && <div className="text-[11px] text-faint">{hint}</div>}
        </div>
        <div className="flex shrink-0 gap-1">
          {(Object.keys(KINDS) as Target["kind"][]).map((k) => {
            const K = KINDS[k];
            return (
              <button
                key={k}
                type="button"
                disabled={(k === "app" && !apps.length) || (k === "swarm" && !swarms.length)}
                title={
                  k === "app" && !apps.length
                    ? "No services in this cluster"
                    : k === "swarm" && !swarms.length
                      ? "No Swarm cluster yet"
                      : k === "swarm"
                        ? "Every node of a Swarm cluster on a routing-mesh port"
                        : `Add a ${K.label.toLowerCase()} target`
                }
                onClick={() => add(k)}
                className="inline-flex h-7 items-center gap-1 rounded-md border border-line px-2 text-xs text-muted transition hover:border-accent/50 hover:text-accent disabled:opacity-40"
              >
                <Plus className="size-3" /> {K.label}
              </button>
            );
          })}
        </div>
      </div>

      {value.length === 0 ? (
        <div className="rounded-md border border-dashed border-line px-3 py-2.5 text-xs text-faint">No targets. Add a service from this cluster, a node port or any address.</div>
      ) : (
        <div className="divide-y divide-line rounded-md border border-line">
          {value.map((t, i) => {
            const K = KINDS[t.kind];
            const r = resolves(t);
            const warn = r === "unknown service" || r === "unknown node" || r === "unknown cluster" || r.startsWith("not placed") || r === "use host:port";
            return (
              <div key={i} className="flex flex-wrap items-center gap-2 px-2 py-1.5 sm:flex-nowrap">
                <span className={`inline-flex w-[76px] shrink-0 items-center gap-1 rounded px-1.5 py-0.5 text-[11px] font-medium ${K.tone}`}>
                  <K.icon className="size-3" /> {K.label}
                </span>
                <div className="min-w-0 flex-1">
                  {t.kind === "app" && (
                    <Select value={t.service_id} onChange={(e) => set(i, { ...t, service_id: e.target.value, port: appPort(apps.find((a) => a.id === e.target.value)) })} aria-label="Service">
                      {!apps.some((a) => a.id === t.service_id) && <option value={t.service_id}>{t.service_id ? "Unknown service" : "Choose a service"}</option>}
                      {[...byNs.entries()].map(([ns, list]) => (
                        <optgroup key={ns} label={nsName(ns) || "environment"}>
                          {list.map((a) => (
                            <option key={a.id} value={a.id}>
                              {a.name}
                            </option>
                          ))}
                        </optgroup>
                      ))}
                    </Select>
                  )}
                  {t.kind === "node" && (
                    <Select value={t.server_id} onChange={(e) => set(i, { ...t, server_id: e.target.value })} aria-label="Node">
                      {members.map((m) => (
                        <option key={m.id} value={m.id}>
                          {m.name}
                        </option>
                      ))}
                    </Select>
                  )}
                  {t.kind === "swarm" && <SwarmTargetPicker t={t} swarms={swarms} onChange={(n) => set(i, n)} />}
                  {t.kind === "external" && (
                    <Input className="h-8 font-mono text-xs" placeholder="10.0.0.20:8000" value={t.address} onChange={(e) => set(i, { ...t, address: e.target.value.trim() })} aria-label="host:port" />
                  )}
                </div>
                {t.kind !== "external" && (
                  <div className="flex shrink-0 items-center gap-1">
                    <span className="text-faint">:</span>
                    <Input
                      className="h-8 w-[72px] text-center font-mono text-xs"
                      type="number"
                      min={1}
                      max={65535}
                      autoFocus={autoFocusPort && i === 0}
                      value={t.port || ""}
                      placeholder="port"
                      onChange={(e) => set(i, { ...t, port: Number(e.target.value) })}
                      aria-label="Port"
                    />
                  </div>
                )}
                {r && (
                  <span className={`inline-flex shrink-0 items-center gap-1 text-[11px] ${warn ? "text-warn" : "text-faint"}`}>
                    <ArrowRight className="size-3" /> {r}
                  </span>
                )}
                <button type="button" aria-label="Remove target" className="ml-auto shrink-0 rounded p-1 text-faint hover:bg-panel-2 hover:text-bad" onClick={() => onChange(value.filter((_, n) => n !== i))}>
                  <X className="size-3.5" />
                </button>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
