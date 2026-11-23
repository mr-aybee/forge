// Scope helpers for access control: one index of every resource a role can
// be assigned at (names and parents), a label/link for a scope, and a tree
// picker: Workspace; Projects → Environments → Services; Clusters → Nodes.

import { Link } from "@tanstack/react-router";
import { ChevronDown, ChevronRight } from "lucide-react";
import { type ReactNode, useMemo, useState } from "react";

import { scopeKey, type ScopeRef } from "../../api/accessControl";
import { useAllEnvironments, useProjects, useServices, useClusters, useServers, useWorkspace } from "../../api/hooks";
import { SearchInput, Skeleton } from "../ui";

export const KIND_LABEL: Record<string, string> = {
  workspace: "Workspace",
  project: "Project",
  environment: "Environment",
  service: "Service",
  cluster: "Cluster",
  server: "Node",
};
export const kindLabel = (k: string) => KIND_LABEL[k] ?? k;

/** One hue per resource level, from the level tokens in styles.css. */
const LEVEL_CLASS: Record<string, string> = {
  workspace: "bg-level-workspace-soft text-level-workspace",
  project: "bg-level-app-soft text-level-app",
  environment: "bg-level-env-soft text-level-env",
  service: "bg-level-service-soft text-level-service",
  cluster: "bg-level-cluster-soft text-level-cluster",
  server: "bg-level-node-soft text-level-node",
};

/** The level of a resource ("Environment", "Node") as a coloured chip. */
export function LevelChip({ kind }: { kind: string }) {
  return <span className={`inline-flex h-4 shrink-0 items-center rounded px-1.5 text-[10px] font-semibold tracking-wide uppercase ${LEVEL_CLASS[kind] ?? "bg-panel-2 text-muted"}`}>{kindLabel(kind)}</span>;
}

export interface ScopeNode {
  key: string;
  kind: string;
  id: string;
  name: string;
  /** Key of the parent node, if any. */
  parent: string | null;
  /** Extra id for links (an environment's cluster). */
  cluster?: string | null;
}

/** Every assignable resource the caller can see, keyed by `kind:id`. */
export function useScopeIndex() {
  const ws = useWorkspace();
  const groups = useProjects();
  const envs = useAllEnvironments();
  const apps = useServices();
  const clusters = useClusters();
  const servers = useServers();
  const loading = ws.isPending || groups.isPending || envs.isPending || clusters.isPending;
  const nodes = useMemo(() => {
    const out: ScopeNode[] = [];
    const w = ws.data;
    if (w?.id) out.push({ key: `workspace:${w.id}`, kind: "workspace", id: w.id, name: w.name || "Workspace", parent: null });
    for (const g of groups.data ?? []) out.push({ key: `project:${g.id}`, kind: "project", id: g.id, name: g.name, parent: null });
    for (const e of envs.data ?? [])
      out.push({ key: `environment:${e.id}`, kind: "environment", id: e.id, name: e.name, parent: e.project_id ? `project:${e.project_id}` : null, cluster: e.cluster_id });
    for (const a of Array.isArray(apps.data) ? apps.data : [])
      if (a && typeof a.id === "string") out.push({ key: `service:${a.id}`, kind: "service", id: a.id, name: a.name ?? a.id, parent: a.environment_id ? `environment:${a.environment_id}` : null });
    for (const c of Array.isArray(clusters.data) ? clusters.data : [])
      if (c && typeof c.id === "string") out.push({ key: `cluster:${c.id}`, kind: "cluster", id: c.id, name: c.name ?? c.id, parent: null });
    for (const s of Array.isArray(servers.data) ? servers.data : [])
      if (s && typeof s.id === "string")
        out.push({ key: `server:${s.id}`, kind: "server", id: s.id, name: s.display_name || s.hostname || s.id.slice(0, 8), parent: s.cluster_id ? `cluster:${s.cluster_id}` : null });
    return out;
  }, [ws.data, groups.data, envs.data, apps.data, clusters.data, servers.data]);
  const byKey = useMemo(() => new Map(nodes.map((n) => [n.key, n])), [nodes]);
  return { nodes, byKey, loading, workspaceId: ws.data?.id ?? null };
}

export type ScopeIndex = ReturnType<typeof useScopeIndex>;

