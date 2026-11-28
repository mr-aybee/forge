// The endpoint editor's form state: one object, built from a saved site (or
// blank) and turned back into the request body. Form-only shapes (list keys,
// "is this basic-auth user already saved") live here; server types come from
// `api/ingress`.
import {
  useInstallations,
  useServers,
  useServices,
  useClusters,
} from "../../api/hooks";
import {
  type BasicAuthUser,
  type DnsCfg,
  type Member,
  type Placement,
  type RedirectRule,
  type RouteRule,
  type RuleKind,
  type Site,
  type SiteBody,
  type SiteSpec,
  type Target,
  defaultProxy,
  useClusterProfile,
  useClusterProfiles,
} from "../../api/ingress";

let seq = 0;
/** A key for a new row of an editable list. */
export const newKey = () => `k${++seq}`;
type Keyed<T> = T & { _k: string };

export type FormRule = Keyed<RouteRule>;
export type FormRedirect = Keyed<RedirectRule>;
/** `saved`: the server already has a password for this user (blank keeps it). */
export type FormUser = Keyed<
  BasicAuthUser & { saved: boolean; password: string }
>;

export interface EndpointForm {
  /** The spec without its keyed lists (`rules`, `redirects`, basic-auth users). */
  spec: SiteSpec;
  targets: Target[];
  standby_targets: Target[];
  rules: FormRule[];
  redirects: FormRedirect[];
  users: FormUser[];
  placement: Placement;
  dns: DnsCfg | null;
}

export const blankSpec = (): SiteSpec => ({
  name: "",
  domains: [],
  kind: "proxy",
  upstreams: [],
  standby: [],
  balance: "round_robin",
  tls: "off",
  acme_email: "",
  force_https: true,
  hsts: true,
  websockets: true,
  max_body_mb: 25,
  rate_limit: 0,
  custom: "",
});

export const blankDns = (): DnsCfg => ({
  server_id: "",
  zone: "",
  ttl: 300,
  mode: "round_robin",
  addresses: [],
});

/** Typed targets of a site; older rows with plain `host:port` upstreams become address targets. */
const targetsOf = (typed: Target[] | undefined, plain: string[]): Target[] =>
  typed?.length
    ? typed
    : plain.map((address) => ({ kind: "external", address }));

export function formFromSite(site: Site): EndpointForm {
  const raw = site.config.spec;
  const targets = targetsOf(site.config.targets, raw.upstreams);
  const standby = targetsOf(site.config.standby_targets, raw.standby);
  // A proxy with standbys was a load balancer before rule types existed.
  const kind: RuleKind =
    (raw.kind ?? "proxy") === "proxy" && standby.length
      ? "balancer"
      : (raw.kind ?? "proxy");
  const { rules, redirects, ...rest } = raw;
  return {
    spec: {
      ...blankSpec(),
      ...rest,
      name: site.name,
      kind,
      hsts: raw.hsts ?? true,
      access: raw.access
        ? {
            ...raw.access,
            basic_auth: raw.access.basic_auth
              ? { ...raw.access.basic_auth, users: [] }
              : undefined,
          }
        : undefined,
    },
    targets,
    standby_targets: standby,
    rules: (rules ?? []).map((r) => ({ ...r, _k: newKey() })),
    redirects: (redirects ?? []).map((r) => ({ ...r, _k: newKey() })),
    users: (raw.access?.basic_auth?.users ?? []).map((u) => ({
      user: u.user,
      password: "",
      saved: true,
      _k: newKey(),
    })),
    placement: site.config.placement,
    dns: site.config.dns,
  };
}

export const isProxyKind = (k: RuleKind | undefined) =>
  (k ?? "proxy") === "proxy" || k === "balancer";

/** The rule as the server takes it: only the fields its type uses. */
function ruleBody({ _k, ...r }: FormRule): RouteRule {
  const kind = r.kind ?? "proxy";
  return {
    path: r.path.trim(),
    match_kind: r.match_kind,
    kind,
    targets: isProxyKind(kind) ? (r.targets ?? []) : [],
    standby_targets: kind === "balancer" ? (r.standby_targets ?? []) : [],
    upstreams: [],
    standby: [],
    balance: kind === "balancer" ? r.balance : "round_robin",
    websockets: r.websockets,
    ...(isProxyKind(kind) && r.proxy ? { proxy: r.proxy } : {}),
    ...(kind === "redirect"
      ? { redirect: r.redirect ?? { to: "", code: 301, keep_path: true } }
      : {}),
    ...(kind === "static"
      ? {
          static_root: r.static_root ?? {
            root: "",
            index: "index.html",
            spa: false,
          },
        }
      : {}),
  };
}

