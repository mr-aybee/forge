import { useNavigate } from "@tanstack/react-router";
import { Info, Pencil, ScrollText, Server, SquareTerminal } from "lucide-react";
import { type ReactNode, useState } from "react";

import { useTagCounts } from "../api/hooks";
import { hasAllTags, nodeFacts, nodeName, nodeTags, type AgentStatus, type Cluster, type OnboardingNode, type Server as Node } from "../api/types";
import type { NodeState } from "../app/router";
import { useTerminals } from "../app/terminals";
import { one } from "../lib/search";
import { FilterBar } from "./filters/FilterBar";
import { RenameNodeDialog } from "./node/NodeDialogs";
import { OnboardingDrawer, onboardingName } from "./OnboardingDrawer";
import { NodeTable } from "./NodeTable";
import { Pager, usePaged } from "./Pager";
import { usePinAction } from "./Pins";
import { Empty, GroupedSections, Panel, type RowAction, RowMenu, type Section, Skeleton, useRowMenu } from "./ui";

/** Filters of a node list; the Nodes page keeps them in the URL, cluster detail locally. */
export interface NodeFilter {
  /** Cluster id, or `none` for nodes in no cluster. */
  cluster?: string;
  role?: "control" | "worker";
  os?: string;
  tags: string[];
  status: NodeState;
  q: string;
}

export const emptyNodeFilter = (status: NodeState = "all"): NodeFilter => ({ tags: [], status, q: "" });

const VIEWS: { id: NodeState; label: string; match: (s: AgentStatus) => boolean }[] = [
  { id: "active", label: "Active", match: (s) => s === "online" },
  { id: "disconnected", label: "Disconnected", match: (s) => s === "offline" },
  { id: "inactive", label: "Inactive", match: (s) => s === "pending" || s === "revoked" },
  { id: "all", label: "All", match: () => true },
];

const osFamily = (os: string | null | undefined) =>
  (os ?? "").toLowerCase().startsWith("win") ? "windows" : (os ?? "").toLowerCase();
const osLabel = (f: string) => (f === "windows" ? "Windows" : f === "linux" ? "Linux" : f || "unknown");

/**
 * Filterable node list shared by the Nodes page and cluster detail: status
 * segments, search, cluster / role / OS / tag pills, an optional grouped
 * "By cluster" layout, and per-row actions (terminal, rename, logs, plus `extra`).
 */
