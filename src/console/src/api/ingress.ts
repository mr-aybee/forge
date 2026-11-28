// Ingress: cluster profiles, endpoints (Nginx sites) and certificates.
// Server: `src/forge/crates/apps/forge-server/src/api/sites.rs`, `ingress.rs`, and
// `forge_catalog::{ingress, sites}`. Every type the console reads from those
// endpoints is declared here once.
import { useQueries, useQuery } from "@tanstack/react-query";

import { api, get, post } from "./client";
import { useAction } from "./hooks";
import type { ServiceInstance } from "./types";

/** Mirrors `forge_catalog::ingress`. */
export type IngressMode = "standalone" | "active_standby" | "active_active";
export type Failover = "vrrp" | "dns";
export type EdgeRole = "primary" | "backup";

export interface NginxSettings {
  worker_connections: number;
  keepalive_timeout_s: number;
  proxy_read_timeout_s: number;
  client_max_body_mb: number;
  gzip: boolean;
  server_tokens: boolean;
  hsts: boolean;
  real_ip_from: string[];
  rate_limit: number;
  access_log: "combined" | "json" | "off";
  custom: string;
}

export interface IngressEdge {
  server_id: string;
  role: EdgeRole;
  /** Address clients reach this edge on (published in DNS). */
  public_ip?: string | null;
}

export interface IngressDns {
  /** PowerDNS node holding the zones. */
  server_id: string;
  ttl: number;
  auto_failover: boolean;
}

export interface IngressSpec {
  mode: IngressMode;
  edges: IngressEdge[];
  failover: Failover;
  vrrp: { vips: string[]; interface: string; router_id: number } | null;
  nginx: NginxSettings;
  dns?: IngressDns | null;
}

export interface EdgeHealth {
  healthy: boolean;
  streak: number;
  last_error: string | null;
  since: number;
}

export interface DnsPublish {
  addresses: string[];
  published_at: number;
  error?: string;
  records: {
    name: string;
    type?: string;
    zone?: string;
    status: string;
    error: string | null;
  }[];
}

export interface ClusterProfile {
  ingress: IngressSpec | null;
}

export interface Member {
  id: string;
  name: string;
  hostname: string;
  role: "control" | "worker";
  status: string;
  os: string;
  address: string;
}

export interface EdgeResult {
  status: string;
  error: string | null;
  updated_at: number;
}

export interface ProfileResponse {
  cluster?: { id: string; name: string };
  profile: ClusterProfile | null;
  status: {
    edges?: Record<string, EdgeResult>;
    dirty?: boolean;
    applied_at?: number;
    health?: Record<string, EdgeHealth>;
    dns?: DnsPublish | null;
  };
  members: Member[];
}

export interface LiveEdge {
  status: string;
  error: string | null;
  state: {
    nginx: string;
    keepalived: string;
    global_config: boolean;
    vips_held: string[];
    traffic?: Record<string, SiteTraffic> | null;
  } | null;
}

/** Last minute of traffic for one endpoint on one edge (bytes per second). */
export interface SiteTraffic {
  requests: number;
  rps: number;
  in_bps: number;
  out_bps: number;
  upstreams: Record<
    string,
    { requests: number; sent_bps: number; received_bps: number }
  >;
}

export interface LiveResponse {
  edges: Record<string, LiveEdge>;
}

export type Target =
  | { kind: "app"; service_id: string; port: number }
  | { kind: "node"; server_id: string; port: number }
  | { kind: "external"; address: string }
  /** Every node of a Swarm cluster on the routing-mesh port; `service` is a label only. */
  | { kind: "swarm"; cluster_id: string; port: number; service?: string };

export const defaultNginx = (): NginxSettings => ({
  worker_connections: 4096,
  keepalive_timeout_s: 65,
  proxy_read_timeout_s: 300,
  client_max_body_mb: 25,
  gzip: true,
  server_tokens: false,
  hsts: true,
  real_ip_from: [],
  rate_limit: 0,
  access_log: "combined",
  custom: "",
});

export const MODES: {
  id: IngressMode;
  title: string;
  body: string;
  min: number;
}[] = [
  {
    id: "standalone",
    title: "Single edge",
    body: "One public edge node serves every endpoint. Point your domains at its IP; no DNS needed.",
    min: 1,
  },
  {
    id: "active_standby",
    title: "Active · standby",
    body: "The primary serves traffic; DNS moves to a backup when the primary goes down.",
    min: 2,
  },
  {
    id: "active_active",
    title: "Active · active",
    body: "Every edge serves traffic; DNS round-robin across the healthy edges.",
    min: 2,
  },
];

