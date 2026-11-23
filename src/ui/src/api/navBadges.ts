import { useMyCapabilities } from "./access";
import { useAlarms } from "./alerts";
import { usePendingApprovalCount } from "./approvals";
import { useForgeUpdate, useJobs, usePackageUpdates, useProjects, useServers } from "./hooks";

/** The live counters the sidebar can show; nav items name one in `badge`. */
export type NavBadgeKey = "nodes" | "alerts" | "updates" | "forge" | "approvals" | "jobs" | "projects";

export type NavTone = "ok" | "warn" | "bad" | "accent" | "muted";

/** One sidebar counter. Absent when the count is zero or not loaded. */
export interface NavBadge {
  /** The number shown, e.g. online nodes or firing alerts. */
  n: number;
  /** Shown as `n/total` when present. */
  total?: number;
  tone: NavTone;
  /** A health bullet next to the counter. */
  dot?: "ok" | "warn" | "bad";
  /** Pulsing bullet: something is in progress. */
  live?: boolean;
  /** Tooltip, also the collapsed-mode label suffix. */
  title: string;
}

export type NavBadges = Partial<Record<NavBadgeKey, NavBadge>>;

const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

/**
 * Sidebar counters built on the existing queries (same keys, same polling),
 * fetched only for the capabilities the user has. Always a complete object;
 * a failed or missing endpoint leaves its badge out.
 */
export function useNavBadges(): NavBadges {
  const caps = useMyCapabilities().data;
  const fleet = caps?.fleet_view === true;
  const servers = useServers(fleet).data;
  const jobs = useJobs(fleet).data;
  const projects = useProjects(fleet).data;
  const alarms = useAlarms(true, caps?.alerts_view === true).data;
  const updates = usePackageUpdates(caps?.software_updates === true).data;
  const forge = useForgeUpdate(caps?.forge_updates === true).data;
  const approvals = usePendingApprovalCount().data ?? 0;

  const out: NavBadges = {};

  if (Array.isArray(servers)) {
    const live = servers.filter((s) => s && s.status !== "revoked");
    const online = live.filter((s) => s.status === "online").length;
    const offline = live.filter((s) => s.status === "offline").length;
    if (live.length)
      out.nodes = {
        n: online,
        total: live.length,
        tone: "muted",
        dot: offline ? "bad" : online === live.length ? "ok" : "warn",
        title: `${online} of ${plural(live.length, "node")} online${offline ? ` · ${offline} offline` : ""}`,
      };
  }
  if (Array.isArray(alarms)) {
    const firing = alarms.filter((a) => a?.state === "firing").length;
    if (firing) out.alerts = { n: firing, tone: "bad", title: `${plural(firing, "alert")} firing` };
  }
  if (Array.isArray(updates)) {
    const n = updates.filter((r) => r?.kind).length;
    if (n) out.updates = { n, tone: "warn", title: `${plural(n, "update")} available` };
  }
  if (forge?.available && forge.latest) out.forge = { n: 1, tone: "accent", title: `Forge ${forge.latest.version} is available` };
  if (approvals > 0) out.approvals = { n: approvals, tone: "accent", title: `${approvals} waiting for you` };
  if (Array.isArray(jobs)) {
    const running = jobs.filter((j) => j?.status === "running" || j?.status === "queued").length;
    if (running) out.jobs = { n: running, tone: "accent", live: true, title: `${plural(running, "job")} running or queued` };
  }
  if (Array.isArray(projects) && projects.length) out.projects = { n: projects.length, tone: "muted", title: plural(projects.length, "project") };
  return out;
}
