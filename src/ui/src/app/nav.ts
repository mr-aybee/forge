import {
  Activity,
  AppWindow,
  BellRing,
  Cable,
  CircleArrowUp,
  Boxes,
  ClipboardCheck,
  Container,
  FolderLock,
  KeyRound,
  Logs,
  ShieldCheck,
  LayoutDashboard,
  type LucideIcon,
  Network,
  PackageCheck,
  PlugZap,
  ScrollText,
  Server,
  ServerCog,
  Settings,
  Siren,
  SquareTerminal,
  Store,
  UserCog,
  UserRound,
  Users,
  Waypoints,
  Workflow,
} from "lucide-react";

import type { Capability, ConsoleCapabilities } from "../api/access";
import type { NavBadgeKey } from "../api/navBadges";

export type NavTag = "new" | "beta";

export interface NavItem {
  to: string;
  label: string;
  icon: LucideIcon;
  /** Planned (Phase 3+) areas are shown, clearly labelled. */
  planned?: boolean;
  /**
   * The console capability (`GET /api/v1/me/capabilities`) the page needs.
   * Without it the item is hidden, and so is a group left without items;
   * the route shows "no access" instead of the page. No `cap`: always shown.
   */
  cap?: Capability | Capability[];
  /** The live counter shown next to the item (`useNavBadges`). */
  badge?: NavBadgeKey;
  /** A small static label, e.g. `new` for a recently added page. */
  tag?: NavTag;
}

/** A collapsible group. Children are items or (one level deeper) sub-groups: group → sub-group → item. */
export interface NavGroup {
  /** Stable id, used to remember the expanded state. */
  id: string;
  label: string;
  icon: LucideIcon;
  children: NavEntry[];
}

export type NavEntry = NavItem | NavGroup;

/** A headed block of top-level entries (WORKLOADS, OPERATIONS, ...). No heading: shown first, unlabelled. */
export interface NavSection {
  id: string;
  heading?: string;
  entries: NavEntry[];
}

export const isGroup = (e: NavEntry): e is NavGroup => "children" in e;

/** Access Control pages: open to user admins and to access admins. */
const ACCESS_ADMIN: Capability[] = ["users_manage", "access_manage"];

/**
 * Fleet navigation: headed sections of links and collapsible groups. Nest at
 * most two group levels. A group is only used for two or more pages; after
 * capability filtering a group left with one page renders as that page.
 * Every route stays reachable; routes are never renamed here.
 */
export const NAV_SECTIONS: NavSection[] = [
  { id: "home", entries: [{ to: "/", label: "Overview", icon: LayoutDashboard }] },
  {
    id: "workloads",
    heading: "Workloads",
    entries: [
      { to: "/projects", label: "Projects", icon: AppWindow, cap: "fleet_view", badge: "projects" },
    ],
  },
  {
    id: "infrastructure",
    heading: "Infrastructure",
    entries: [
      { to: "/clusters", label: "Clusters", icon: ServerCog, cap: "fleet_view" },
      { to: "/nodes", label: "Nodes", icon: Server, cap: "fleet_view", badge: "nodes" },
      {
        id: "network",
        label: "Network",
        icon: Network,
        children: [
          { to: "/dns", label: "DNS", icon: Network, cap: "fleet_view" },
          { to: "/ingress", label: "Ingress", icon: Waypoints, cap: "fleet_view" },
          { to: "/terminal", label: "Terminal", icon: SquareTerminal, cap: "terminal" },
        ],
      },
    ],
  },
  {
    id: "operations",
    heading: "Operations",
    entries: [
      { to: "/alerts", label: "Alerts", icon: Siren, cap: "alerts_view", badge: "alerts" },
      { to: "/logs", label: "Logs", icon: Logs, cap: "fleet_view", tag: "new" },
      { to: "/jobs", label: "Jobs", icon: Workflow, cap: "fleet_view", badge: "jobs" },
      { to: "/audit", label: "Audit log", icon: ScrollText, cap: "audit_view" },
    ],
  },
  {
    id: "packages",
    heading: "Packages",
    entries: [
      { to: "/app-store", label: "App Store", icon: Store, cap: "package_install" },
      { to: "/apps-installed", label: "Installed", icon: PackageCheck, cap: "fleet_view" },
      { to: "/apps-updates", label: "Updates", icon: BellRing, cap: "software_updates", badge: "updates" },
    ],
  },
  {
    id: "admin",
    heading: "Admin",
    entries: [
      { to: "/approvals", label: "Approvals", icon: ClipboardCheck, badge: "approvals" },
      {
        id: "admin.access",
        label: "Access",
        icon: UserCog,
        children: [
          { to: "/sftp-access", label: "SFTP access", icon: FolderLock, cap: "sftp_access" },
          { to: "/admin/tunnels", label: "Tunnels", icon: Cable, cap: "tunnels" },
        ],
      },
      {
        id: "admin.accessControl",
        label: "Access Control",
        icon: ShieldCheck,
        children: [
          { to: "/access", label: "Access", icon: ShieldCheck, cap: ACCESS_ADMIN },
          { to: "/members", label: "Members", icon: Users, cap: "users_manage" },
        ],
      },
      { to: "/vault", label: "Credential vault", icon: KeyRound, cap: "vault" },
      { to: "/admin/updates", label: "Forge updates", icon: CircleArrowUp, cap: "forge_updates", badge: "forge" },
      { to: "/plugins", label: "Plugins", icon: PlugZap, cap: "plugins" },
      { to: "/profile", label: "Profile", icon: UserRound },
      { to: "/settings", label: "Settings", icon: Settings, cap: "platform_settings" },
    ],
  },
];