/** An uploaded TLS certificate (the key never leaves the server). */
export interface Certificate {
  id: string;
  name: string;
  domains: string[];
  issuer: string;
  not_before: number;
  not_after: number;
  days_left: number;
  fingerprint: string;
  used_by: string[];
  created_at: number;
}

/** Whether a certificate's names cover `host` (exact or one-label wildcard). */
export const covers = (names: string[], host: string) => {
  const h = host.toLowerCase();
  return names.some(
    (d) =>
      d === h ||
      (d.startsWith("*.") && h.split(".").slice(1).join(".") === d.slice(2)),
  );
};

// ------------------------------------------------------------------ endpoints

/** How a route answers: `proxy` (one upstream set), `balancer` (spread with standbys), `redirect`, `static` files. */
export type RuleKind = "proxy" | "balancer" | "redirect" | "static";
export const RULE_KINDS: { id: RuleKind; label: string }[] = [
  { id: "proxy", label: "Proxy" },
  { id: "balancer", label: "Load balancer" },
  { id: "redirect", label: "Redirect" },
  { id: "static", label: "Static" },
];
export const ruleKindLabel = (k: RuleKind | undefined) =>
  RULE_KINDS.find((r) => r.id === (k ?? "proxy"))?.label ?? "Proxy";
export type MatchKind = "prefix" | "exact" | "regex";
export type Tls = "off" | "acme" | "uploaded";

/** `forge_catalog::sites::ProxyOptions`. */
export interface ProxyOptions {
  preserve_host: boolean;
  strip_prefix: boolean;
  read_timeout_s: number;
  connect_timeout_s: number;
  buffering: boolean;
}
export const defaultProxy = (): ProxyOptions => ({
  preserve_host: true,
  strip_prefix: false,
  read_timeout_s: 60,
  connect_timeout_s: 10,
  buffering: true,
});

export interface RedirectTo {
  to: string;
  code: number;
  keep_path: boolean;
}
export interface StaticRoot {
  root: string;
  index: string;
  spa: boolean;
}

/** A path rule (`spec.rules[]`). The server resolves `upstreams`/`standby` from the targets. */
export interface RouteRule {
  path: string;
  match_kind: MatchKind;
  kind: RuleKind;
  targets?: Target[];
  standby_targets?: Target[];
  upstreams: string[];
  standby: string[];
  balance: Balance;
  websockets: boolean;
  proxy?: ProxyOptions;
  redirect?: RedirectTo;
  static_root?: StaticRoot;
}

export interface CacheSpec {
  key: string;
  extensions: string[];
  ttl_minutes: number;
  ignore_headers: boolean;
  max_size_mb: number;
}
export interface GzipSpec {
  level: number;
  types: string[];
}
/** A basic-auth user. The server never returns hashes: send `{user}` to keep the password, `{user, password}` to set it. */
export interface BasicAuthUser {
  user: string;
  password?: string;
}
export interface BasicAuth {
  realm: string;
  users: BasicAuthUser[];
}
export interface Hotlink {
  referers: string[];
  extensions: string[];
}
export interface AccessSpec {
  deny: string[];
  allow: string[];
  basic_auth?: BasicAuth;
  hotlink?: Hotlink;
}
export interface RedirectRule {
  from: string;
  to: string;
  code: number;
  keep_path: boolean;
}

/** Mirrors `forge_catalog::sites::SiteSpec` (an Nginx site). The server omits optional fields at their default. */
export interface SiteSpec {
  name: string;
  domains: string[];
  http_port?: number;
  https_port?: number;
  kind?: RuleKind;
  upstreams: string[];
  standby: string[];
  balance: Balance;
  proxy?: ProxyOptions;
  redirect?: RedirectTo;
  static_root?: StaticRoot;
  rules?: RouteRule[];
  tls: Tls;
  acme_email: string;
  cert_id?: string;
  force_https: boolean;
  hsts?: boolean;
  websockets: boolean;
  max_body_mb: number;
  rate_limit: number;
  rate_burst?: number;
  cache?: CacheSpec;
  gzip?: GzipSpec;
  access?: AccessSpec;
  redirects?: RedirectRule[];
  custom: string;
}

