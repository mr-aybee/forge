import { RouteError } from "./RouteError";
import { createRootRoute, createRoute, createRouter, lazyRouteComponent, redirect } from "@tanstack/react-router";

import { csv, optOneOf, optPosInt, optStr, toCsv } from "../lib/search";
import { Layout } from "./Layout";

// Each page is its own chunk so the initial bundle stays small. Routes are
// declared one by one so their literal paths stay typed for <Link to>.
const rootRoute = createRootRoute({ component: Layout });
const parent = () => rootRoute;

const overview = createRoute({ getParentRoute: parent, path: "/", component: lazyRouteComponent(() => import("../pages/Overview"), "Overview") });
export const PROJECT_HEALTH = ["unhealthy", "deploying"] as const;
/** `/projects?sel=<key>&q=&cluster=&runtime=&health=`: `sel` is `project:<id>`, `env:<id>` or `service:<id>`. */
export interface ProjectsSearch {
  sel?: string;
  q?: string;
  cluster?: string;
  runtime?: string;
  health?: (typeof PROJECT_HEALTH)[number];
}
const projects = createRoute({
  getParentRoute: parent,
  path: "/projects",
  component: lazyRouteComponent(() => import("../pages/Projects"), "Projects"),
  // Explorer state lives in the URL so a selection survives navigation and can be shared.
  validateSearch: (s: Record<string, unknown>): ProjectsSearch => ({
    sel: optStr(s.sel),
    q: optStr(s.q),
    cluster: optStr(s.cluster),
    runtime: optStr(s.runtime),
    health: optOneOf(s.health, PROJECT_HEALTH),
  }),
});
// Filters live in the URL so a filtered view can be shared: `?cluster=&project=&env=&q=&view=`.
export const SERVICE_VIEWS = ["list", "env"] as const;
export interface ServicesSearch {
  cluster?: string;
  project?: string;
  env?: string;
  q?: string;
  view?: (typeof SERVICE_VIEWS)[number];
}
const services = createRoute({
  getParentRoute: parent,
  path: "/services",
  validateSearch: (s: Record<string, unknown>): ServicesSearch => ({
    cluster: optStr(s.cluster),
    project: optStr(s.project),
    env: optStr(s.env),
    q: optStr(s.q),
    view: optOneOf(s.view, SERVICE_VIEWS),
  }),
  component: lazyRouteComponent(() => import("../pages/Services"), "Services"),
});
const APP_TABS = ["overview", "configuration", "health", "storage", "releases", "webhooks"] as const;
export type AppTab = (typeof APP_TABS)[number];
const service = createRoute({
  getParentRoute: parent,
  path: "/services/$serviceId",
  validateSearch: (s: Record<string, unknown>): { tab?: AppTab } => ({ tab: APP_TABS.find((t) => t === s.tab) }),
  component: lazyRouteComponent(() => import("../pages/ServiceDetail"), "ServiceDetail"),
});
export const INGRESS_VIEWS = ["list", "cluster"] as const;
export interface IngressSearch {
  cluster?: string;
  view?: (typeof INGRESS_VIEWS)[number];
  /** Routing tab filters: text, rule kind, edge node id, placement, TLS, status. */
  rq?: string;
  rkind?: (typeof ROUTE_KINDS)[number];
  redge?: string;
  rplace?: (typeof ROUTE_PLACES)[number];
  rtls?: (typeof ROUTE_TLS)[number];
  rstatus?: (typeof ROUTE_STATUS)[number];
}
export const ROUTE_KINDS = ["proxy", "balancer", "redirect", "static"] as const;
export const ROUTE_PLACES = ["nodes", "cluster"] as const;
export const ROUTE_TLS = ["on", "off"] as const;
export const ROUTE_STATUS = ["healthy", "degraded", "down"] as const;
const ingress = createRoute({
  getParentRoute: parent,
  path: "/ingress",
  validateSearch: (s: Record<string, unknown>): IngressSearch => ({
    cluster: optStr(s.cluster),
    view: optOneOf(s.view, INGRESS_VIEWS),
    rq: optStr(s.rq),
    rkind: optOneOf(s.rkind, ROUTE_KINDS),
    redge: optStr(s.redge),
    rplace: optOneOf(s.rplace, ROUTE_PLACES),
    rtls: optOneOf(s.rtls, ROUTE_TLS),
    rstatus: optOneOf(s.rstatus, ROUTE_STATUS),
  }),
  component: lazyRouteComponent(() => import("../pages/Ingress"), "Ingress"),
});
const dns = createRoute({ getParentRoute: parent, path: "/dns", component: lazyRouteComponent(() => import("../pages/Dns"), "Dns") });
const ALERT_TABS = ["alarms", "rules", "policies", "channels", "deliveries", "silences"] as const;
export type AlertTab = (typeof ALERT_TABS)[number];
const alerts = createRoute({
  getParentRoute: parent,
  path: "/alerts",
  // `?alarm=<id>` is the link in notification messages: it opens that alarm.
  // `?silence=<id>` opens a silence (links from alarms and deliveries).
  validateSearch: (s: Record<string, unknown>): { tab?: AlertTab; alarm?: string; silence?: string } => ({
    tab: ALERT_TABS.find((t) => t === s.tab),
    alarm: typeof s.alarm === "string" && s.alarm ? s.alarm : undefined,
    silence: typeof s.silence === "string" && s.silence ? s.silence : undefined,
  }),
  component: lazyRouteComponent(() => import("../pages/Alerts"), "Alerts"),
});
// `/logs?services=<appId>,..&nodes=<appId>:<serverId>,..&levels=Error,Warning&q=<text>`: the filter
// lives in the URL so links work.
const LOG_SOURCE_VALUES = ["ingest", "container"] as const;
/**
 * `services`: service ids; `nodes`: single placements as `<serviceId>:<nodeId>`;
 * `node`: node ids (any event from them); `container`: name or id prefix;
 * `stack`; `source`: `ingest` or `container`.
 */
