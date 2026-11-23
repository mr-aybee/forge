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
  records: { name: string; type?: string; zone?: string; status: string; error: string | null }[];
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
  status: { edges?: Record<string, EdgeResult>; dirty?: boolean; applied_at?: number; health?: Record<string, EdgeHealth>; dns?: DnsPublish | null };
  members: Member[];
}

export interface LiveEdge {
  status: string;
  error: string | null;
  state: { nginx: string; keepalived: string; global_config: boolean; vips_held: string[]; traffic?: Record<string, SiteTraffic> | null } | null;
}

/** Last minute of traffic for one endpoint on one edge (bytes per second). */
export interface SiteTraffic {
  requests: number;
  rps: number;
  in_bps: number;
  out_bps: number;
  upstreams: Record<string, { requests: number; sent_bps: number; received_bps: number }>;
}

export interface LiveResponse {
  edges: Record<string, LiveEdge>;
}

/** `spec.balance` of an endpoint, as the console labels it. */
export type Balance = "round_robin" | "least_conn" | "ip_hash";
export const BALANCE_LABELS: Record<Balance, string> = { round_robin: "Round robin", least_conn: "Least connections", ip_hash: "IP hash" };
export const balanceLabel = (b: string | null | undefined) => BALANCE_LABELS[(b ?? "round_robin") as Balance] ?? "Round robin";

/** Mirrors `forge_catalog::ingress::SiteSpec` (an Nginx site). */
export interface SiteSpec {
  name: string;
  domains: string[];
  upstreams: string[];
  standby: string[];
  balance: Balance;
  tls: "off" | "acme" | "uploaded";
  acme_email: string;
  cert_id?: string;
  force_https: boolean;
  websockets: boolean;
  max_body_mb: number;
  rate_limit: number;
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
}
export interface DnsCfg {
  server_id: string;
  zone: string;
  ttl: number;
  mode: "round_robin" | "active_standby";
  addresses: string[];
}
/** A row of `GET /api/v1/sites`. Every nested field may be missing on older servers; read with `?.`. */
export interface Site {
  id: string;
  name: string;
  config: {
    spec: Omit<SiteSpec, "name">;
    edges: string[];
    dns: DnsCfg | null;
    cluster_id?: string | null;
    targets?: Target[];
    standby_targets?: Target[];
  };
  edge_status: Record<string, { status: string; error: string | null; updated_at: number; cert?: EdgeCert }>;
  updated_at: number;
}

export type Target = { kind: "app"; service_id: string; port: number } | { kind: "node"; server_id: string; port: number } | { kind: "external"; address: string };

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

export const MODES: { id: IngressMode; title: string; body: string; min: number }[] = [
  { id: "standalone", title: "Standalone", body: "One public edge node serves every endpoint. Point your domains at its IP; no DNS needed.", min: 1 },
  { id: "active_standby", title: "Active · standby", body: "The primary serves traffic; DNS moves to a backup when the primary goes down.", min: 2 },
  { id: "active_active", title: "Active · active", body: "Every edge serves traffic; DNS round-robin across the healthy edges.", min: 2 },
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
  return names.some((d) => d === h || (d.startsWith("*.") && h.split(".").slice(1).join(".") === d.slice(2)));
};