/** Let's Encrypt certificate state on one edge (checked by Forge every 6 h). */
export interface EdgeCert {
  days_left?: number;
  expiry?: string;
  checked_at: number;
  renewed_at?: number;
  renew_error?: string | null;
  error?: string;
  /** No certificate on that edge yet. */
  missing?: boolean;
}
export interface DnsCfg {
  server_id: string;
  zone: string;
  ttl: number;
  mode: "round_robin" | "active_standby";
  addresses: string[];
}
/** Where an endpoint is served: nodes with Nginx, or a cluster's edge group. */
export type Placement =
  | { kind: "nodes"; server_ids: string[] }
  | { kind: "cluster"; cluster_id: string };
export interface SiteEdgeStatus {
  status: string;
  error: string | null;
  updated_at: number;
  cert?: EdgeCert;
}

/** A row of `GET /api/v1/sites`, normalised by `useSites` (every field below exists). */
export interface Site {
  id: string;
  name: string;
  config: {
    spec: Omit<SiteSpec, "name">;
    placement: Placement;
    /** The edges it is (or will be) applied to; from the cluster profile for cluster placement. */
    edges: string[];
    dns: DnsCfg | null;
    cluster_id?: string | null;
    targets?: Target[];
    standby_targets?: Target[];
  };
  /** Per edge server id. The server's internal `_applied_by` key is dropped. */
  edge_status: Record<string, SiteEdgeStatus>;
  updated_at: number;
}

/** Body of `POST /sites`, `PUT /sites/{id}` and `POST /sites/preview`. */
export type SiteBody = SiteSpec & {
  placement: Placement;
  targets: Target[];
  standby_targets: Target[];
  dns: DnsCfg | null;
};

/** `POST /api/v1/sites/preview`. */
export interface SitePreview {
  variant?: "http" | "https";
  config?: string;
  http: string;
  https: string | null;
  custom: string;
  custom_error?: {
    line: number;
    directive: string | null;
    message: string;
  } | null;
  edges?: string[];
}

/** `GET /api/v1/sites/{id}/logs`. */
export interface SiteLog {
  job_id?: string;
  status: "succeeded" | "failed" | "running";
  result?: { path?: string; size?: number; log?: string; missing?: boolean };
  error?: string;
}

/** Result of apply / remove / cache clear: one row per edge. */
export interface EdgeRun {
  edges: { server_id: string; result: { status: string; error?: string } }[];
  dns?: { name: string; type: string; status: string; error?: string }[] | null;
}

/** `spec.balance` of an endpoint, as the console labels it. */
export type Balance = "round_robin" | "least_conn" | "ip_hash";
export const BALANCE_LABELS: Record<Balance, string> = {
  round_robin: "Round robin",
  least_conn: "Least connections",
  ip_hash: "IP hash",
};
export const balanceLabel = (b: string | null | undefined) =>
  BALANCE_LABELS[(b ?? "round_robin") as Balance] ?? "Round robin";

const arr = <T>(v: unknown): T[] => (Array.isArray(v) ? (v as T[]) : []);
const obj = (v: unknown): Record<string, unknown> =>
  v && typeof v === "object" && !Array.isArray(v)
    ? (v as Record<string, unknown>)
    : {};

