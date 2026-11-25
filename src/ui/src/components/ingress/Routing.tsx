import { useQuery } from "@tanstack/react-query";
import { Link, useNavigate, useSearch } from "@tanstack/react-router";
import { ArrowUpRight, Box, CornerDownRight, ExternalLink, Folder, Globe, Layers, Lock, Network, Server } from "lucide-react";
import { type ReactNode, useCallback, useLayoutEffect, useRef, useState } from "react";

import { get } from "../../api/client";
import { type ContainerSummary, nodeAddresses, useClusters, useNodeContainers, useServices, useServers } from "../../api/hooks";
import { type RedirectRule, type RedirectTo, type RouteRule, type RuleKind, type Site, type StaticRoot, type Target, RULE_KINDS, ruleKindLabel, useSites } from "../../api/ingress";
import { nodeName, type Service, type ServiceInstance, type Server as Node } from "../../api/types";
import type { IngressSearch } from "../../app/router";
import { type LiveState, liveState } from "../InstanceState";
import { Badge, CopyIcon, Empty, ErrorNote, SearchInput, Select, Skeleton } from "../ui";
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

/** Where a pill links to. */
type Dest = { kind: "app" | "node"; id: string } | { kind: "container"; node: string; id: string };
type Reg = (id: string) => (el: HTMLElement | null) => void;

