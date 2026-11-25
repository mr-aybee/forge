import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { ArrowUpRight, CornerDownRight, Folder, Globe, Layers, Lock, Network, Server } from "lucide-react";
import { type ReactNode, useCallback, useLayoutEffect, useRef, useState } from "react";

import { get } from "../../api/client";
import { useClusters, useServices, useServers } from "../../api/hooks";
import { type RedirectRule, type RedirectTo, type RouteRule, type RuleKind, type Site, type StaticRoot, type Target, ruleKindLabel, useSites } from "../../api/ingress";
import { nodeName, type Service, type ServiceInstance, type Server as Node } from "../../api/types";
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
  target: { kind: "app"; service_id: string; port: number } | { kind: "node"; server_id: string; port: number } | { kind: "external"; address: string };
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
  if (t.target.kind === "external") return undefined;
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

/** A node a target runs on (an upstream hop). */
interface ViewHop {
  key: string;
  serverId: string | null;
  name: string;
  /** `ip:port`, when the server resolved it (top-level targets only). */
  address?: string;
  resolved: string[];
  standby: boolean;
  role?: Role;
  state?: LiveState;
  weight?: number;
}
/** One target of a path rule, resolved for display. */
interface ViewTarget {
  key: string;
  kind: "app" | "node" | "external" | "redirect" | "static";
  label: string;
  sub?: string;
  to?: { kind: "app" | "node"; id: string };
  internal?: string | null;
  standby: boolean;
  hops: ViewHop[];
  title?: string;
}
interface ViewRule {
  key: string;
  path: string;
  match: string;
  kind: RuleKind;
  targets: ViewTarget[];
}

const kindTone: Record<RuleKind, "accent" | "muted" | "warn" | "ok"> = { proxy: "accent", balancer: "ok", redirect: "warn", static: "muted" };
const arr = <T,>(v: unknown): T[] => (Array.isArray(v) ? (v as T[]) : []);
const appRole = (r: string | undefined): Role => (r === "standby" ? "standby" : "replica");

/** Rule targets the routing API doesn't expand: resolve from the node and app lists. */
function resolveTarget(t: Target | undefined, standby: boolean, key: string, nodes: Map<string, Node>, apps: Map<string, Service>): ViewTarget | null {
  if (!t || typeof t !== "object") return null;
  if (t.kind === "app") {
    const a = apps.get(t.service_id);
    const hops = arr<ServiceInstance>(a?.instances).map((i, j) => {
      const n = nodes.get(i.server_id);
      return {
        key: `${key}:${j}`,
        serverId: i.server_id,
        name: n ? nodeName(n) : i.server_id.slice(0, 8),
        resolved: [],
        standby: standby || i.role === "standby",
        role: appRole(i.role),
        state: liveState({ status: "deployed", state: i.state ?? null, last_error: null }, n),
      } satisfies ViewHop;
    });
    return { key, kind: "app", label: a?.name ?? t.service_id.slice(0, 8), sub: `:${t.port}`, to: { kind: "app", id: t.service_id }, standby, hops };
  }
  if (t.kind === "node") {
    const n = nodes.get(t.server_id);
    return { key, kind: "node", label: n ? nodeName(n) : t.server_id.slice(0, 8), sub: `:${t.port}`, to: { kind: "node", id: t.server_id }, standby, hops: [] };
  }
  if (t.kind === "external") return { key, kind: "external", label: t.address ?? "—", sub: "external", standby, hops: [], title: t.address };
  return null;
}

