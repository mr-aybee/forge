import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { ArrowUpRight, Boxes, Globe, Layers, Lock, Network, Server } from "lucide-react";
import { type ReactNode, useCallback, useLayoutEffect, useRef, useState } from "react";

import { get } from "../../api/client";
import { useServices, useServers } from "../../api/hooks";
import type { Service, Server as Node } from "../../api/types";
import { type LiveState, liveState } from "../InstanceState";
import { Badge, Empty, ErrorNote, Skeleton } from "../ui";
import { bytesRate as rate } from "../Traffic";
import type { LiveResponse, SiteTraffic } from "./types";


interface Hop {
  address: string;
  standby: boolean;
  server_id: string | null;
  node: string | null;
  state: string | null;
  /** Placement role, when the server sends it (newer servers). */
  role?: "active" | "standby" | null;
}
type Role = "replica" | "standby";
/**
 * The placement role of an app upstream: from the payload when present, else
 * from the app's instances by node, else from the upstream's `standby` flag.
 */
const roleOf = (t: RouteTarget, u: Hop, apps: Map<string, Service>): Role | undefined => {
  if (t.target.kind !== "app") return undefined;
  if (u.role) return u.role === "standby" ? "standby" : "replica";
  const inst = u.server_id ? apps.get(t.target.service_id)?.instances?.find((i) => i.server_id === u.server_id) : undefined;
  if (inst) return inst.role === "standby" ? "standby" : "replica";
  return u.standby ? "standby" : "replica";
};
interface RouteTarget {
  target: { kind: "app"; service_id: string; port: number } | { kind: "node"; server_id: string; port: number } | { kind: "external"; address: string } | { kind: "swarm"; cluster_id: string; port: number; service?: string };
  label: string;
  internal_name: string | null;
  standby: boolean;
  upstreams: (Hop & { resolved?: string[] })[];
}
interface RouteEndpoint {
  id: string;
  name: string;
  domains: string[];
  tls: string | Record<string, unknown> | null;
  edges: { server_id: string; name: string | null; address: string | null; status: string | null }[];
  targets: RouteTarget[];
  /** `profile`: linked to this cluster; `edges`: explicit edges on this cluster's nodes, not linked. */
  source?: "profile" | "edges";
}

type Tone = "ok" | "bad" | "warn" | "idle";
const dot: Record<Tone, string> = { ok: "bg-ok", bad: "bg-bad", warn: "bg-warn", idle: "bg-faint/60" };
/**
 * One upstream's state: the node's agent status plus, for app targets, the
 * state the node last reported for the placement. External targets: unknown.
 */
const hopState = (t: RouteTarget, u: Hop, node: Node | undefined): LiveState | undefined => {
  if (t.target.kind === "external" || t.target.kind === "swarm") return undefined;
  if (t.target.kind === "node") return node && (node.status === "offline" || node.status === "revoked") ? liveState({ status: "deployed", state: null, last_error: null }, node) : undefined;
  return liveState({ status: "deployed", state: u.state ?? null, last_error: null }, node);
};
const isDown = (s?: LiveState) => !!s && (s.kind === "node-offline" || s.kind === "failed" || s.kind === "stopped" || s.kind === "not-deployed");
const hopTone = (s?: LiveState): Tone | undefined => (!s ? undefined : isDown(s) ? "bad" : s.kind === "running" ? "ok" : s.kind === "changing" ? "warn" : "idle");
const downLabel = (s: LiveState) => (s.kind === "node-offline" ? "node offline" : "not running");
const stroke = (t?: Tone) => (t === "bad" ? "var(--color-bad)" : t === "warn" ? "var(--color-warn)" : "var(--color-accent)");

type Reg = (id: string) => (el: HTMLElement | null) => void;