/** "shop / prod / api" for a scope; the raw id when the resource is unknown (deleted or hidden). */
export function scopePath(idx: ScopeIndex, s: ScopeRef): string {
  const parts: string[] = [];
  let n = idx.byKey.get(scopeKey(s));
  if (!n) return s.id ? s.id.slice(0, 8) : "—";
  for (let i = 0; n && i < 4; i++) {
    parts.unshift(n.name);
    n = n.parent ? idx.byKey.get(n.parent) : undefined;
  }
  return parts.join(" / ");
}

/**
 * Scopes whose assignments also apply at `key`: its parents up the tree and
 * the workspace, nearest first. Unknown keys get the workspace only.
 */
export function ancestorsOf(idx: ScopeIndex, key: string): string[] {
  const out: string[] = [];
  let p = idx.byKey.get(key)?.parent ?? null;
  for (let i = 0; p && i < 5; i++) {
    out.push(p);
    p = idx.byKey.get(p)?.parent ?? null;
  }
  const ws = idx.workspaceId ? `workspace:${idx.workspaceId}` : null;
  if (ws && key !== ws && !out.includes(ws)) out.push(ws);
  return out;
}

/** The scope tree flattened in display order (workspace, projects with their environments and services, clusters with their nodes), with depth. */
export function flatScopes(idx: ScopeIndex): { node: ScopeNode; depth: number }[] {
  const kids = new Map<string, ScopeNode[]>();
  const roots: ScopeNode[] = [];
  for (const n of idx.nodes) {
    if (n.parent && idx.byKey.has(n.parent)) kids.set(n.parent, [...(kids.get(n.parent) ?? []), n]);
    else roots.push(n);
  }
  const order = ["workspace", "project", "environment", "service", "cluster", "server"];
  const sort = (l: ScopeNode[]) => [...l].sort((a, b) => order.indexOf(a.kind) - order.indexOf(b.kind) || a.name.localeCompare(b.name));
  const out: { node: ScopeNode; depth: number }[] = [];
  const walk = (l: ScopeNode[], depth: number) => {
    for (const n of sort(l)) {
      out.push({ node: n, depth });
      walk(kids.get(n.key) ?? [], depth + 1);
    }
  };
  walk(roots, 0);
  return out;
}

/** Level chip plus the resource path, linked to its page. */
export function ScopeLabel({ idx, scope }: { idx: ScopeIndex; scope: ScopeRef }) {
  const path = scopePath(idx, scope);
  return (
    <span className="flex min-w-0 items-center gap-1.5 text-[13px]" title={`${kindLabel(scope.kind)} · ${path} (${scopeKey(scope)})`}>
      <LevelChip kind={scope.kind} />
      <ScopeLink idx={idx} scope={scope} text={path} />
    </span>
  );
}

/** `text` (default: the resource path) linked to the resource's page; plain text when it has none. */
export function ScopeLink({ idx, scope, text }: { idx: ScopeIndex; scope: ScopeRef; text?: string }) {
  const path = text ?? scopePath(idx, scope);
  const n = idx.byKey.get(scopeKey(scope));
  const cls = "truncate hover:text-accent";
  let link: ReactNode = <span className="truncate" title={path}>{path}</span>;
  if (n) {
    if (scope.kind === "project") link = <Link to="/projects" search={{ sel: `project:${scope.id}` }} className={cls} title={path}>{path}</Link>;
    else if (scope.kind === "service") link = <Link to="/services/$serviceId" params={{ serviceId: scope.id }} className={cls} title={path}>{path}</Link>;
    else if (scope.kind === "cluster") link = <Link to="/clusters/$clusterId" params={{ clusterId: scope.id }} className={cls} title={path}>{path}</Link>;
    else if (scope.kind === "server") link = <Link to="/nodes/$nodeId" params={{ nodeId: scope.id }} className={cls} title={path}>{path}</Link>;
    else if (scope.kind === "environment" && n.cluster)
      link = <Link to="/projects" search={{ sel: `env:${scope.id}` }} className={cls} title={path}>{path}</Link>;
  }
  return link;
}

interface TreeRow {
  node: ScopeNode | { key: string; kind: "heading"; name: string };
  depth: number;
  hasChildren: boolean;
}

/**
 * Tree picker over every assignable resource. `value` is `kind:id`.
 * Searching shows matches with their ancestors, all expanded.
 */