/** Top-level targets from the routing API (with upstream addresses, state and traffic keys). */
function fromRouting(t: RouteTarget, key: string, nodes: Map<string, Node>, apps: Map<string, Service>, weights: Record<string, number>): ViewTarget {
  const tt = t.target;
  const kind = tt?.kind ?? "external";
  const hops: ViewHop[] =
    kind === "external"
      ? []
      : arr<RouteTarget["upstreams"][number]>(t.upstreams).map((u, j) => ({
          key: `${key}:${j}`,
          serverId: u.server_id,
          name: u.node ?? u.address,
          address: u.address,
          resolved: arr<string>(u.resolved),
          standby: !!u.standby,
          role: roleOf(t, u, apps),
          state: hopState(t, u, u.server_id ? nodes.get(u.server_id) : undefined),
          weight: weights[u.address],
        }));
  return {
    key,
    kind,
    label: kind === "external" ? (tt?.kind === "external" ? tt.address : t.label) : t.label,
    sub: tt?.kind === "external" ? "external" : tt && "port" in tt ? `:${tt.port}` : undefined,
    to: tt?.kind === "app" ? { kind: "app", id: tt.service_id } : tt?.kind === "node" ? { kind: "node", id: tt.server_id } : undefined,
    internal: t.internal_name,
    standby: !!t.standby,
    hops,
    title: t.internal_name ?? undefined,
  };
}

/** Redirect / static rules end in a pseudo-target that says where requests go. */
function terminal(kind: RuleKind, key: string, redirect?: RedirectTo, root?: StaticRoot): ViewTarget[] {
  if (kind === "redirect")
    return [{ key, kind: "redirect", label: redirect?.to || "—", sub: redirect?.code ? String(redirect.code) : undefined, standby: false, hops: [], title: redirect ? `${redirect.code} → ${redirect.to}${redirect.keep_path ? " (keeps the path)" : ""}` : "No destination" }];
  if (kind === "static")
    return [{ key, kind: "static", label: root?.root || "—", sub: root?.spa ? "SPA" : undefined, standby: false, hops: [], title: root ? `Files from ${root.root}${root.index ? ` · index ${root.index}` : ""}` : "No root directory" }];
  return [];
}

/** The endpoint's path rules: the default `/` rule plus `spec.rules`. Old rows (no site spec) get one `/` proxy rule. */
function rulesOf(ep: RouteEndpoint, site: Site | undefined, nodes: Map<string, Node>, apps: Map<string, Service>): ViewRule[] {
  const spec = site?.config.spec;
  const weights = (spec as { weights?: Record<string, number> } | undefined)?.weights ?? {};
  const kind: RuleKind = spec?.kind ?? (arr<RouteTarget>(ep.targets).some((t) => t?.standby) ? "balancer" : "proxy");
  const base: ViewRule = {
    key: "r0",
    path: "/",
    match: "prefix",
    kind,
    targets:
      kind === "redirect" || kind === "static"
        ? terminal(kind, "r0:0", spec?.redirect, spec?.static_root)
        : arr<RouteTarget>(ep.targets).map((t, i) => fromRouting(t, `r0:${i}`, nodes, apps, weights)),
  };
  const extra = arr<RouteRule>(spec?.rules).map((r, k): ViewRule => {
    const key = `r${k + 1}`;
    const rk: RuleKind = r?.kind ?? "proxy";
    const targets =
      rk === "redirect" || rk === "static"
        ? terminal(rk, `${key}:0`, r.redirect, r.static_root)
        : [...arr<Target>(r?.targets).map((t, i) => resolveTarget(t, false, `${key}:${i}`, nodes, apps)), ...arr<Target>(r?.standby_targets).map((t, i) => resolveTarget(t, true, `${key}:s${i}`, nodes, apps))].filter((x): x is ViewTarget => !!x);
    return { key, path: r?.path || "/", match: r?.match_kind ?? "prefix", kind: rk, targets };
  });
  return [base, ...extra];
}

const targetIcon = (k: ViewTarget["kind"]) =>
  k === "app" ? <Layers className="size-3" /> : k === "node" ? <Server className="size-3" /> : k === "redirect" ? <CornerDownRight className="size-3" /> : k === "static" ? <Folder className="size-3" /> : <Globe className="size-3" />;