/** The `POST`/`PUT`/preview body for a form. */
export function toBody(f: EndpointForm): SiteBody {
  const s = f.spec;
  const kind = s.kind ?? "proxy";
  const proxy = isProxyKind(kind);
  const spec: SiteSpec = {
    ...s,
    name: s.name.trim(),
    domains: s.domains.map((d) => d.trim()).filter(Boolean),
    kind,
    upstreams: [],
    standby: [],
    balance: kind === "balancer" ? s.balance : "round_robin",
    proxy: proxy ? (s.proxy ?? defaultProxy()) : undefined,
    redirect:
      kind === "redirect"
        ? (s.redirect ?? { to: "", code: 301, keep_path: true })
        : undefined,
    static_root:
      kind === "static"
        ? (s.static_root ?? { root: "", index: "index.html", spa: false })
        : undefined,
    rules: f.rules.map(ruleBody),
    redirects: f.redirects.map(({ _k, ...r }) => ({
      ...r,
      from: r.from.trim(),
      to: r.to.trim(),
    })),
    cert_id: s.tls === "uploaded" ? s.cert_id : undefined,
    access: s.access
      ? {
          deny: s.access.deny,
          allow: s.access.allow,
          hotlink: s.access.hotlink,
          basic_auth: s.access.basic_auth
            ? {
                realm: s.access.basic_auth.realm,
                users: f.users.map((u) =>
                  u.password
                    ? { user: u.user.trim(), password: u.password }
                    : { user: u.user.trim() },
                ),
              }
            : undefined,
        }
      : undefined,
  };
  return {
    ...spec,
    placement: f.placement,
    targets: proxy ? f.targets : [],
    standby_targets: kind === "balancer" ? f.standby_targets : [],
    dns: f.dns,
  };
}

/** Problems that stop a save, in plain words (the server checks again). */
export function formProblems(f: EndpointForm): string[] {
  const out: string[] = [];
  const kind = f.spec.kind ?? "proxy";
  if (!f.spec.domains.some((d) => d.trim()))
    out.push("Add at least one domain.");
  if (f.spec.tls === "acme" && f.spec.domains.some((d) => d.includes("*")))
    out.push(
      "Let's Encrypt cannot issue wildcard domains here; use an uploaded certificate.",
    );
  if (f.spec.tls === "acme" && !f.spec.acme_email.trim())
    out.push("Let's Encrypt needs a contact email.");
  if (f.spec.tls === "uploaded" && !f.spec.cert_id)
    out.push("Choose or upload a certificate.");
  if (isProxyKind(kind) && !f.targets.length)
    out.push("The default route needs at least one target.");
  if (kind === "redirect" && !f.spec.redirect?.to.trim())
    out.push("The default redirect needs a target URL.");
  if (kind === "static" && !f.spec.static_root?.root.trim())
    out.push("The default static route needs a directory.");
  if (f.rules.some((r) => !r.path.trim()))
    out.push("Every path rule needs a path.");
  if (f.users.some((u) => !u.saved && !u.password))
    out.push("New basic-auth users need a password.");
  if (f.users.some((u) => !u.user.trim()))
    out.push("Every basic-auth user needs a name.");
  if (f.placement.kind === "nodes" && !f.placement.server_ids.length)
    out.push("Choose the node(s) with Nginx that serve this endpoint.");
  if (f.placement.kind === "cluster" && !f.placement.cluster_id)
    out.push("Choose the cluster whose edge group serves this endpoint.");
  return out;
}

export const lines = (s: string) =>
  s
    .split(/[\n,]/)
    .map((x) => x.trim())
    .filter(Boolean);

// ------------------------------------------------------------------ placement context

/** Nodes with Nginx installed, clusters with an ingress edge group, and the targets' context for a placement. */
export function usePlacementOptions() {
  const servers = useServers();
  const installs = useInstallations();
  const clusters = useClusters();
  const list = clusters.data ?? [];
  const profiles = useClusterProfiles(list.map((c) => c.id));
  const nodes = (servers.data ?? []).filter((n) => n.status !== "revoked");
  const nginx = nodes.filter((n) =>
    (installs.data ?? []).some(
      (i) =>
        i.server_id === n.id &&
        i.package === "nginx" &&
        i.status === "installed",
    ),
  );
  const edgeGroups = list.flatMap((c, i) => {
    const edges = profiles[i]?.data?.profile?.ingress?.edges ?? [];
    return edges.length ? [{ cluster: c, edges: edges.length }] : [];
  });
  return {
    loading:
      servers.isLoading ||
      installs.isLoading ||
      clusters.isLoading ||
      profiles.some((p) => p.isLoading),
    error: servers.error ?? installs.error ?? clusters.error,
    nodes,
    nginx,
    edgeGroups,
    clusters: list,
  };
}

/** Members (for node targets) and services (for service targets) of where the endpoint lives. */
export function useTargetContext(placement: Placement) {
  const clusterId =
    placement.kind === "cluster" ? placement.cluster_id : undefined;
  const profile = useClusterProfile(clusterId);
  const servers = useServers().data ?? [];
  const apps = useServices(undefined, clusterId ?? null).data ?? [];
  const members: Member[] = clusterId
    ? (profile.data?.members ?? [])
    : servers
        .filter((n) => n.status !== "revoked")
        .map((n) => ({
          id: n.id,
          name: n.display_name || n.hostname,
          hostname: n.hostname,
          role: "worker",
          status: n.status,
          os: n.os,
          address: n.hostname,
        }));
  return { members, apps };
}

/** Proxy options for a new route of type `k` (none for redirects and static files). */
export const defaultProxyFor = (k: RuleKind | undefined) =>
  isProxyKind(k) ? defaultProxy() : undefined;