export interface LogsSearch {
  services?: string;
  nodes?: string;
  node?: string;
  container?: string;
  stack?: string;
  source?: (typeof LOG_SOURCE_VALUES)[number];
  levels?: string;
  q?: string;
}
const logs = createRoute({
  getParentRoute: parent,
  path: "/logs",
  validateSearch: (s: Record<string, unknown>): LogsSearch => ({
    services: optStr(s.services),
    nodes: optStr(s.nodes),
    node: optStr(s.node),
    container: optStr(s.container),
    stack: optStr(s.stack),
    source: optOneOf(s.source, LOG_SOURCE_VALUES),
    levels: optStr(s.levels),
    q: optStr(s.q),
  }),
  component: lazyRouteComponent(() => import("../pages/Logs"), "Logs"),
});
export const CLUSTER_VIEWS = ["list", "cards"] as const;
export const CLUSTER_STATES = ["all", "healthy", "degraded", "empty"] as const;
export interface ClustersSearch {
  q?: string;
  status?: (typeof CLUSTER_STATES)[number];
  view?: (typeof CLUSTER_VIEWS)[number];
}
const clusters = createRoute({
  getParentRoute: parent,
  path: "/clusters",
  validateSearch: (s: Record<string, unknown>): ClustersSearch => ({ q: optStr(s.q), status: optOneOf(s.status, CLUSTER_STATES), view: optOneOf(s.view, CLUSTER_VIEWS) }),
  component: lazyRouteComponent(() => import("../pages/Clusters"), "Clusters"),
});
export const CLUSTER_TABS = ["overview", "network", "containers", "swarm"] as const;
export type ClusterTab = (typeof CLUSTER_TABS)[number];
const cluster = createRoute({
  getParentRoute: parent,
  path: "/clusters/$clusterId",
  validateSearch: (s: Record<string, unknown>): { tab?: ClusterTab } => ({ tab: optOneOf(s.tab, CLUSTER_TABS) }),
  component: lazyRouteComponent(() => import("../pages/ClusterDetail"), "ClusterDetail"),
});
// The former per-cluster pages are filters and tabs now.
const clusterOld = createRoute({
  getParentRoute: parent,
  path: "/clusters/$clusterId/$section",
  beforeLoad: ({ params }) => {
    const id = params.clusterId;
    if (params.section === "nodes") throw redirect({ to: "/nodes", search: { cluster: id }, replace: true });
    if (params.section === "services") throw redirect({ to: "/services", search: { cluster: id }, replace: true });
    if (params.section === "ingress") throw redirect({ to: "/ingress", search: { cluster: id }, replace: true });
    const tab = params.section === "network" ? "network" : params.section === "containers" ? "containers" : undefined;
    throw redirect({ to: "/clusters/$clusterId", params: { clusterId: id }, search: { tab }, replace: true });
  },
});
const clusterOldEnv = createRoute({
  getParentRoute: parent,
  path: "/clusters/$clusterId/services/$envId",
  beforeLoad: ({ params }) => {
    throw redirect({ to: "/services", search: { cluster: params.clusterId, env: params.envId, view: "env" }, replace: true });
  },
});
export const NODE_STATES = ["active", "disconnected", "inactive", "all"] as const;
export type NodeState = (typeof NODE_STATES)[number];
export const NODE_VIEWS = ["list", "cluster"] as const;
export interface NodesSearch {
  /** Cluster id, or `none` for nodes in no cluster. */
  cluster?: string;
  role?: "control" | "worker";
  os?: string;
  /** Comma-separated tags (AND); tags never contain commas. */
  tag?: string;
  status?: NodeState;
  q?: string;
  view?: (typeof NODE_VIEWS)[number];
}
const nodes = createRoute({
  getParentRoute: parent,
  path: "/nodes",
  validateSearch: (s: Record<string, unknown>): NodesSearch => ({
    cluster: optStr(s.cluster),
    role: optOneOf(s.role, ["control", "worker"] as const),
    os: optStr(s.os),
    tag: toCsv(csv(s.tag)),
    status: optOneOf(s.status, NODE_STATES),
    q: optStr(s.q),
    view: optOneOf(s.view, NODE_VIEWS),
  }),
  component: lazyRouteComponent(() => import("../pages/Nodes"), "Nodes"),
});
const addNode = createRoute({ getParentRoute: parent, path: "/nodes/add", component: lazyRouteComponent(() => import("../pages/AddNode"), "AddNode") });
const NODE_TABS = ["overview", "processes", "services", "packages", "containers", "files", "terminal", "credentials", "details"] as const;
export const DOCKER_TABS = ["containers", "images", "volumes", "networks", "events", "disk"] as const;
export type DockerTab = (typeof DOCKER_TABS)[number];
const node = createRoute({
  getParentRoute: parent,
  path: "/nodes/$nodeId",
  validateSearch: (s: Record<string, unknown>): { tab?: (typeof NODE_TABS)[number]; root?: string; docker?: DockerTab } => ({
    tab: NODE_TABS.find((t) => t === s.tab),
    // Docker tab: which sub-tab (containers when absent).
    docker: DOCKER_TABS.find((t) => t === s.docker),
    // Files tab: open this file root (id) first.
    root: typeof s.root === "string" && s.root ? s.root : undefined,
  }),
  component: lazyRouteComponent(() => import("../pages/NodeDetail"), "NodeDetail"),
});
const CONTAINER_TABS = ["overview", "logs", "shell", "environment"] as const;
export type ContainerTab = (typeof CONTAINER_TABS)[number];
// One Docker container on a node; `containerId` is a Docker id or name.
const containerDetail = createRoute({
  getParentRoute: parent,
  path: "/nodes/$nodeId/containers/$containerId",
  validateSearch: (s: Record<string, unknown>): { tab?: ContainerTab } => ({ tab: CONTAINER_TABS.find((t) => t === s.tab) }),
  component: lazyRouteComponent(() => import("../pages/ContainerDetail"), "ContainerDetail"),
});
const terminal = createRoute({
  getParentRoute: parent,
  path: "/terminal",
  validateSearch: (s: Record<string, unknown>): { node?: string; login?: string } => ({
    node: typeof s.node === "string" ? s.node : undefined,
    login: typeof s.login === "string" ? s.login : undefined,
  }),
  component: lazyRouteComponent(() => import("../pages/Terminal"), "TerminalPage"),
});
const str = (v: unknown) => (typeof v === "string" ? v : "");
const popout = createRoute({
  getParentRoute: parent,
  path: "/popout/terminal",
  validateSearch: (s: Record<string, unknown>) => ({ key: str(s.key), node: str(s.node), login: str(s.login), host: str(s.host), role: str(s.role), container: str(s.container), cname: str(s.cname) }),
  component: lazyRouteComponent(() => import("../pages/TerminalPopout"), "TerminalPopout"),
});
const apps = createRoute({
  getParentRoute: parent,
  path: "/app-store",
  validateSearch: (s: Record<string, unknown>): { node?: string } => ({ node: typeof s.node === "string" ? s.node : undefined }),
  component: lazyRouteComponent(() => import("../pages/AppStore"), "AppStore"),
});
const packageDetail = createRoute({ getParentRoute: parent, path: "/app-store/$packageId", component: lazyRouteComponent(() => import("../pages/PackageDetail"), "PackageDetail") });
export const INSTALLED_VIEWS = ["list", "nodes"] as const;
export const INSTALLED_STATES = ["all", "installed", "busy", "failed"] as const;
export interface InstalledSearch {
  node?: string;
  status?: (typeof INSTALLED_STATES)[number];
  q?: string;
  view?: (typeof INSTALLED_VIEWS)[number];
}
const installed = createRoute({
  getParentRoute: parent,
  path: "/apps-installed",
  validateSearch: (s: Record<string, unknown>): InstalledSearch => ({
    node: optStr(s.node),
    status: optOneOf(s.status, INSTALLED_STATES),
    q: optStr(s.q),
    view: optOneOf(s.view, INSTALLED_VIEWS),
  }),
  component: lazyRouteComponent(() => import("../pages/Installed"), "Installed"),
});
export interface UpdatesSearch {
  show?: "all";
  /** Comma-separated update kinds (patch, minor, …). */
  kind?: string;
  node?: string;
  q?: string;
}
const updates = createRoute({
  getParentRoute: parent,
  path: "/apps-updates",
  validateSearch: (s: Record<string, unknown>): UpdatesSearch => ({ show: s.show === "all" ? "all" : undefined, kind: toCsv(csv(s.kind)), node: optStr(s.node), q: optStr(s.q) }),
  component: lazyRouteComponent(() => import("../pages/Updates"), "Updates"),
});
const storage = createRoute({
  getParentRoute: parent,
  path: "/storage",
  validateSearch: (s: Record<string, unknown>): { mode?: "managed" | "external"; show?: "all" } => ({
    mode: s.mode === "managed" || s.mode === "external" ? s.mode : undefined,
    show: s.show === "all" ? "all" : undefined,
  }),
  component: lazyRouteComponent(() => import("../pages/Storage"), "Storage") });