/** One endpoint: a slim row, with thin connectors drawn between the pills. */
function Row({ ep, site, live, nodes, apps, clusterName }: { ep: RouteEndpoint; site?: Site; live?: LiveResponse; nodes: Map<string, Node>; apps: Map<string, Service>; clusterName: (id: string) => string }) {
  const wrap = useRef<HTMLDivElement>(null);
  const els = useRef(new Map<string, HTMLElement>());
  const [paths, setPaths] = useState<{ d: string; dashed?: boolean; tone?: Tone }[]>([]);
  const reg = useCallback<Reg>((id) => (el) => {
    if (el) els.current.set(id, el);
    else els.current.delete(id);
  }, []);

  const edges = arr<RouteEndpoint["edges"][number]>(ep.edges);
  const domains = arr<string>(ep.domains).length ? arr<string>(ep.domains) : arr<string>(site?.config.spec.domains);
  const redirects = arr<RedirectRule>(site?.config.spec.redirects);
  const rules = rulesOf(ep, site, nodes, apps);
  const nodeOf = (id: string | null) => (id ? nodes.get(id) : undefined);
  const edgeOffline = (id: string) => {
    const st = nodeOf(id)?.status;
    return st === "offline" || st === "revoked";
  };
  const edgeTone = (e: RouteEndpoint["edges"][number]): Tone => {
    if (edgeOffline(e.server_id)) return "bad";
    const st = live?.edges?.[e.server_id]?.state;
    if (st) return st.nginx === "running" ? "ok" : "bad";
    return e.status === "applied" ? "ok" : e.status === "failed" ? "bad" : "idle";
  };

  // Last minute of traffic for this endpoint, per edge (keyed by its first domain).
  const domain = domains[0] ?? "";
  const siteOn = (edge: string): SiteTraffic | undefined => live?.edges?.[edge]?.state?.traffic?.[domain];
  const anyTraffic = edges.some((e) => siteOn(e.server_id));
  const edgeRate = (id: string) => {
    const t = siteOn(id);
    return t ? { text: rate(t.in_bps + t.out_bps), title: `${t.rps.toFixed(2)} req/s · in ${rate(t.in_bps)} · out ${rate(t.out_bps)} (last minute)` } : undefined;
  };
  const hopRate = (h: ViewHop) => {
    if (!h.address) return undefined;
    const keys = new Set([h.address, ...h.resolved]);
    let sent = 0;
    let recv = 0;
    let reqs = 0;
    let seen = false;
    for (const e of edges)
      for (const [addr, v] of Object.entries(siteOn(e.server_id)?.upstreams ?? {}))
        if (keys.has(addr)) {
          seen = true;
          sent += v.sent_bps;
          recv += v.received_bps;
          reqs += v.requests;
        }
    if (seen) return { text: rate(sent + recv), title: `${h.name}: ${reqs} requests · to app ${rate(sent)} · from app ${rate(recv)} (last minute)` };
    return anyTraffic ? { text: "idle" } : undefined;
  };

  const wires: Wire[] = [];
  for (const e of edges) wires.push({ from: "domain", to: `edge:${e.server_id}`, tone: edgeTone(e) });
  for (const r of rules) {
    for (const e of edges) wires.push({ from: `edge:${e.server_id}`, to: `rule:${r.key}` });
    for (const t of r.targets) {
      const down = t.hops.length > 0 && t.hops.every((h) => isDown(h.state));
      wires.push({ from: `rule:${r.key}`, to: `t:${t.key}`, dashed: t.standby, tone: down ? "bad" : undefined });
    }
  }
  const key = JSON.stringify(wires);

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
        next.push({ d: `M ${x1} ${y1} C ${x1 + c} ${y1}, ${x2 - c} ${y2}, ${x2} ${y2}`, dashed: w.dashed, tone: w.tone });
      }
      setPaths(next);
    };
    draw();
    const ro = new ResizeObserver(draw);
    ro.observe(box);
    return () => ro.disconnect();
  }, [key]);

  // Endpoint health from its upstreams: degraded when some are down, down when none is up.
  const hops = rules.flatMap((r) => r.targets.flatMap((t) => t.hops.map((h) => h.state)));
  const downN = hops.filter(isDown).length;
  const health: "down" | "degraded" | undefined = hops.length && downN === hops.length ? "down" : downN > 0 ? "degraded" : undefined;
  const tlsOn = !!ep.tls && ep.tls !== "off";
  const p = site?.config.placement;
  const placement = p
    ? p.kind === "cluster"
      ? { text: `${clusterName(p.cluster_id)} edge group`, title: `Cluster placement: served by every edge of ${clusterName(p.cluster_id)}'s ingress profile` }
      : { text: arr<string>(p.server_ids).map((id) => { const n = nodes.get(id); return n ? nodeName(n) : id.slice(0, 8); }).join(", ") || "—", title: "Placed on these nodes" }
    : ep.source === "edges"
      ? { text: "nodes", title: "Placed on single nodes in this cluster" }
      : undefined;
  const mid = `m-${ep.id}`;
  return (
    <div ref={wrap} className="relative grid gap-2 border-b border-line px-3 py-2.5 last:border-b-0 hover:bg-panel-2/30 md:grid-cols-[minmax(0,1fr)_minmax(0,0.8fr)_minmax(0,0.8fr)_minmax(0,1.6fr)] md:items-center md:gap-x-10">
      <svg className="pointer-events-none absolute inset-0 z-0 h-full w-full overflow-visible" aria-hidden>
        <defs>
          {(["ok", "warn", "bad"] as const).map((t) => (
            <marker key={t} id={`${mid}-${t}`} viewBox="0 0 10 10" refX="9" refY="5" markerWidth="6" markerHeight="6" orient="auto">
              <path d="M 0 0 L 10 5 L 0 10 z" fill={stroke(t)} />
            </marker>
          ))}
        </defs>
        {paths.map((pa, i) => (
          <path
            key={i}
            d={pa.d}
            fill="none"
            stroke={stroke(pa.tone)}
            strokeOpacity={pa.dashed ? 0.45 : 0.7}
            strokeWidth={1.25}
            strokeDasharray={pa.dashed ? "4 3" : undefined}
            markerEnd={`url(#${mid}-${pa.tone === "bad" || pa.tone === "warn" ? pa.tone : "ok"})`}
          />
        ))}
      </svg>

      {/* Domain */}
      <div className="min-w-0">
        <Label>Domain</Label>
        <Pill id="domain" reg={reg} icon={tlsOn ? <Lock className="size-3 text-ok" /> : <Globe className="size-3" />} title={domains.join(", ")}>
          {domains[0] ?? ep.name ?? "—"}
        </Pill>
        {domains.slice(1).map((d) => (
          <div key={d} className="truncate pl-1 font-mono text-[10.5px] text-muted" title={d}>
            + {d}
          </div>
        ))}
        {redirects.map((r, i) => (
          <div key={i} className="truncate pl-1 font-mono text-[10.5px] text-faint" title={`${r.from} → ${r.to} (${r.code})${r.keep_path ? ", keeps the path" : ""}`}>
            {r.from} → {r.to} <span className="text-warn">{r.code}</span>
          </div>
        ))}
        <div className="mt-1 flex flex-wrap items-center gap-1">
          {placement && (
            <span className="min-w-0 truncate rounded bg-panel-2 px-1.5 py-0.5 text-[10.5px] text-muted" title={placement.title}>
              {placement.text}
            </span>
          )}
          {health && (
            <span title={health === "down" ? "No upstream is reachable: every node is offline or the service isn't running" : `${downN} of ${hops.length} upstreams are offline or not running`}>
              <Badge tone={health === "down" ? "bad" : "warn"}>{health === "down" ? "down" : `degraded · ${hops.length - downN}/${hops.length} up`}</Badge>
            </span>
          )}
        </div>
      </div>

      {/* Edges */}
      <div className="flex min-w-0 flex-col items-start gap-1">
        <Label>Edge</Label>
        {edges.map((e) => (
          <Pill key={e.server_id} id={`edge:${e.server_id}`} reg={reg} tone={edgeTone(e)} down={edgeOffline(e.server_id) ? "node offline" : undefined} icon={<Server className="size-3" />} to={{ kind: "node", id: e.server_id }} rate={edgeRate(e.server_id)}>
            {e.name ?? e.server_id.slice(0, 8)}
          </Pill>
        ))}
        {!edges.length && <span className="text-xs text-warn">No edge nodes</span>}
      </div>

      {/* Path rules */}
      <div className="flex min-w-0 flex-col items-start gap-1">
        <Label>Path</Label>
        {rules.map((r) => (
          <Pill key={r.key} id={`rule:${r.key}`} reg={reg} title={`${r.match} match · ${ruleKindLabel(r.kind)}`} sub={r.match === "prefix" ? undefined : r.match === "exact" ? "=" : "~"}>
            <span className="font-mono">{r.path}</span>{" "}
            <span className={`rounded px-1 text-[10px] font-medium ${kindTone[r.kind] === "accent" ? "bg-accent-soft text-accent" : kindTone[r.kind] === "ok" ? "bg-ok-soft text-ok" : kindTone[r.kind] === "warn" ? "bg-warn-soft text-warn" : "bg-panel-2 text-muted"}`}>{r.kind}</span>
          </Pill>
        ))}
      </div>

      {/* Targets → the nodes they run on */}
      <div className="flex min-w-0 flex-col items-start gap-1">
        <Label>Target → runs on</Label>
        {rules.map((r) => (
          <div key={r.key} className="flex min-w-0 max-w-full flex-col items-start gap-1">
            {r.targets.map((t) => (
              <div key={t.key} className="flex min-w-0 max-w-full flex-wrap items-center gap-1">
                <Pill id={`t:${t.key}`} reg={reg} standby={t.standby} icon={targetIcon(t.kind)} to={t.to} sub={t.sub} title={t.title}>
                  {t.label}
                </Pill>
                {t.hops.map((h) => (
                  <Pill
                    key={h.key}
                    id={`h:${h.key}`}
                    reg={reg}
                    tone={hopTone(h.state)}
                    down={h.state && isDown(h.state) ? downLabel(h.state) : undefined}
                    standby={h.standby}
                    role={t.kind === "app" ? h.role : undefined}
                    icon={<Server className="size-3" />}
                    to={h.serverId ? { kind: "node", id: h.serverId } : undefined}
                    sub={h.weight !== undefined && h.weight !== 100 ? `w${h.weight}` : undefined}
                    title={`${h.address ?? h.name}${h.weight !== undefined ? ` · weight ${h.weight}` : ""}${h.state ? ` · ${h.state.label}: ${h.state.why}` : ""}`}
                    rate={hopRate(h)}
                  >
                    {h.name}
                  </Pill>
                ))}
                {t.kind === "app" && !t.hops.length && <span className="text-[11px] text-warn">not placed on any node</span>}
                {t.internal && <span className="w-full truncate pl-1 font-mono text-[10px] text-faint" title={t.internal}>{t.internal}</span>}
              </div>
            ))}
            {!r.targets.length && <span className="text-xs text-warn">No targets</span>}
          </div>
        ))}
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
  // Path rules, redirects and placement come from the endpoint itself; older servers without it still render.
  const siteList = useSites().data;
  const sites = new Map((Array.isArray(siteList) ? siteList : []).map((s) => [s.id, s] as const));
  const clusterList = useClusters().data;
  const clusterName = (id: string) => (Array.isArray(clusterList) ? clusterList.find((c) => c?.id === id)?.name : undefined) ?? id.slice(0, 8);
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
          <div className="hidden grid-cols-[minmax(0,1fr)_minmax(0,0.8fr)_minmax(0,0.8fr)_minmax(0,1.6fr)] gap-x-10 border-b border-line bg-panel-2/40 px-3 py-1.5 text-[10px] font-semibold tracking-widest text-faint uppercase md:grid">
            <span>Domain</span>
            <span>Edge</span>
            <span>Path</span>
            <span>Target → runs on</span>
          </div>
          {list.map((ep) => (
            <Row key={ep.id} ep={ep} site={sites.get(ep.id)} live={live} nodes={nodes} apps={apps} clusterName={clusterName} />
          ))}
        </div>
      )}
      <div className="flex flex-wrap items-center justify-between gap-2 pt-1 text-[11px] text-faint">
        <span>Solid: active route · dashed: standby · w: weight · replica / standby: the node's placement role · dimmed: node offline or service not running · rates are the last minute of traffic (refreshed every 10 s; hover for details) · names open the app or node</span>
        <Link to="/clusters/$clusterId" params={{ clusterId }} search={{ tab: "network" }} className="inline-flex items-center gap-1.5 text-accent hover:underline">
          <Network className="size-3.5" /> Cluster network
        </Link>
      </div>
    </div>
  );
}