export function ScopePicker({ idx, value, onChange, height = 260 }: { idx: ScopeIndex; value: string; onChange: (v: string) => void; height?: number }) {
  const [q, setQ] = useState("");
  const [open, setOpen] = useState<Set<string>>(() => new Set(["h:apps", "h:clusters"]));
  const children = useMemo(() => {
    const m = new Map<string, ScopeNode[]>();
    for (const n of idx.nodes) {
      const p = n.parent && idx.byKey.has(n.parent) ? n.parent : n.kind === "project" ? "h:apps" : n.kind === "cluster" ? "h:clusters" : n.kind === "environment" ? "h:apps" : n.kind === "server" ? "h:clusters" : n.kind === "workspace" ? "root" : "h:apps";
      m.set(p, [...(m.get(p) ?? []), n]);
    }
    for (const l of m.values()) l.sort((a, b) => (a.kind === b.kind ? a.name.localeCompare(b.name) : a.kind === "project" || a.kind === "cluster" ? -1 : 1));
    return m;
  }, [idx.nodes, idx.byKey]);

  const needle = q.trim().toLowerCase();
  const visible = useMemo(() => {
    if (!needle) return null;
    const keep = new Set<string>();
    for (const n of idx.nodes) {
      if (!n.name.toLowerCase().includes(needle) && !n.id.startsWith(needle)) continue;
      let k: string | null = n.key;
      for (let i = 0; k && i < 5; i++) {
        keep.add(k);
        k = idx.byKey.get(k)?.parent ?? null;
      }
    }
    return keep;
  }, [needle, idx.nodes, idx.byKey]);

  const rows: TreeRow[] = [];
  const walk = (parent: string, depth: number) => {
    for (const n of children.get(parent) ?? []) {
      if (visible && !visible.has(n.key)) continue;
      const kids = (children.get(n.key) ?? []).length > 0;
      rows.push({ node: n, depth, hasChildren: kids });
      if (kids && (visible || open.has(n.key))) walk(n.key, depth + 1);
    }
  };
  walk("root", 0);
  for (const [h, label] of [
    ["h:apps", "Projects"],
    ["h:clusters", "Clusters"],
  ] as const) {
    if (visible && !(children.get(h) ?? []).some((n) => visible.has(n.key))) continue;
    rows.push({ node: { key: h, kind: "heading", name: label }, depth: 0, hasChildren: (children.get(h) ?? []).length > 0 });
    if (visible || open.has(h)) walk(h, 1);
  }

  const toggle = (k: string) =>
    setOpen((s) => {
      const n = new Set(s);
      if (n.has(k)) n.delete(k);
      else n.add(k);
      return n;
    });

  return (
    <div className="rounded-md border border-line">
      <div className="border-b border-line p-1.5">
        <SearchInput value={q} onChange={setQ} placeholder="Search scopes…" />
      </div>
      {idx.loading ? (
        <Skeleton rows={3} />
      ) : (
        <ul role="tree" aria-label="Scope" className="overflow-y-auto py-1 text-[13px]" style={{ maxHeight: height }}>
          {rows.length === 0 && <li className="px-3 py-2 text-xs text-muted">No resource matches.</li>}
          {rows.map(({ node, depth, hasChildren }) => {
            const heading = node.kind === "heading";
            const expanded = !!visible || open.has(node.key);
            const selected = node.key === value;
            return (
              <li key={node.key} role="treeitem" aria-selected={selected} aria-expanded={hasChildren ? expanded : undefined} className="flex items-center" style={{ paddingLeft: 4 + depth * 14 }}>
                <button
                  type="button"
                  tabIndex={hasChildren && !visible ? 0 : -1}
                  aria-label={expanded ? `Collapse ${node.name}` : `Expand ${node.name}`}
                  onClick={() => hasChildren && !visible && toggle(node.key)}
                  className={`grid size-5 shrink-0 place-items-center text-faint ${hasChildren ? "hover:text-fg" : "invisible"}`}
                >
                  {expanded ? <ChevronDown className="size-3.5" /> : <ChevronRight className="size-3.5" />}
                </button>
                {heading ? (
                  <span className="py-1 text-[11px] font-medium tracking-wide text-muted uppercase">{node.name}</span>
                ) : (
                  <button
                    type="button"
                    onClick={() => onChange(node.key)}
                    className={`mr-1 flex min-w-0 flex-1 items-center gap-1.5 rounded px-1.5 py-1 text-left ${selected ? "bg-accent-soft text-accent" : "hover:bg-panel-2"}`}
                  >
                    <span className="truncate" title={node.name}>
                      {node.name}
                    </span>
                    <span className="ml-auto shrink-0 text-[10px] tracking-wide text-faint uppercase">{kindLabel(node.kind)}</span>
                  </button>
                )}
              </li>
            );
          })}
        </ul>
      )}
    </div>
  );
}