/** Normalises one site row so callers never index a missing field. */
export const normSite = (v: unknown): Site | undefined => {
  const s = obj(v);
  if (typeof s.id !== "string") return undefined;
  const cfg = obj(s.config);
  const raw = obj(cfg.spec);
  const spec = { ...raw } as unknown as Site["config"]["spec"];
  spec.domains = arr<string>(raw.domains);
  spec.upstreams = arr<string>(raw.upstreams);
  spec.standby = arr<string>(raw.standby);
  spec.balance = (raw.balance as Balance) ?? "round_robin";
  spec.tls = (raw.tls as Tls) ?? "off";
  spec.acme_email = typeof raw.acme_email === "string" ? raw.acme_email : "";
  spec.force_https = raw.force_https !== false;
  spec.websockets = raw.websockets !== false;
  spec.max_body_mb = typeof raw.max_body_mb === "number" ? raw.max_body_mb : 25;
  spec.rate_limit = typeof raw.rate_limit === "number" ? raw.rate_limit : 0;
  spec.custom = typeof raw.custom === "string" ? raw.custom : "";
  spec.rules = arr<RouteRule>(raw.rules);
  spec.redirects = arr<RedirectRule>(raw.redirects);
  const edges = arr<string>(cfg.edges);
  const clusterId = typeof cfg.cluster_id === "string" ? cfg.cluster_id : null;
  const p = obj(cfg.placement);
  // Rows saved before `placement` existed: derive it from cluster_id / edges.
  const placement: Placement =
    p.kind === "cluster" && typeof p.cluster_id === "string"
      ? { kind: "cluster", cluster_id: p.cluster_id }
      : p.kind === "nodes"
        ? { kind: "nodes", server_ids: arr<string>(p.server_ids) }
        : clusterId
          ? { kind: "cluster", cluster_id: clusterId }
          : { kind: "nodes", server_ids: edges };
  const status: Record<string, SiteEdgeStatus> = {};
  for (const [k, val] of Object.entries(obj(s.edge_status)))
    if (!k.startsWith("_") && val && typeof val === "object")
      status[k] = val as SiteEdgeStatus;
  return {
    id: s.id,
    name: typeof s.name === "string" ? s.name : s.id.slice(0, 8),
    config: {
      spec,
      placement,
      edges,
      dns: (cfg.dns as DnsCfg | null) ?? null,
      cluster_id:
        placement.kind === "cluster" ? placement.cluster_id : clusterId,
      targets: arr<Target>(cfg.targets),
      standby_targets: arr<Target>(cfg.standby_targets),
    },
    edge_status: status,
    updated_at: typeof s.updated_at === "number" ? s.updated_at : 0,
  };
};

/** The rule types an endpoint uses (default route first), for list badges. */
export const siteKinds = (
  spec: Pick<SiteSpec, "kind" | "rules">,
): RuleKind[] => [
  ...new Set<RuleKind>([
    spec.kind ?? "proxy",
    ...(spec.rules ?? []).map((r) => r.kind ?? "proxy"),
  ]),
];

const targetsApp = (list: Target[] | undefined, appId: string) =>
  (list ?? []).some((t) => t?.kind === "app" && t.service_id === appId);
/** The endpoints that route to service `appId`, as an active or a standby target. */
export const sitesForApp = (
  sites: Site[] | undefined,
  appId: string | undefined,
): Site[] =>
  appId
    ? (sites ?? []).filter(
        (s) =>
          targetsApp(s.config?.targets, appId) ||
          targetsApp(s.config?.standby_targets, appId),
      )
    : [];

/** How many replicas (active) and standbys a service's placements give an endpoint. */
export const fanOut = (instances: ServiceInstance[] | undefined) => {
  const list = Array.isArray(instances) ? instances : [];
  return {
    replicas: list.filter((i) => i.role !== "standby").length,
    standbys: list.filter((i) => i.role === "standby").length,
  };
};

/** Endpoint slug from a domain: lowercase, invalid characters to dashes, starts with a letter, max 60. */
export const slugFromDomain = (domain: string) => {
  let s = domain
    .toLowerCase()
    .replace(/^\*\./, "")
    .replace(/[^a-z0-9-]+/g, "-")
    .replace(/-+/g, "-")
    .replace(/^[^a-z]+/, "")
    .slice(0, 60)
    .replace(/-+$/, "");
  if (!s) s = "";
  return s;
};
export const NAME_RE = /^[a-z][a-z0-9-]{0,59}$/;

// ------------------------------------------------------------------ queries

const enc = encodeURIComponent;

/** The one key factory for ingress queries. `sites` and `certificates` are prefixes other pages invalidate. */
export const ingressKeys = {
  sites: ["sites"] as const,
  preview: (body: unknown) => ["sites", "preview", body] as const,
  logs: (id: string, server: string, log: "access" | "error") =>
    ["sites", id, "logs", server, log] as const,
  certificates: ["certificates"] as const,
  profile: (clusterId: string) => ["cluster-profile", clusterId] as const,
  live: (clusterId: string) => ["cluster-live", clusterId] as const,
  routing: (clusterId: string) => ["cluster-routing", clusterId] as const,
};