export function NodeList({
  nodes,
  clusters,
  loading,
  showCluster = true,
  title,
  headerActions,
  extra,
  empty,
  onboarding,
  filter,
  onFilterChange,
  grouped = false,
  viewToggle,
}: {
  nodes: Node[];
  clusters: Cluster[];
  loading?: boolean;
  showCluster?: boolean;
  title?: (count: number, view: string) => string;
  headerActions?: ReactNode;
  extra?: (n: Node) => RowAction[];
  empty?: ReactNode;
  /** Nodes being onboarded; shown as "installing" / "failed" rows until the node is listed. */
  onboarding?: OnboardingNode[];
  filter: NodeFilter;
  onFilterChange: (patch: Partial<NodeFilter>) => void;
  /** Sections per cluster (plus Unassigned) instead of one paged table. */
  grouped?: boolean;
  /** The page's `ViewToggle`, shown at the right of the filter bar. */
  viewToggle?: ReactNode;
}) {
  const navigate = useNavigate();
  const terminals = useTerminals();
  const { q, status: view, cluster = "", role = "", os = "", tags } = filter;
  const [renaming, setRenaming] = useState<Node>();
  const [openOnboarding, setOpenOnboarding] = useState<OnboardingNode>();
  const rm = useRowMenu();
  const pinAction = usePinAction();
  const nodeActions = (n: Node): RowAction[] => [
    {
      label: "Shell",
      icon: SquareTerminal,
      disabled: n.status === "revoked",
      hint: n.status === "revoked" ? "The node's agent is revoked" : undefined,
      onSelect: () => terminals.open(n),
    },
    { label: "Edit", icon: Pencil, onSelect: () => setRenaming(n) },
    {
      label: "Logs & jobs",
      icon: ScrollText,
      onSelect: () => void navigate({ to: "/nodes/$nodeId", params: { nodeId: n.id }, search: { tab: "details" } }),
    },
    { label: "Details", icon: Info, onSelect: () => void navigate({ to: "/nodes/$nodeId", params: { nodeId: n.id } }) },
    pinAction({ kind: "node", id: n.id, label: nodeName(n) }),
    ...(extra?.(n) ?? []),
  ];
  const setTags = (next: string[]) => onFilterChange({ tags: next });
  const addTag = (t: string) => {
    if (t && !tags.some((x) => x.toLowerCase() === t.toLowerCase())) setTags([...tags, t]);
  };
  const tagCounts = useTagCounts("server").data ?? [];

  const families = [...new Set(nodes.map((n) => osFamily(n.os)))].sort();
  const base = nodes.filter(
    (n) =>
      (!cluster ||
        (cluster === "none" ? !n.cluster_id : n.cluster_id === cluster)) &&
      (!role || n.role === role) &&
      (!os || osFamily(n.os) === os) &&
      (!tags.length || hasAllTags(nodeTags(n), tags)) &&
      `${n.display_name ?? ""} ${n.hostname} ${n.os} ${n.arch} ${nodeFacts(n).os_version ?? ""} ${nodeTags(n).join(" ")}`
        .toLowerCase()
        .includes(q.toLowerCase()),
  );
  const current = VIEWS.find((v) => v.id === view) ?? VIEWS[3]!;
  const rows = base.filter((n) => current.match(n.status));
  const count = (v: (typeof VIEWS)[number]) =>
    base.filter((n) => v.match(n.status)).length;
  const label = view === "all" ? "" : `${current.label.toLowerCase()} `;
  // Onboarding rows: installing or failed, not yet listed as a node; the
  // cluster filter and search apply (role and OS are unknown until enrolled).
  const listed = new Set(nodes.map((n) => n.id));
  const pending = (Array.isArray(onboarding) ? onboarding : []).filter(
    (o) =>
      (o.active || o.status === "failed") &&
      !(o.server_id && listed.has(o.server_id)) &&
      !role &&
      !os &&
      !tags.length &&
      (view === "all" || view === "active" || (view === "inactive" && o.status === "failed")) &&
      (!cluster || (cluster === "none" ? !o.cluster_id : o.cluster_id === cluster)) &&
      `${onboardingName(o)} ${o.host ?? ""}`.toLowerCase().includes(q.toLowerCase()),
  );
  const paged = usePaged(rows, { reset: [view, q, cluster, role, os, tags.join(",")].join("|") });
  const pendingNote = pending.length ? ` · ${pending.length} joining` : "";

  const table = (list: Node[], withPending: boolean) => (
    <NodeTable
      nodes={list}
      clusters={clusters}
      showCluster={showCluster && !grouped}
      onboarding={withPending ? pending : []}
      onOpenOnboarding={setOpenOnboarding}
      onTag={addTag}
      actions={(n) => <RowMenu actions={nodeActions(n)} label={`Actions for ${nodeName(n)}`} />}
      rowProps={(n) => rm.rowProps(nodeActions(n), `Actions for ${nodeName(n)}`)}
    />
  );
  const sections: Section<Node>[] = grouped
    ? [
        ...clusters.map((c) => ({ id: c.id, title: c.name, items: rows.filter((n) => n.cluster_id === c.id) })),
        { id: "", title: "Unassigned", items: rows.filter((n) => !n.cluster_id || !clusters.some((c) => c.id === n.cluster_id)) },
      ].map((sec) => ({ ...sec, meta: `${sec.items.filter((n) => n.status === "online").length}/${sec.items.length} online` }))
    : [];

  return (
    <>
      <FilterBar
        status={{ label: "Node state", value: view, options: VIEWS.map((v) => ({ id: v.id, label: v.label, count: count(v) })), onChange: (status) => onFilterChange({ status }) }}
        search={{ value: q, onChange: (v) => onFilterChange({ q: v }), placeholder: "Search name, OS, arch or tag" }}
        filters={[
          {
            id: "cluster",
            label: "Cluster",
            hidden: !showCluster,
            value: one(cluster),
            onChange: (v) => onFilterChange({ cluster: v[0] }),
            options: [{ value: "none", label: "No cluster" }, ...clusters.map((c) => ({ value: c.id, label: c.name, count: nodes.filter((n) => n.cluster_id === c.id).length }))],
          },
          {
            id: "role",
            label: "Role",
            value: one(role),
            onChange: (v) => onFilterChange({ role: v[0] === "control" || v[0] === "worker" ? v[0] : undefined }),
            options: [
              { value: "control", label: "Control plane" },
              { value: "worker", label: "Worker" },
            ],
          },
          {
            id: "os",
            label: "OS",
            value: one(os),
            onChange: (v) => onFilterChange({ os: v[0] }),
            options: families.map((f) => ({ value: f, label: osLabel(f), count: nodes.filter((n) => osFamily(n.os) === f).length })),
          },
          {
            id: "tag",
            label: "Tags",
            multi: true,
            value: tags,
            onChange: setTags,
            disabled: !tagCounts.length && !tags.length,
            title: tagCounts.length ? undefined : "No node has tags yet",
            options: [...tags.filter((t) => !tagCounts.some((c) => c.tag.toLowerCase() === t.toLowerCase())).map((t) => ({ value: t, label: t })), ...tagCounts.map((c) => ({ value: c.tag, label: c.tag, count: c.count }))],
          },
        ]}
        onClear={() => onFilterChange({ q: "", cluster: undefined, role: undefined, os: undefined, tags: [] })}
        view={viewToggle}
      />

      <Panel
        title={
          (title
            ? title(rows.length, label)
            : `${rows.length} ${label}node${rows.length === 1 ? "" : "s"}`) + pendingNote
        }
        actions={headerActions}
      >
        {loading ? (
          <Skeleton />
        ) : rows.length === 0 && pending.length === 0 ? (
          (empty && !nodes.length) || (
            <Empty
              icon={<Server className="size-5" />}
              title={`No ${label}nodes match`}
            >
              Change the filters above.
            </Empty>
          )
        ) : (
          <div className="fade-in">
            {grouped ? (
              <GroupedSections sections={sections} label="Nodes by cluster">
                {(list) => table(list, false)}
              </GroupedSections>
            ) : (
              <>
                {table(paged.rows, paged.page === 0)}
                <Pager paged={paged} />
              </>
            )}
            {grouped && pending.length > 0 && table([], true)}
          </div>
        )}
      </Panel>
      {rm.menu}
      <OnboardingDrawer
        entry={
          openOnboarding &&
          ((Array.isArray(onboarding) ? onboarding : []).find(
            (o) => (o.job_id ?? o.token_id) === (openOnboarding.job_id ?? openOnboarding.token_id),
          ) ??
            openOnboarding)
        }
        clusters={clusters}
        onOpenChange={(o) => !o && setOpenOnboarding(undefined)}
      />
      {renaming && (
        <RenameNodeDialog
          node={renaming}
          open
          onOpenChange={(o) => !o && setRenaming(undefined)}
        />
      )}
    </>
  );
}
