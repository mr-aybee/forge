// Agent versions and what an update would do on each node
// (`GET /api/v1/agents/versions`).

import { useQuery } from "@tanstack/react-query";

import { get } from "./client";

/** What an agent update would do on a node, from the server's verdict. */
export type UpdateVerdict = "will_update" | "needs_reinstall" | "offline" | "ahead" | "current" | "unknown";

/** Why a node cannot self-update, or what to know before it does. */
export type UpdateIssue =
  | "not_linux"
  | "no_agent_build"
  | "no_systemd"
  | "no_curl"
  | "bin_dir_not_writable"
  | "low_disk"
  | "ephemeral_container"
  | "unknown";

export interface AgentVersionRow {
  id: string;
  hostname: string;
  display_name: string | null;
  status: string;
  os: string;
  arch: string;
  agent_version: string;
  outdated: boolean;
  ahead: boolean;
  updatable: boolean;
  verdict: UpdateVerdict;
  /** Why the node needs a reinstall (`needs_reinstall`). */
  reasons: UpdateIssue[];
  /** Notes that do not block the update, such as a container's image layer. */
  warnings: UpdateIssue[];
}

export interface AgentVersions {
  /** The version of the agents on offer. */
  target: string;
  /** The server's own version. */
  server: string;
  outdated: number;
  nodes: AgentVersionRow[];
}

export const agentUpdateKeys = {
  versions: ["agent-versions"] as const,
  autoUpdate: ["agent-auto-update"] as const,
};

const VERDICTS: readonly UpdateVerdict[] = ["will_update", "needs_reinstall", "offline", "ahead", "current", "unknown"];
const ISSUES: readonly UpdateIssue[] = ["not_linux", "no_agent_build", "no_systemd", "no_curl", "bin_dir_not_writable", "low_disk", "ephemeral_container"];

type Rec = Record<string, unknown>;
const rec = (v: unknown): Rec => (v && typeof v === "object" && !Array.isArray(v) ? (v as Rec) : {});
const str = (v: unknown): string => (typeof v === "string" ? v : "");
const issues = (v: unknown): UpdateIssue[] =>
  (Array.isArray(v) ? v : []).map((x) => (ISSUES as readonly unknown[]).includes(x) ? (x as UpdateIssue) : "unknown");

/** An older server sends no verdict: derive a safe one (`unknown` for an outdated node). */
function verdictOf(n: Rec): UpdateVerdict {
  if ((VERDICTS as readonly unknown[]).includes(n.verdict)) return n.verdict as UpdateVerdict;
  if (n.outdated !== true) return "current";
  if (n.status !== "online") return "offline";
  return n.updatable === true ? "unknown" : "needs_reinstall";
}

export function normAgentVersions(raw: unknown): AgentVersions {
  const r = rec(raw);
  const nodes = (Array.isArray(r.nodes) ? r.nodes : [])
    .map(rec)
    .filter((n) => typeof n.id === "string" && n.id)
    .map((n) => ({
      id: str(n.id),
      hostname: str(n.hostname) || str(n.id).slice(0, 8),
      display_name: typeof n.display_name === "string" && n.display_name ? n.display_name : null,
      status: str(n.status) || "unknown",
      os: str(n.os),
      arch: str(n.arch),
      agent_version: str(n.agent_version),
      outdated: n.outdated === true,
      ahead: n.ahead === true,
      updatable: n.updatable === true,
      verdict: verdictOf(n),
      reasons: issues(n.reasons),
      warnings: issues(n.warnings),
    }));
  const outdated = typeof r.outdated === "number" ? r.outdated : nodes.filter((n) => n.outdated).length;
  return { target: str(r.target), server: str(r.server), outdated, nodes };
}

/** Agent versions per node (shared with the Forge updates page). */
export const useAgentVersions = () =>
  useQuery({
    queryKey: agentUpdateKeys.versions,
    queryFn: () => get<unknown>("/api/v1/agents/versions").then(normAgentVersions),
    refetchInterval: 10_000,
  });

/** Plain words for an issue, and what fixes it. */
export const ISSUE_TEXT: Record<UpdateIssue, string> = {
  not_linux: "Self-update is for Linux nodes; reinstall this agent",
  no_agent_build: "This control plane has no agent build for this platform",
  no_systemd: "No systemd on this node; reinstall the agent to update it",
  no_curl: "curl is not installed on this node",
  bin_dir_not_writable: "/usr/local/bin is not writable (read-only root?)",
  low_disk: "Not enough free disk space next to the agent binaries",
  ephemeral_container: "Updates last until the container is recreated",
  unknown: "Reported by a newer agent",
};