/** Every endpoint, normalised. A failed request surfaces as `error` (never as an empty list). */
export function useSites(enabled = true) {
  return useQuery({
    queryKey: ingressKeys.sites,
    enabled,
    queryFn: async (): Promise<Site[]> =>
      arr<unknown>(await get<unknown>("/api/v1/sites"))
        .map(normSite)
        .filter((s): s is Site => !!s),
    retry: false,
  });
}

/** Uploaded certificates (never the key). */
export const useCertificates = () =>
  useQuery({
    queryKey: ingressKeys.certificates,
    queryFn: async () =>
      arr<Certificate>(await get<unknown>("/api/v1/certificates")),
    staleTime: 30_000,
  });

/** A cluster's ingress profile, edges and members. */
export const useClusterProfile = (clusterId: string | undefined) =>
  useQuery({
    queryKey: ingressKeys.profile(clusterId ?? ""),
    queryFn: () =>
      get<ProfileResponse>(`/api/v1/clusters/${enc(clusterId ?? "")}/profile`),
    enabled: !!clusterId,
  });

/** Several clusters' profiles at once (placement pickers). Failed ones are `undefined`. */
export const useClusterProfiles = (ids: string[]) =>
  useQueries({
    queries: ids.map((id) => ({
      queryKey: ingressKeys.profile(id),
      queryFn: () =>
        get<ProfileResponse>(`/api/v1/clusters/${enc(id)}/profile`),
      staleTime: 30_000,
    })),
  });

/** The live state of a cluster's edges. */
export const useClusterLive = (
  clusterId: string,
  enabled: boolean,
  refetchInterval: number,
) =>
  useQuery({
    queryKey: ingressKeys.live(clusterId),
    queryFn: () => get<LiveResponse>(`/api/v1/clusters/${enc(clusterId)}/live`),
    enabled: !!clusterId && enabled,
    refetchInterval,
  });

/** The rendered config for an unsaved body. Validation errors arrive as `error`. */
export const useSitePreview = (body: SiteBody | undefined) =>
  useQuery({
    queryKey: ingressKeys.preview(body),
    queryFn: () => post<SitePreview>("/api/v1/sites/preview", body),
    enabled: !!body,
    retry: false,
    staleTime: 60_000,
    placeholderData: (prev) => prev,
  });

/** The tail of an endpoint's access or error log on one edge. */
export const useSiteLog = (
  id: string,
  server: string,
  log: "access" | "error",
  follow: boolean,
) =>
  useQuery({
    queryKey: ingressKeys.logs(id, server, log),
    queryFn: () =>
      get<SiteLog>(
        `/api/v1/sites/${enc(id)}/logs?server_id=${enc(server)}&log=${log}`,
      ),
    enabled: !!id && !!server,
    retry: false,
    refetchInterval: follow ? 3_000 : false,
  });

// ------------------------------------------------------------------ mutations

const SITE_KEYS = [ingressKeys.sites, ["cluster-routing"]];

export const useCreateSite = () =>
  useAction(
    (b: SiteBody) => post<unknown>("/api/v1/sites", b).then(normSite),
    SITE_KEYS,
  );
export const useUpdateSite = () =>
  useAction(
    ({ id, body }: { id: string; body: SiteBody }) =>
      api<unknown>("PUT", `/api/v1/sites/${enc(id)}`, body).then(normSite),
    SITE_KEYS,
  );
export const useDeleteSite = () =>
  useAction(
    (id: string) => api<void>("DELETE", `/api/v1/sites/${enc(id)}`),
    SITE_KEYS,
  );
/** Apply to / remove from the edges, or clear the proxy cache on every edge. */
export const useSiteRun = () =>
  useAction(
    ({
      id,
      action,
    }: {
      id: string;
      action: "apply" | "remove" | "cache/clear";
    }) => post<EdgeRun>(`/api/v1/sites/${enc(id)}/${action}`),
    SITE_KEYS,
  );
/** Renew the Let's Encrypt certificate now. */
export const useRenewSite = () =>
  useAction(
    (id: string) =>
      post<{ ok: boolean; certificates: Record<string, EdgeCert | null> }>(
        `/api/v1/sites/${enc(id)}/renew`,
      ),
    [ingressKeys.sites, ingressKeys.certificates],
  );
/** Upload a certificate (chain + key). */
export const useUploadCertificate = () =>
  useAction(
    (b: { name: string; chain_pem: string; key_pem: string }) =>
      post<Certificate>("/api/v1/certificates", b),
    [ingressKeys.certificates],
  );