const ingressEndpoint = createRoute({ getParentRoute: parent, path: "/ingress/endpoints/$siteId", component: lazyRouteComponent(() => import("../pages/IngressEndpoint"), "IngressEndpoint") });
const storageInstall = createRoute({ getParentRoute: parent, path: "/storage/install", component: lazyRouteComponent(() => import("../pages/StorageInstall"), "StorageInstall") });
const storageSettings = createRoute({ getParentRoute: parent, path: "/storage/$storeId/settings", component: lazyRouteComponent(() => import("../pages/StorageSettings"), "StorageSettings") });
const storageDetail = createRoute({ getParentRoute: parent, path: "/storage/$storeId", component: lazyRouteComponent(() => import("../pages/StorageDetail"), "StorageDetail") });
const jobs = createRoute({ getParentRoute: parent, path: "/jobs", component: lazyRouteComponent(() => import("../pages/Jobs"), "Jobs") });
/** `/audit` filters; parsed into an `AuditFilter` by `parseAuditSearch`. */
export interface AuditSearch {
  q?: string;
  /** Comma-separated action prefixes. */
  action?: string;
  actor?: string;
  outcome?: string;
  /** A preset key (`1h`, `24h`, `7d`, `30d`) or `all`; absent with `since`/`until` for a custom window. */
  range?: string;
  since?: number;
  until?: number;
}
const audit = createRoute({
  getParentRoute: parent,
  path: "/audit",
  validateSearch: (s: Record<string, unknown>): AuditSearch => ({
    q: optStr(s.q),
    action: optStr(s.action),
    actor: optStr(s.actor),
    outcome: optStr(s.outcome),
    range: optStr(s.range),
    since: optPosInt(s.since),
    until: optPosInt(s.until),
  }),
  component: lazyRouteComponent(() => import("../pages/Audit"), "Audit"),
});
const members = createRoute({ getParentRoute: parent, path: "/members", component: lazyRouteComponent(() => import("../pages/Members"), "Members") });
const ACCESS_VIEWS = ["users", "groups", "roles"] as const;
const access = createRoute({
  getParentRoute: parent,
  path: "/access",
  validateSearch: (s: Record<string, unknown>): { view?: (typeof ACCESS_VIEWS)[number]; id?: string } => ({
    view: ACCESS_VIEWS.find((v) => v === s.view),
    id: typeof s.id === "string" && s.id ? s.id : undefined,
  }),
  component: lazyRouteComponent(() => import("../pages/Access"), "Access"),
});
// The former Access Control pages land on the dashboard.
const accessOld = createRoute({
  getParentRoute: parent,
  path: "/access/$rest",
  beforeLoad: ({ params }) => {
    throw redirect({ to: "/access", search: { view: params.rest === "groups" ? "groups" : params.rest === "roles" ? "roles" : "users" }, replace: true });
  },
});
const vault = createRoute({ getParentRoute: parent, path: "/vault", component: lazyRouteComponent(() => import("../pages/Vault"), "Vault") });
const plugins = createRoute({ getParentRoute: parent, path: "/plugins", component: lazyRouteComponent(() => import("../pages/Plugins"), "Plugins") });
const stacks = createRoute({
  getParentRoute: parent,
  path: "/stacks",
  validateSearch: (s: Record<string, unknown>): { ns?: string; cluster?: string; kind?: string; q?: string } => ({
    ns: optStr(s.ns),
    cluster: optStr(s.cluster),
    kind: optStr(s.kind),
    q: optStr(s.q),
  }),
  component: lazyRouteComponent(() => import("../pages/Stacks"), "Stacks"),
});
const newStack = createRoute({
  getParentRoute: parent,
  path: "/stacks/new",
  validateSearch: (s: Record<string, unknown>): { node?: string; environment?: string } => ({
    node: typeof s.node === "string" && s.node ? s.node : undefined,
    environment: typeof s.environment === "string" && s.environment ? s.environment : undefined,
  }),
  component: lazyRouteComponent(() => import("../pages/NewStack"), "NewStack"),
});
const newContainer = createRoute({
  getParentRoute: parent,
  path: "/stacks/new-container",
  validateSearch: (s: Record<string, unknown>): { node?: string; environment?: string } => ({
    node: typeof s.node === "string" && s.node ? s.node : undefined,
    environment: typeof s.environment === "string" && s.environment ? s.environment : undefined,
  }),
  component: lazyRouteComponent(() => import("../pages/NewContainer"), "NewContainer"),
});
const newService = createRoute({
  getParentRoute: parent,
  path: "/services/new",
  validateSearch: (s: Record<string, unknown>): { environment?: string; cluster?: string } => ({
    environment: typeof s.environment === "string" && s.environment ? s.environment : undefined,
    cluster: typeof s.cluster === "string" && s.cluster ? s.cluster : undefined,
  }),
  component: lazyRouteComponent(() => import("../pages/NewService"), "NewService"),
});
const stackDetail = createRoute({ getParentRoute: parent, path: "/stacks/$stackId", component: lazyRouteComponent(() => import("../pages/StackDetail"), "StackDetail") });
// Consent for an MCP connector's sign-in (issue #6); rendered without the shell.
const connect = createRoute({
  getParentRoute: parent,
  path: "/connect",
  validateSearch: (s: Record<string, unknown>): { request: string } => ({ request: str(s.request) }),
  component: lazyRouteComponent(() => import("../pages/Connect"), "Connect"),
});
// Approval of a device sign-in (`forge auth login --device`); without the shell.
const device = createRoute({
  getParentRoute: parent,
  path: "/device",
  validateSearch: (s: Record<string, unknown>): { code: string } => ({ code: str(s.code) }),
  component: lazyRouteComponent(() => import("../pages/Device"), "Device"),
});
// Changes AI connectors asked for in Ask mode (issue #6); `?id=` opens one.
const approvals = createRoute({
  getParentRoute: parent,
  path: "/approvals",
  validateSearch: (s: Record<string, unknown>): { id?: string } => ({ id: typeof s.id === "string" && s.id ? s.id : undefined }),
  component: lazyRouteComponent(() => import("../pages/Approvals"), "Approvals"),
});
const sftpAccess = createRoute({ getParentRoute: parent, path: "/sftp-access", component: lazyRouteComponent(() => import("../pages/SftpAccess"), "SftpAccess") });
// `/profile` (your account and preferences) opens General; the old in-page anchors land on their sections (`#remote-access` → Tunnels).
const profileSectionOf = (hash: string) => (hash === "connected-apps" ? "tokens" : hash === "remote-access" ? "access" : "general");
const profile = createRoute({
  getParentRoute: parent,
  path: "/profile",
  beforeLoad: ({ location }) => {
    const section = profileSectionOf((location.hash ?? "").replace(/^#/, ""));
    throw redirect({ to: "/profile/$section", params: { section }, replace: true });
  },
});
const profileSection = createRoute({ getParentRoute: parent, path: "/profile/$section", component: lazyRouteComponent(() => import("../pages/Settings"), "Settings") });
// Server settings (D-120). `/settings/<section>` and the old anchors were the profile: they redirect there.
const settings = createRoute({
  getParentRoute: parent,
  path: "/settings",
  beforeLoad: ({ location }) => {
    const hash = (location.hash ?? "").replace(/^#/, "");
    if (hash) throw redirect({ to: "/profile/$section", params: { section: profileSectionOf(hash) }, replace: true });
  },
  component: lazyRouteComponent(() => import("../pages/ServerSettings"), "ServerSettings"),
});
const settingsAnalytics = createRoute({
  getParentRoute: parent,
  path: "/settings/analytics",
  component: lazyRouteComponent(() => import("../pages/AnalyticsSettings"), "AnalyticsSettings"),
});
const settingsRegistries = createRoute({
  getParentRoute: parent,
  path: "/settings/registries",
  component: lazyRouteComponent(() => import("../pages/Registries"), "Registries"),
});
const settingsSection = createRoute({
  getParentRoute: parent,
  path: "/settings/$section",
  beforeLoad: ({ params }) => {
    throw redirect({ to: "/profile/$section", params: { section: params.section }, replace: true });
  },
});
const forgeUpdates = createRoute({ getParentRoute: parent, path: "/admin/updates", component: lazyRouteComponent(() => import("../pages/ForgeUpdates"), "ForgeUpdates") });
const tunnels = createRoute({ getParentRoute: parent, path: "/admin/tunnels", component: lazyRouteComponent(() => import("../pages/Tunnels"), "Tunnels") });

const routeTree = rootRoute.addChildren([
  overview,
  projects,
  services,
  newService,
  service,
  ingress,
  ingressEndpoint,
  dns,
  alerts,
  logs,
  clusters,
  cluster,
  clusterOld,
  clusterOldEnv,
  nodes,
  addNode,
  node,
  containerDetail,
  terminal,
  popout,
  apps,
  packageDetail,
  installed,
  updates,
  storage,
  storageInstall,
  storageSettings,
  storageDetail,
  jobs,
  audit,
  members,
  access,
  accessOld,
  vault,
  plugins,
  settings,
  settingsAnalytics,
  settingsRegistries,
  settingsSection,
  profile,
  profileSection,
  forgeUpdates,
  tunnels,
  approvals,
  sftpAccess,
  connect,
  device,
  stacks,
  newStack,
  newContainer,
  stackDetail,
]);

export const router = createRouter({ routeTree, defaultPreload: "intent", defaultErrorComponent: RouteError });

declare module "@tanstack/react-router" {
  interface Register {
    router: typeof router;
  }
}