/** A slim pill. Pills that name an app or node are links (accent, ↗). */
function Pill({ id, reg, tone, icon, children, sub, to, standby, role, title, rate: flow, down }: { id: string; reg: Reg; tone?: Tone; icon?: ReactNode; children: ReactNode; sub?: string; to?: { kind: "app" | "node"; id: string }; standby?: boolean; /** Placement role chip (`replica` / `standby`) for app upstreams. */ role?: Role; title?: string; rate?: { text: string; title?: string }; down?: string }) {
  const cls = `group relative z-10 inline-flex max-w-full items-center gap-1.5 rounded-md border px-2 py-0.5 text-xs ${down ? "bg-panel-2 opacity-60" : "bg-panel"} ${standby ? "border-dashed border-line" : "border-line"} ${to ? `${down ? "text-muted" : "text-accent"} transition hover:border-accent/60 hover:bg-accent-soft hover:opacity-100` : ""}`;
  const inner = (
    <>
      {tone && <span className={`size-1.5 shrink-0 rounded-full ${dot[tone]}`} />}
      {icon && <span className={`shrink-0 ${to ? "text-accent/80" : "text-faint"}`}>{icon}</span>}
      <span className="truncate font-medium">{children}</span>
      {sub && <span className="shrink-0 font-mono text-[10.5px] text-faint">{sub}</span>}
      {role ? (
        <span className={`shrink-0 rounded px-1 text-[10px] font-medium ${role === "standby" ? "bg-panel-2 text-muted" : "bg-accent-soft text-accent"}`} title={role === "standby" ? "Standby placement: Nginx backup, used when every replica is down" : "Replica: active upstream"}>
          {role}
        </span>
      ) : (
        standby && <span className="shrink-0 text-[10px] text-faint">standby</span>
      )}
      {down && <span className="shrink-0 rounded bg-bad-soft px-1 text-[10px] font-medium text-bad">{down}</span>}
      {to && <ArrowUpRight className="size-3 shrink-0 opacity-50 group-hover:opacity-100" />}
      {flow && (
        <span
          title={flow.title}
          className={`ml-0.5 shrink-0 border-l border-line pl-1.5 font-mono text-[10px] tabular-nums ${flow.text === "idle" ? "text-faint" : "text-fg"}`}
        >
          {flow.text}
        </span>
      )}
    </>
  );
  if (to?.kind === "app")
    return (
      <Link ref={reg(id)} to="/services/$serviceId" params={{ serviceId: to.id }} className={cls} title={title ?? "Open the service"}>
        {inner}
      </Link>
    );
  if (to?.kind === "node")
    return (
      <Link ref={reg(id)} to="/nodes/$nodeId" params={{ nodeId: to.id }} className={cls} title={title ?? "Open the node"}>
        {inner}
      </Link>
    );
  return (
    <span ref={reg(id)} className={cls} title={title}>
      {inner}
    </span>
  );
}

interface Wire {
  from: string;
  to: string;
  dashed?: boolean;
  tone?: Tone;
  /** Throughput shown on the line. */
  label?: string;
  title?: string;
}