/** A slim pill. Pills that name an app or node are links (accent, ↗). */
function Pill({ id, reg, tone, icon, children, sub, to, standby, role, title, rate: flow, down }: { id: string; reg: Reg; tone?: Tone; icon?: ReactNode; children: ReactNode; sub?: string; to?: Dest; standby?: boolean; /** Placement role chip (`replica` / `standby`) for app upstreams. */ role?: Role; title?: string; rate?: { text: string; title?: string }; down?: string }) {
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
  if (to?.kind === "container")
    return (
      <Link ref={reg(id)} to="/nodes/$nodeId/containers/$containerId" params={{ nodeId: to.node, containerId: to.id }} className={cls} title={title ?? "Open the container"}>
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
  /** `ip:port`, when known: matches the edge's traffic counters. */
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
  kind: "app" | "node" | "container" | "external" | "redirect" | "static";
  label: string;
  /** Ports: `:8080`, or published → internal for containers (`127.0.0.1:18081 → :80`). */
  ports?: string;
  /** A second, faint line (internal name, image, swarm tasks). */
  detail?: string;
  to?: Dest;
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

/** What a rule's targets are resolved against. */
interface Ctx {
  nodes: Map<string, Node>;
  apps: Map<string, Service>;
  containers: Map<string, ContainerSummary[]>;
  /** Node id by every address it is known by. */
  byAddr: Map<string, string>;
  /** The endpoint's edges: a loopback upstream is on the edge itself. */
  edges: string[];
}

const kindTone: Record<RuleKind, string> = { proxy: "bg-accent-soft text-accent", balancer: "bg-ok-soft text-ok", redirect: "bg-warn-soft text-warn", static: "bg-panel-2 text-muted" };
/** Whether a redirect source (`host` or a URL) names domain `d`. */
const sameHost = (from: string | undefined, d: string) => !!from && !!d && from.replace(/^https?:\/\//, "").split("/")[0]?.toLowerCase() === d.toLowerCase();
/** Endpoint health from its upstreams: degraded when some are down, down when none is up. */
function healthOf(rules: ViewRule[]): { health?: "down" | "degraded"; downN: number; total: number } {
  const hops = rules.flatMap((r) => r.targets.flatMap((t) => t.hops.map((h) => h.state)));
  const downN = hops.filter(isDown).length;
  return { health: hops.length && downN === hops.length ? "down" : downN > 0 ? "degraded" : undefined, downN, total: hops.length };
}

/** The right-most edge of an element and its direct children (icons may overflow a truncated group). */
function rightEdge(el: HTMLElement | undefined): DOMRect | undefined {
  if (!el) return undefined;
  const r = el.getBoundingClientRect();
  const right = Math.max(r.right, ...[...el.children].map((c) => c.getBoundingClientRect().right));
  return new DOMRect(r.left, r.top, right - r.left, r.height);
}

/** Domain pills shown before the "+N more" chip. */
const DOMAINS_SHOWN = 3;
const arr = <T,>(v: unknown): T[] => (Array.isArray(v) ? (v as T[]) : []);
const appRole = (r: string | undefined): Role => (r === "standby" ? "standby" : "replica");
const isLoopback = (h: string) => h === "localhost" || h === "::1" || h.startsWith("127.");
const isWildcard = (h: string) => h === "" || h === "0.0.0.0" || h === "::" || h === "[::]";
const nameOf = (ctx: Ctx, id: string) => {
  const n = ctx.nodes.get(id);
  return n ? nodeName(n) : id.slice(0, 8);
};

/** `host:port` / `[v6]:port` → parts; `port` is 0 when absent. */
function splitAddr(a: string): { host: string; port: number } {
  const m = /^\[([^\]]+)\]:(\d+)$/.exec(a) ?? /^([^:]+):(\d+)$/.exec(a);
  return m ? { host: m[1] ?? "", port: Number(m[2]) } : { host: a, port: 0 };
}

/** A container on `node` publishing host port `port` reachable at `host` (Docker's `bind:host->container/proto`). */
function containerAt(ctx: Ctx, node: string, host: string, port: number): { c: ContainerSummary; bind: string; inner: number } | undefined {
  if (!port) return undefined;
  for (const c of ctx.containers.get(node) ?? [])
    for (const p of c.ports) {
      const m = /^(.*):(\d+)->(\d+)\/\w+$/.exec(p);
      if (!m || Number(m[2]) !== port) continue;
      const bind = m[1] ?? "";
      if (isWildcard(bind) || bind === host || (isLoopback(bind) && isLoopback(host)) || !host) return { c, bind, inner: Number(m[3]) };
    }
  return undefined;
}

/** A node-and-port target: the container publishing that port when there is one, else the node itself. */
function onNode(ctx: Ctx, node: string, host: string, port: number, base: Pick<ViewTarget, "key" | "standby"> & { weight?: number; address?: string; state?: LiveState }): ViewTarget {
  const hop: ViewHop = { key: `${base.key}:n`, serverId: node, name: nameOf(ctx, node), address: base.address, resolved: [], standby: base.standby, weight: base.weight, state: base.state };
  const hit = containerAt(ctx, node, host, port);
  if (hit) {
    const swarm = hit.c.labels?.["com.docker.swarm.service.name"];
    const tasks = swarm ? (ctx.containers.get(node) ?? []).filter((x) => x.labels?.["com.docker.swarm.service.name"] === swarm).length : 0;
    const published = `${isWildcard(hit.bind) ? "" : hit.bind}:${port}`;
    return {
      ...base,
      kind: "container",
      label: swarm ?? hit.c.name,
      ports: `${published} → :${hit.inner}`,
      detail: [swarm ? `swarm service · ${tasks} task${tasks === 1 ? "" : "s"} here · ${hit.c.name}` : undefined, hit.c.image].filter(Boolean).join(" · "),
      to: { kind: "container", node, id: hit.c.id },
      hops: [hop],
      title: `Container ${hit.c.name} (${hit.c.image}) on ${hop.name}: host ${published} → container :${hit.inner} · ${hit.c.state}`,
    };
  }
  return { ...base, kind: "node", label: hop.name, ports: port ? `:${port}` : undefined, to: { kind: "node", id: node }, hops: [], title: `${hop.name}${port ? ` port ${port}` : ""}${base.address ? ` (${base.address})` : ""}` };
}

/** A plain `host:port` upstream (external target or a row saved before typed targets): resolved to a node and container when its host is one of ours. */
function fromAddress(ctx: Ctx, address: string, standby: boolean, key: string, weight?: number): ViewTarget {
  const { host, port } = splitAddr(address);
  const node = isLoopback(host) ? ctx.edges[0] : ctx.byAddr.get(host);
  if (node) return onNode(ctx, node, host, port, { key, standby, weight, address });
  return { key, kind: "external", label: host || address || "—", ports: port ? `:${port}` : undefined, detail: "external", standby, hops: [], title: `${address} (outside Forge${weight !== undefined ? `, weight ${weight}` : ""})` };
}

/** A typed target (`forge_catalog::ingress::Target`). */
function fromTarget(ctx: Ctx, t: Target | undefined, standby: boolean, key: string): ViewTarget | null {
  if (!t || typeof t !== "object") return null;
  if (t.kind === "app") {
    const a = ctx.apps.get(t.service_id);
    const hops = arr<ServiceInstance>(a?.instances).map(
      (i, j): ViewHop => ({
        key: `${key}:${j}`,
        serverId: i.server_id,
        name: nameOf(ctx, i.server_id),
        resolved: [],
        standby: standby || i.role === "standby",
        role: appRole(i.role),
        state: liveState({ status: "deployed", state: i.state ?? null, last_error: null }, ctx.nodes.get(i.server_id)),
      }),
    );
    return { key, kind: "app", label: a?.name ?? t.service_id.slice(0, 8), ports: `:${t.port}`, to: { kind: "app", id: t.service_id }, standby, hops };
  }
  if (t.kind === "node") return onNode(ctx, t.server_id, "", t.port, { key, standby });
  if (t.kind === "external") return fromAddress(ctx, t.address ?? "", standby, key);
  return null;
}

/** Top-level targets from the routing API: app targets carry upstream addresses, state and traffic keys. */
function fromRouting(ctx: Ctx, t: RouteTarget, key: string, weights: Record<string, number>): ViewTarget | null {
  const tt = t?.target;
  if (tt?.kind !== "app") return fromTarget(ctx, tt, !!t?.standby, key);
  const hops = arr<RouteTarget["upstreams"][number]>(t.upstreams).map(
    (u, j): ViewHop => ({
      key: `${key}:${j}`,
      serverId: u.server_id,
      name: u.node ?? u.address,
      address: u.address,
      resolved: arr<string>(u.resolved),
      standby: !!u.standby,
      role: roleOf(t, u, ctx.apps),
      state: hopState(t, u, u.server_id ? ctx.nodes.get(u.server_id) : undefined),
      weight: weights[u.address],
    }),
  );
  return { key, kind: "app", label: t.label, ports: `:${tt.port}`, detail: t.internal_name ?? undefined, to: { kind: "app", id: tt.service_id }, standby: !!t.standby, hops, title: t.internal_name ?? undefined };
}

/** Redirect / static rules end in a pseudo-target that says where requests go. */
function terminal(kind: RuleKind, key: string, redirect?: RedirectTo, root?: StaticRoot): ViewTarget[] {
  if (kind === "redirect")
    return [{ key, kind: "redirect", label: redirect?.to || "—", ports: redirect?.code ? String(redirect.code) : undefined, standby: false, hops: [], title: redirect ? `${redirect.code} → ${redirect.to}${redirect.keep_path ? " (keeps the path)" : ""}` : "No destination" }];
  if (kind === "static")
    return [{ key, kind: "static", label: root?.root || "—", ports: root?.spa ? "SPA" : undefined, standby: false, hops: [], title: root ? `Files from ${root.root}${root.index ? ` · index ${root.index}` : ""}` : "No root directory" }];
  return [];
}

/** Typed targets when present, else the plain upstream lists (rows saved before typed targets). */
function targetsOf(ctx: Ctx, key: string, typed: { active: Target[]; standby: Target[] }, plain: { active: string[]; standby: string[] }, weights: Record<string, number>): ViewTarget[] {
  if (typed.active.length || typed.standby.length)
    return [...typed.active.map((t, i) => fromTarget(ctx, t, false, `${key}:${i}`)), ...typed.standby.map((t, i) => fromTarget(ctx, t, true, `${key}:s${i}`))].filter((x): x is ViewTarget => !!x);
  return [
    ...plain.active.filter((a) => typeof a === "string").map((a, i) => fromAddress(ctx, a, false, `${key}:${i}`, weights[a])),
    ...plain.standby.filter((a) => typeof a === "string").map((a, i) => fromAddress(ctx, a, true, `${key}:s${i}`)),
  ];
}

/** The endpoint's path rules: the default `/` rule plus `spec.rules`. Old rows (no site spec) get one `/` rule from the routing API. */
function rulesOf(ctx: Ctx, ep: RouteEndpoint, site: Site | undefined): ViewRule[] {
  const spec = site?.config.spec;
  const weights = (spec as { weights?: Record<string, number> } | undefined)?.weights ?? {};
  const routed = arr<RouteTarget>(ep.targets);
  const kind: RuleKind = spec?.kind ?? (routed.some((t) => t?.standby) || arr(spec?.standby).length ? "balancer" : "proxy");
  const baseTargets =
    kind === "redirect" || kind === "static"
      ? terminal(kind, "r0:0", spec?.redirect, spec?.static_root)
      : routed.length
        ? routed.map((t, i) => fromRouting(ctx, t, `r0:${i}`, weights)).filter((x): x is ViewTarget => !!x)
        : targetsOf(ctx, "r0", { active: arr(site?.config.targets), standby: arr(site?.config.standby_targets) }, { active: arr(spec?.upstreams), standby: arr(spec?.standby) }, weights);
  const extra = arr<RouteRule>(spec?.rules).map((r, k): ViewRule => {
    const key = `r${k + 1}`;
    const rk: RuleKind = r?.kind ?? "proxy";
    const targets =
      rk === "redirect" || rk === "static"
        ? terminal(rk, `${key}:0`, r.redirect, r.static_root)
        : targetsOf(ctx, key, { active: arr(r?.targets), standby: arr(r?.standby_targets) }, { active: arr(r?.upstreams), standby: arr(r?.standby) }, {});
    return { key, path: r?.path || "/", match: r?.match_kind ?? "prefix", kind: rk, targets };
  });
  return [{ key: "r0", path: "/", match: "prefix", kind, targets: baseTargets }, ...extra];
}

const targetIcon = (k: ViewTarget["kind"]) =>
  k === "app" ? <Layers className="size-3" /> : k === "node" ? <Server className="size-3" /> : k === "container" ? <Box className="size-3" /> : k === "redirect" ? <CornerDownRight className="size-3" /> : k === "static" ? <Folder className="size-3" /> : <Globe className="size-3" />;

/** A domain with open-in-new-tab and copy-URL buttons. */
function DomainLinks({ domain, tls, open = true }: { domain: string; tls: boolean; /** False for wildcards: nothing to open. */ open?: boolean }) {
  const url = `${tls ? "https" : "http"}://${domain}`;
  return (
    <span className="relative z-10 inline-flex shrink-0 items-center">
      {open && (<a href={url} target="_blank" rel="noopener noreferrer" aria-label={`Open ${url}`} title={`Open ${url}`} className="grid size-5 place-items-center rounded text-faint hover:bg-panel-2 hover:text-fg focus-visible:outline-2 focus-visible:outline-accent">
        <ExternalLink className="size-3" />
      </a>)}
      <CopyIcon text={url} label={`Copy ${url}`} />
    </span>
  );
}

/** One endpoint: a slim row, with thin connectors drawn between the pills. */
function Row({ ep, site, live, ctx, clusterName }: { ep: RouteEndpoint; site?: Site; live?: LiveResponse; ctx: Ctx; clusterName: (id: string) => string }) {
  const { nodes } = ctx;
  const wrap = useRef<HTMLDivElement>(null);
  const els = useRef(new Map<string, HTMLElement>());
  const [paths, setPaths] = useState<{ d: string; dashed?: boolean; tone?: Tone; arrow?: boolean }[]>([]);
  const [moreOpen, setMoreOpen] = useState(false);
  const [redirPos, setRedirPos] = useState<Record<number, { top: number; left: number }>>({});
  const reg = useCallback<Reg>((id) => (el) => {
    if (el) els.current.set(id, el);
    else els.current.delete(id);
  }, []);

  const edges = arr<RouteEndpoint["edges"][number]>(ep.edges);
  const domains = arr<string>(ep.domains).length ? arr<string>(ep.domains) : arr<string>(site?.config.spec.domains);
  const redirects = arr<RedirectRule>(site?.config.spec.redirects);
  const redirectOf = (d: string) => redirects.find((r) => sameHost(r.from, d));
  const rules = rulesOf({ ...ctx, edges: edges.map((e) => e.server_id) }, ep, site);
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
  // Domains: first few as pills, the rest behind a "+N more" chip (one wire source for all of them).
  // Primary first, then domains served here, then redirect sources: their amber wires
  // run below the blue trunk instead of across it.
  const isRedirSrc = (d: string) => redirects.some((r) => sameHost(r.from, d));
  const allDomains = domains.length ? [domains[0] ?? "", ...domains.slice(1).filter((d) => !isRedirSrc(d)), ...domains.slice(1).filter(isRedirSrc)] : [ep.name ?? "—"];
  const shownDomains = moreOpen ? allDomains : allDomains.slice(0, DOMAINS_SHOWN);
  const hiddenDomains = allDomains.slice(shownDomains.length);
  const domainWires = (from: string, d: string) => {
    const ri = redirects.findIndex((r) => sameHost(r.from, d));
    if (ri >= 0) wires.push({ from, to: `redir:${ri}`, tone: "warn" });
    else for (const e of edges) wires.push({ from, to: `edge:${e.server_id}`, tone: edgeTone(e) });
  };
  shownDomains.forEach((d, i) => domainWires(`dom:${i}`, d));
  hiddenDomains.forEach((d) => domainWires("dom:more", d));
  // Which domain element each redirect badge lines up with.
  const redirSrc: Record<number, string> = {};
  for (const w of wires) if (w.to.startsWith("redir:")) redirSrc[Number(w.to.slice(6))] ??= w.from;
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
      if (box.clientWidth < 720) {
        setRedirPos({});
        return setPaths([]);
      }
      const base = box.getBoundingClientRect();
      const next: typeof paths = [];
      // Elbow connectors: out horizontally, one vertical trunk per column gap
      // (fan-outs share it), then horizontally in. Pixel-snapped for 1px-ish strokes.
      const snap = (v: number) => Math.round(v) + 0.5;
      const gap = (id: string) => id.split(":")[0] ?? "";
      // Redirect badges: level with their source domain, in the edge column, unless that
      // would cover an edge pill (then stacked below the edges).
      const place = new Map<number, { top: number; left: number; h: number }>();
      const col = els.current.get("edgecol")?.getBoundingClientRect();
      const edgeBoxes = edges.map((e) => els.current.get(`edge:${e.server_id}`)?.getBoundingClientRect()).filter((r): r is DOMRect => !!r);
      let below = Math.max(0, ...edgeBoxes.map((r) => r.bottom - base.top)) + 4;
      const taken: number[] = [];
      for (const [ri, src] of Object.entries(redirSrc).sort(([a], [b]) => Number(a) - Number(b))) {
        const sr = els.current.get(src)?.getBoundingClientRect();
        const h = els.current.get(`redir:${ri}`)?.getBoundingClientRect().height ?? 22;
        if (!sr || !col) continue;
        let top = sr.top + sr.height / 2 - base.top - h / 2;
        const hits = (t: number) => edgeBoxes.some((r) => t < r.bottom - base.top && t + h > r.top - base.top) || taken.some((t2) => Math.abs(t2 - t) < h);
        if (hits(top)) {
          top = below;
          below += h + 4;
        }
        taken.push(top);
        place.set(Number(ri), { top: Math.round(top), left: Math.round(col.left - base.left), h });
      }
      setRedirPos(Object.fromEntries(place));
      const seg: { w: Wire; x1: number; y1: number; x2: number; y2: number; g: string }[] = [];
      for (const w of JSON.parse(key) as Wire[]) {
        const a = rightEdge(els.current.get(w.from));
        const b = els.current.get(w.to)?.getBoundingClientRect();
        if (!a || !b) continue;
        // A redirect badge is placed level with its domain (below), so its wire is straight.
        const rp = w.to.startsWith("redir:") ? place.get(Number(w.to.slice(6))) : undefined;
        if (rp) {
          seg.push({ w, g: "redir", x1: a.right - base.left + 2, y1: a.top + a.height / 2 - base.top, x2: rp.left - 4, y2: rp.top + rp.h / 2 });
          continue;
        }
        seg.push({ w, g: gap(w.from), x1: a.right - base.left + 2, y1: a.top + a.height / 2 - base.top, x2: b.left - base.left - 4, y2: b.top + b.height / 2 - base.top });
      }
      const trunk = new Map<string, number>();
      for (const g of new Set(seg.map((x) => x.g))) {
        const in_ = seg.filter((x) => x.g === g);
        trunk.set(g, snap((Math.max(...in_.map((x) => x.x1)) + Math.min(...in_.map((x) => x.x2))) / 2));
      }
      // Each wire = a source leg (out to the trunk, along it) + a target leg (trunk → target, with
      // the arrow). Target legs are shared: many sources into one target draw one arrow (a bracket).
      const legs = new Set<string>();
      for (const { w, g, x1: rx1, y1: ry1, x2: rx2, y2: ry2 } of seg) {
        const [x1, y1, x2, y2] = [snap(rx1), snap(ry1), snap(rx2), snap(ry2)];
        const dy = y2 - y1;
        const tx = Math.min(Math.max(trunk.get(g) ?? (x1 + x2) / 2, x1 + 4), x2 - 4);
        const r = Math.max(0, Math.min(4, Math.abs(dy) / 2, tx - x1, x2 - tx));
        const sy = Math.sign(dy);
        const src = Math.abs(dy) < 1 ? `M ${x1} ${y1} H ${tx}` : `M ${x1} ${y1} H ${tx - r} Q ${tx} ${y1} ${tx} ${y1 + sy * r} V ${y2 - sy * r} Q ${tx} ${y2} ${tx + r} ${y2}`;
        next.push({ d: src, dashed: w.dashed, tone: w.tone, arrow: false });
        const leg = `${w.to}|${w.tone ?? ""}|${w.dashed ? 1 : 0}`;
        if (legs.has(leg)) continue;
        legs.add(leg);
        next.push({ d: `M ${tx} ${y2} H ${x2}`, dashed: w.dashed, tone: w.tone, arrow: true });
      }
      setPaths(next);
    };
    draw();
    const ro = new ResizeObserver(draw);
    ro.observe(box);
    return () => ro.disconnect();
  }, [key]);

  // Endpoint health from its upstreams: degraded when some are down, down when none is up.
  const { health, downN, total } = healthOf(rules);
  const hops = { length: total };
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
        {/* Opacity on the group, so overlapping legs of a shared trunk don't darken. */}
        {([false, true] as const).map((dashed) => (
          <g key={String(dashed)} opacity={dashed ? 0.45 : 0.7}>
            {paths
              .filter((pa) => !!pa.dashed === dashed)
              .map((pa, i) => (
                <path
                  key={i}
                  d={pa.d}
                  fill="none"
                  stroke={stroke(pa.tone)}
                  strokeWidth={1.25}
                  strokeDasharray={dashed ? "4 3" : undefined}
                  markerEnd={pa.arrow ? `url(#${mid}-${pa.tone === "bad" || pa.tone === "warn" ? pa.tone : "ok"})` : undefined}
                />
              ))}
          </g>
        ))}
      </svg>

      {/* Domain */}
      <div className="min-w-0">
        <Label>Domain</Label>
        <div className="flex min-w-0 flex-col items-start gap-1">
          {shownDomains.map((d, i) => {
            const wild = d.startsWith("*.");
            return (
              // The wire starts at the right edge of the whole group (pill + icons), so it never crosses them.
              <div key={d} ref={reg(`dom:${i}`)} className="flex min-w-0 max-w-full items-center gap-0.5">
                <Pill id={`dp:${i}`} reg={reg} icon={tlsOn ? <Lock className="size-3 text-ok" /> : <Globe className="size-3" />} sub={wild ? "wildcard" : undefined} title={redirectOf(d) ? `${d}: redirects to ${redirectOf(d)?.to}` : wild ? `${d}: every subdomain of ${d.slice(2)}` : d}>
                  {d}
                </Pill>
                {domains.length > 0 && <DomainLinks domain={d} tls={tlsOn} open={!wild} />}
              </div>
            );
          })}
          {(hiddenDomains.length > 0 || moreOpen) && allDomains.length > DOMAINS_SHOWN && (
            <button
              type="button"
              ref={reg("dom:more")}
              onClick={() => setMoreOpen((o) => !o)}
              aria-expanded={moreOpen}
              title={allDomains.join("\n")}
              className="relative z-10 rounded-md border border-line bg-panel px-1.5 py-0.5 text-[11px] text-muted hover:border-accent/60 hover:text-fg focus-visible:outline-2 focus-visible:outline-accent"
            >
              {moreOpen ? "show fewer" : `+${hiddenDomains.length} more`}
            </button>
          )}
        </div>
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
      <div ref={reg("edgecol")} className="flex min-w-0 flex-col items-start gap-1 md:self-start md:pt-px">
        <Label>Edge</Label>
        {edges.map((e) => (
          <Pill key={e.server_id} id={`edge:${e.server_id}`} reg={reg} tone={edgeTone(e)} down={edgeOffline(e.server_id) ? "node offline" : undefined} icon={<Server className="size-3" />} to={{ kind: "node", id: e.server_id }} rate={edgeRate(e.server_id)}>
            {e.name ?? e.server_id.slice(0, 8)}
          </Pill>
        ))}
        {!edges.length && <span className="text-xs text-warn">No edge nodes</span>}
        {redirects.map((r, i) => (
          <div key={i} className="z-10 max-w-full" style={redirPos[i] ? { position: "absolute", top: redirPos[i].top, left: redirPos[i].left } : undefined}>
          <Pill id={`redir:${i}`} reg={reg} icon={<CornerDownRight className="size-3" />} sub={String(r.code)} title={`${r.from} → ${r.to} (${r.code})${r.keep_path ? ", keeps the path" : ""}`}>
            → {(r.to ?? "—").replace(/^https?:\/\//, "")}
          </Pill>
          </div>
        ))}
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
                <Pill id={`t:${t.key}`} reg={reg} standby={t.standby} icon={targetIcon(t.kind)} to={t.to} sub={t.ports} title={t.title}>
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
                {t.detail && (
                  <span className="w-full truncate pl-1 font-mono text-[10px] text-faint" title={t.detail}>
                    {t.detail}
                  </span>
                )}
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
  const search = useSearch({ from: "/ingress" });
  const navigate = useNavigate();
  const patch = (p: Partial<IngressSearch>) => void navigate({ from: "/ingress", to: "/ingress", search: (s) => ({ ...s, ...p }), replace: true });
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
  // Addresses → nodes, and each involved node's containers, so `host:port` upstreams resolve to what they reach.
  const byAddr = new Map<string, string>();
  for (const n of nodes.values()) for (const a of nodeAddresses(n)) if (!isLoopback(a) && !byAddr.has(a)) byAddr.set(a, n.id);
  const list0 = Array.isArray(q.data?.endpoints) ? q.data.endpoints : [];
  const involved = new Set<string>();
  for (const ep of list0) {
    for (const e of arr<{ server_id: string }>(ep.edges)) involved.add(e.server_id);
    const spec = sites.get(ep.id)?.config.spec;
    const addrs = [...arr<string>(spec?.upstreams), ...arr<string>(spec?.standby), ...arr<RouteRule>(spec?.rules).flatMap((r) => [...arr<string>(r?.upstreams), ...arr<string>(r?.standby)])];
    for (const a of addrs) {
      const id = typeof a === "string" ? byAddr.get(splitAddr(a).host) : undefined;
      if (id) involved.add(id);
    }
    for (const t of arr<RouteTarget>(ep.targets)) if (t?.target?.kind === "node") involved.add(t.target.server_id);
  }
  // Only online nodes: the list is an agent call.
  const containers = useNodeContainers([...involved].filter((id) => nodes.get(id)?.status === "online"));
  const ctx: Ctx = { nodes, apps, containers, byAddr, edges: [] };
  if (q.isLoading) return <Skeleton />;
  if (q.isError) return <ErrorNote error={q.error} />;
  const list = Array.isArray(q.data?.endpoints) ? q.data.endpoints : [];
  // Filters (kept in the URL): matched against what each row shows.
  const edgeOpts = new Map<string, string>();
  for (const ep of list) for (const e of arr<RouteEndpoint["edges"][number]>(ep.edges)) edgeOpts.set(e.server_id, e.name ?? e.server_id.slice(0, 8));
  const needle = (search.rq ?? "").trim().toLowerCase();
  const shown = list.filter((ep) => {
    const site = sites.get(ep.id);
    const edges = arr<RouteEndpoint["edges"][number]>(ep.edges);
    const rules = rulesOf({ ...ctx, edges: edges.map((e) => e.server_id) }, ep, site);
    if (search.rkind && !rules.some((r) => r.kind === search.rkind)) return false;
    if (search.redge && !edges.some((e) => e.server_id === search.redge)) return false;
    if (search.rplace && (site?.config.placement.kind ?? (ep.source === "profile" ? "cluster" : "nodes")) !== search.rplace) return false;
    const tls = !!ep.tls && ep.tls !== "off";
    if (search.rtls && (search.rtls === "on") !== tls) return false;
    if (search.rstatus && (healthOf(rules).health ?? "healthy") !== search.rstatus) return false;
    if (!needle) return true;
    const hay = [
      ep.name,
      ...arr<string>(ep.domains),
      ...arr<RedirectRule>(site?.config.spec.redirects).flatMap((r) => [r?.from, r?.to]),
      ...rules.flatMap((r) => [r.path, ...r.targets.flatMap((t) => [t.label, t.ports, t.detail, t.title, ...t.hops.flatMap((h) => [h.name, h.address])])]),
    ];
    return hay.some((x) => typeof x === "string" && x.toLowerCase().includes(needle));
  });
  const filtered = !!(needle || search.rkind || search.redge || search.rplace || search.rtls || search.rstatus);
  const pick = <K extends keyof IngressSearch>(k: K) => (e: { target: { value: string } }) => patch({ [k]: e.target.value || undefined } as Partial<IngressSearch>);
  return (
    <div className="max-w-5xl space-y-2">
      {list.length > 0 && (
        <div className="flex flex-wrap items-center gap-2 text-xs">
          <SearchInput value={search.rq ?? ""} onChange={(v) => patch({ rq: v || undefined })} placeholder="Domain, path, target, address…" />
          <Select aria-label="Rule kind" value={search.rkind ?? ""} onChange={pick("rkind")}>
            <option value="">Any kind</option>
            {RULE_KINDS.map((k) => (
              <option key={k.id} value={k.id}>
                {k.label}
              </option>
            ))}
          </Select>
          <Select aria-label="Edge node" value={search.redge ?? ""} onChange={pick("redge")}>
            <option value="">Any edge</option>
            {[...edgeOpts].map(([id, name]) => (
              <option key={id} value={id}>
                {name}
              </option>
            ))}
          </Select>
          <Select aria-label="Placement" value={search.rplace ?? ""} onChange={pick("rplace")}>
            <option value="">Any placement</option>
            <option value="nodes">Single nodes</option>
            <option value="cluster">Cluster edge group</option>
          </Select>
          <Select aria-label="TLS" value={search.rtls ?? ""} onChange={pick("rtls")}>
            <option value="">TLS on or off</option>
            <option value="on">TLS on</option>
            <option value="off">TLS off</option>
          </Select>
          <Select aria-label="Status" value={search.rstatus ?? ""} onChange={pick("rstatus")}>
            <option value="">Any status</option>
            <option value="healthy">Healthy</option>
            <option value="degraded">Degraded</option>
            <option value="down">Down</option>
          </Select>
          <span className="font-mono text-[11px] text-faint tabular-nums">
            {shown.length} of {list.length} endpoints
          </span>
          {filtered && (
            <button type="button" className="text-accent hover:underline focus-visible:outline-2 focus-visible:outline-accent" onClick={() => patch({ rq: undefined, rkind: undefined, redge: undefined, rplace: undefined, rtls: undefined, rstatus: undefined })}>
              Clear filters
            </button>
          )}
        </div>
      )}
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
          {shown.length === 0 && (
            <div className="px-3 py-6 text-center text-xs text-muted">
              No endpoint matches these filters.{" "}
              <button type="button" className="text-accent hover:underline" onClick={() => patch({ rq: undefined, rkind: undefined, redge: undefined, rplace: undefined, rtls: undefined, rstatus: undefined })}>
                Clear filters
              </button>
            </div>
          )}
          {shown.map((ep) => (
            <Row key={ep.id} ep={ep} site={sites.get(ep.id)} live={live} ctx={ctx} clusterName={clusterName} />
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