/** Every top-level entry, in order (helpers that do not care about headings). */
export const NAV: NavEntry[] = NAV_SECTIONS.flatMap((s) => s.entries);

/** Whether `item` is allowed; while `caps` is loading only ungated items are. */
export const allowed = (item: NavItem, caps: ConsoleCapabilities | undefined): boolean => capAllowed(item.cap, caps);

/** Whether `caps` satisfy a gate: any of the listed capabilities. */
export const capAllowed = (cap: Capability | Capability[] | undefined, caps: ConsoleCapabilities | undefined): boolean =>
  !cap || (Array.isArray(cap) ? cap : [cap]).some((c) => caps?.[c] === true);

/** Applies item gates, drops groups left without children and turns a group with one child into that child. */
export function visibleNav(caps: ConsoleCapabilities | undefined, entries: NavEntry[] = NAV): NavEntry[] {
  const out: NavEntry[] = [];
  for (const e of entries) {
    if (isGroup(e)) {
      const children = visibleNav(caps, e.children);
      if (children.length === 1 && children[0]) out.push(children[0]);
      else if (children.length) out.push({ ...e, children });
    } else if (allowed(e, caps)) out.push(e);
  }
  return out;
}

/** The visible sections; a section without visible entries is dropped. */
export const visibleSections = (caps: ConsoleCapabilities | undefined): NavSection[] =>
  NAV_SECTIONS.map((s) => ({ ...s, entries: visibleNav(caps, s.entries) })).filter((s) => s.entries.length > 0);

/** The gate of the page at `path`: the gated item whose route is `path` or a parent of it (longest wins). */
export function gateOf(path: string): Capability | Capability[] | undefined {
  let best: NavItem | undefined;
  for (const i of ALL_ITEMS) {
    if (!i.cap || i.to === "/") continue;
    if ((path === i.to || path.startsWith(`${i.to}/`)) && (!best || i.to.length > best.to.length)) best = i;
  }
  return best?.cap;
}

/** Every item with the labels of the groups it sits in (for the palette and the header). */
export function flatItems(entries: NavEntry[] = NAV, trail: string[] = []): (NavItem & { trail: string[] })[] {
  return entries.flatMap((e) => (isGroup(e) ? flatItems(e.children, [...trail, e.label]) : [{ ...e, trail }]));
}

/** Ids of the groups that contain `to` (outermost first). */
export function ancestorsOf(match: (to: string) => boolean, entries: NavEntry[] = NAV): string[] {
  for (const e of entries) {
    if (!isGroup(e)) continue;
    if (e.children.some((c) => !isGroup(c) && match(c.to))) return [e.id];
    const inner = ancestorsOf(match, e.children);
    if (inner.length) return [e.id, ...inner];
  }
  return [];
}

/**
 * Pages reached from other pages (the project explorer, breadcrumbs), not
 * from the sidebar. Listed so they keep their capability gate and header title.
 */
const UNLISTED_ITEMS: NavItem[] = [
  { to: "/services", label: "Services", icon: Boxes, cap: "fleet_view" },
  { to: "/stacks", label: "Stacks", icon: Container, cap: "fleet_view" },
];

export const ALL_ITEMS: NavItem[] = [...flatItems(), ...UNLISTED_ITEMS];
export const ActivityIcon = Activity;