/** One endpoint: a slim row, with thin connectors drawn between the pills. */
function Row({ ep, live, nodes, apps }: { ep: RouteEndpoint; live?: LiveResponse; nodes: Map<string, Node>; apps: Map<string, Service> }) {
  const wrap = useRef<HTMLDivElement>(null);
  const els = useRef(new Map<string, HTMLElement>());
  const [paths, setPaths] = useState<{ d: string; dashed?: boolean; tone?: Tone; label?: string; title?: string; lx: number; ly: number }[]>([]);
  const reg = useCallback<Reg>((id) => (el) => {
    if (el) els.current.set(id, el);
    else els.current.delete(id);
  }, []);

  const nodeOf = (id: string | null) => (id ? nodes.get(id) : undefined);
  const edgeOffline = (id: string) => {
    const st = nodeOf(id)?.status;
    return st === "offline" || st === "revoked";
  };
  const edgeTone = (e: RouteEndpoint["edges"][number]): Tone => {
    if (edgeOffline(e.server_id)) return "bad";
    const st = live?.edges[e.server_id]?.state;
    if (st) return st.nginx === "running" ? "ok" : "bad";
    return e.status === "applied" ? "ok" : e.status === "failed" ? "bad" : "idle";
  };

  // Last minute of traffic for this endpoint, per edge (keyed by its first domain).
  const domain = ep.domains[0] ?? "";
  const siteOn = (edge: string): SiteTraffic | undefined => live?.edges[edge]?.state?.traffic?.[domain];
  const wires: Wire[] = [];
  for (const e of ep.edges) {
    const t = siteOn(e.server_id);
    wires.push({
      from: "domain",
      to: `edge:${e.server_id}`,
      tone: edgeTone(e),
      label: t ? rate(t.in_bps + t.out_bps) : undefined,
      title: t ? `${e.name ?? "edge"}: ${t.rps.toFixed(2)} req/s · in ${rate(t.in_bps)} · out ${rate(t.out_bps)} (last minute)` : undefined,
    });
  }
  ep.targets.forEach((t, i) => {
    for (const e of ep.edges) wires.push({ from: `edge:${e.server_id}`, to: `target:${i}`, dashed: t.standby });
    t.upstreams.forEach((u, j) => {
      // Sum what every edge sent to / received from this upstream.
      const keys = new Set([u.address, ...(u.resolved ?? [])]);
      let sent = 0;
      let recv = 0;
      let reqs = 0;
      let seen = false;
      for (const e of ep.edges) {
        for (const [addr, v] of Object.entries(siteOn(e.server_id)?.upstreams ?? {})) {
          if (keys.has(addr)) {
            seen = true;
            sent += v.sent_bps;
            recv += v.received_bps;
            reqs += v.requests;
          }
        }
      }
      const anyTraffic = ep.edges.some((e) => siteOn(e.server_id));
      wires.push({
        from: `target:${i}`,
        to: `up:${i}:${j}`,
        dashed: u.standby,
        tone: hopTone(hopState(t, u, nodeOf(u.server_id))),
        label: seen ? rate(sent + recv) : anyTraffic ? "idle" : undefined,
        title: seen ? `${u.node ?? u.address}: ${reqs} requests · to app ${rate(sent)} · from app ${rate(recv)} (last minute)` : undefined,
      });
    });
  });
  const key = JSON.stringify(wires.map(({ label: _l, title: _t, ...w }) => w));
  // The figure belongs to the box a line ends at (edge or node), not the line.
  const rateOf = (id: string) => {
    const w = wires.find((x) => x.to === id && x.label);
    return w?.label ? { text: w.label, title: w.title } : undefined;
  };

  useLayoutEffect(() => {
    const box = wrap.current;
    if (!box) return;
    const draw = () => {
      // Only side by side (md+); stacked rows read top to bottom without wires.
      if (box.clientWidth < 720) return setPaths([]);
      const base = box.getBoundingClientRect();
      const next: typeof paths = [];
      for (const w of JSON.parse(key) as Wire[]) {
        const a = els.current.get(w.from)?.getBoundingClientRect();
        const b = els.current.get(w.to)?.getBoundingClientRect();
        if (!a || !b) continue;
        const x1 = a.right - base.left + 2;
        const y1 = a.top + a.height / 2 - base.top;
        const x2 = b.left - base.left - 4;
        const y2 = b.top + b.height / 2 - base.top;
        const c = Math.max(12, (x2 - x1) / 2);
        next.push({ d: `M ${x1} ${y1} C ${x1 + c} ${y1}, ${x2 - c} ${y2}, ${x2} ${y2}`, dashed: w.dashed, tone: w.tone, label: w.label, title: w.title, lx: (x1 + x2) / 2, ly: (y1 + y2) / 2 });
      }
      setPaths(next);
    };
    draw();
    const ro = new ResizeObserver(draw);
    ro.observe(box);
    return () => ro.disconnect();
  }, [key]);

  // Endpoint health from its upstreams: degraded when some are down, down when none is up.
  const hops = ep.targets.flatMap((t) => t.upstreams.map((u) => hopState(t, u, nodeOf(u.server_id))));
  const downN = hops.filter(isDown).length;
  const health: "down" | "degraded" | undefined = hops.length && downN === hops.length ? "down" : downN > 0 ? "degraded" : undefined;
  const tlsOn = !!ep.tls && ep.tls !== "off";
  const mid = `m-${ep.id}`;
  return (
    <div ref={wrap} className="relative grid gap-2 border-b border-line px-3 py-2.5 last:border-b-0 hover:bg-panel-2/30 md:grid-cols-[minmax(0,1fr)_minmax(0,1fr)_minmax(0,1.1fr)_minmax(0,1.3fr)] md:items-center md:gap-x-10">
      <svg className="pointer-events-none absolute inset-0 z-0 h-full w-full overflow-visible" aria-hidden>
        <defs>
          {(["ok", "warn", "bad"] as const).map((t) => (
            <marker key={t} id={`${mid}-${t}`} viewBox="0 0 10 10" refX="9" refY="5" markerWidth="6" markerHeight="6" orient="auto">
              <path d="M 0 0 L 10 5 L 0 10 z" fill={stroke(t)} />
            </marker>
          ))}
        </defs>
        {paths.map((p, i) => (
          <path
            key={i}
            d={p.d}
            fill="none"
            stroke={stroke(p.tone)}
            strokeOpacity={p.dashed ? 0.45 : 0.7}
            strokeWidth={1.25}
            strokeDasharray={p.dashed ? "4 3" : undefined}
            markerEnd={`url(#${mid}-${p.tone === "bad" || p.tone === "warn" ? p.tone : "ok"})`}
          />
        ))}
      </svg>

      {/* Domain */}
      <div className="min-w-0">
        <Label>Domain</Label>
        <Pill id="domain" reg={reg} icon={tlsOn ? <Lock className="size-3 text-ok" /> : <Globe className="size-3" />} sub={ep.domains.length > 1 ? `+${ep.domains.length - 1}` : undefined} title={ep.domains.join(", ")}>
          {ep.domains[0]}
        </Pill>
        {ep.source === "edges" && (
          <div className="mt-1" title="Explicit edge nodes, not linked to this cluster's ingress profile. Link it from the Endpoints tab.">
            <Badge tone="warn">explicit edges</Badge>
          </div>
        )}
        {health && (
          <div className="mt-1" title={health === "down" ? "No upstream is reachable: every node is offline or the service isn't running" : `${downN} of ${hops.length} upstreams are offline or not running`}>
            <Badge tone={health === "down" ? "bad" : "warn"}>{health === "down" ? "down" : `degraded · ${hops.length - downN}/${hops.length} up`}</Badge>
          </div>
        )}
      </div>

      {/* Edges */}
      <div className="flex min-w-0 flex-col items-start gap-1">
        <Label>Edge</Label>
        {ep.edges.map((e) => (
          <Pill key={e.server_id} id={`edge:${e.server_id}`} reg={reg} tone={edgeTone(e)} down={edgeOffline(e.server_id) ? "node offline" : undefined} icon={<Server className="size-3" />} to={{ kind: "node", id: e.server_id }} rate={rateOf(`edge:${e.server_id}`)}>
            {e.name ?? e.server_id.slice(0, 8)}
          </Pill>
        ))}
        {!ep.edges.length && <span className="text-xs text-warn">No edge nodes</span>}
      </div>

      {/* Targets */}
      <div className="flex min-w-0 flex-col items-start gap-1">
        <Label>Target</Label>
        {ep.targets.map((t, i) => {
          const tt = t.target;
          return (
            <div key={i} className="min-w-0 max-w-full">
              <Pill
                id={`target:${i}`}
                reg={reg}
                standby={t.standby}
                icon={tt.kind === "app" ? <Layers className="size-3" /> : tt.kind === "node" ? <Server className="size-3" /> : tt.kind === "swarm" ? <Boxes className="size-3" /> : <Globe className="size-3" />}
                to={tt.kind === "app" ? { kind: "app", id: tt.service_id } : tt.kind === "node" ? { kind: "node", id: tt.server_id } : undefined}
                sub={tt.kind === "app" ? `:${tt.port}` : undefined}
                title={t.internal_name ?? undefined}
              >
                {tt.kind === "swarm" ? `${tt.service || "routing mesh"} (swarm :${tt.port})` : t.label}
              </Pill>
              {t.internal_name && <div className="truncate pl-1 font-mono text-[10px] text-faint">{t.internal_name}</div>}
            </div>
          );
        })}
        {!ep.targets.length && <span className="text-xs text-faint">Fixed upstreams</span>}
      </div>

      {/* Upstream nodes */}
      <div className="flex min-w-0 flex-col items-start gap-1">
        <Label>Runs on</Label>
        {ep.targets.flatMap((t, i) =>
          t.upstreams.length
            ? t.upstreams.map((u, j) => {
                const hs = hopState(t, u, nodeOf(u.server_id));
                return (
                <Pill
                  key={`${i}:${j}`}
                  id={`up:${i}:${j}`}
                  reg={reg}
                  tone={hopTone(hs)}
                  down={hs && isDown(hs) ? downLabel(hs) : undefined}
                  standby={u.standby}
                  role={roleOf(t, u, apps)}
                  icon={<Server className="size-3" />}
                  to={u.server_id ? { kind: "node", id: u.server_id } : undefined}
                  sub={u.address.includes(":") ? `:${u.address.split(":").pop()}` : undefined}
                  title={`${u.address}${hs ? ` · ${hs.label}: ${hs.why}` : ""}`}
                  rate={rateOf(`up:${i}:${j}`)}
                >
                  {u.node ?? u.address}
                </Pill>
                );
              })
            : [
                <span key={`none:${i}`} className="text-xs text-warn">
                  {t.label}: not placed on any node
                </span>,
              ],
        )}
      </div>
    </div>
  );
}

const Label = ({ children }: { children: ReactNode }) => <div className="mb-0.5 text-[10px] font-semibold tracking-widest text-faint uppercase md:hidden">{children}</div>;

/**
 * How a request travels, one slim row per endpoint: domain → edge node(s) →
 * target → the nodes Nginx forwards to. App and node names are links.
 */
export function Routing({ clusterId, live }: { clusterId: string; live?: LiveResponse }) {
  const q = useQuery({
    queryKey: ["cluster-routing", clusterId],
    queryFn: () => get<{ endpoints: RouteEndpoint[] }>(`/api/v1/clusters/${clusterId}/routing`),
    refetchInterval: 30_000,
  });
  const servers = useServers().data;
  const nodes = new Map((Array.isArray(servers) ? servers : []).map((s) => [s.id, s] as const));
  // Placement roles for the replica/standby chips (one list query, shared with the cluster's Services page).
  const appList = useServices(undefined, clusterId).data;
  const apps = new Map((Array.isArray(appList) ? appList : []).map((a) => [a.id, a] as const));
  if (q.isLoading) return <Skeleton />;
  if (q.isError) return <ErrorNote error={q.error} />;
  const list = Array.isArray(q.data?.endpoints) ? q.data.endpoints : [];
  return (
    <div className="max-w-5xl space-y-2">
      {list.length === 0 ? (
        <Empty icon={<Globe className="size-5" />} title="No endpoints yet">
          Add an endpoint in the Endpoints tab; its route shows up here.
        </Empty>
      ) : (
        <div className="overflow-hidden rounded-lg border border-line bg-panel">
          <div className="hidden grid-cols-[minmax(0,1fr)_minmax(0,1fr)_minmax(0,1.1fr)_minmax(0,1.3fr)] gap-x-10 border-b border-line bg-panel-2/40 px-3 py-1.5 text-[10px] font-semibold tracking-widest text-faint uppercase md:grid">
            <span>Domain</span>
            <span>Edge</span>
            <span>Target</span>
            <span>Runs on</span>
          </div>
          {list.map((ep) => (
            <Row key={ep.id} ep={ep} live={live} nodes={nodes} apps={apps} />
          ))}
        </div>
      )}
      <div className="flex flex-wrap items-center justify-between gap-2 pt-1 text-[11px] text-faint">
        <span>Solid: active route · dashed: standby · replica / standby: the node's placement role · dimmed: node offline or service not running · rates are the last minute of traffic (refreshed every 10 s; hover for details) · names open the app or node</span>
        <Link to="/clusters/$clusterId" params={{ clusterId }} search={{ tab: "network" }} className="inline-flex items-center gap-1.5 text-accent hover:underline">
          <Network className="size-3.5" /> Cluster network
        </Link>
      </div>
    </div>
  );
}
