import { Link, useNavigate } from "@tanstack/react-router";
import { AppWindow, ChevronDown, ChevronRight, ChevronsDownUp, ChevronsUpDown, ExternalLink, FolderInput, Layers, Link2, Lock, Pencil, Play, Plus, RefreshCw, RotateCw, ScrollText, Server as ServerIcon, ServerCog, Square, Boxes, Trash2 } from "lucide-react";
import { useEffect, useMemo, useRef, useState, type KeyboardEvent, type ReactNode } from "react";

import { useIsFetching, useQueryClient } from "@tanstack/react-query";

import { explorerKeys, useExplorerTree } from "../../api/explorer";
import { useServiceSwarm, useSwarm } from "../../api/swarm";
import { ReplicaBar } from "../../components/service/ReplicaBar";
import { fleetKeys, useAllEnvironments, useClusters, useProjects, useServiceAction } from "../../api/hooks";
import type { Environment, Project } from "../../api/types";
import type { ProjectsSearch } from "../../app/router";
import { useTerminals } from "../../app/terminals";
import { errorText, useToast } from "../../app/toast";
import { DeleteEnvironmentConfirm, MoveEnvironmentModal, NewEnvironmentModal, RenameEnvironmentModal, SetProjectModal } from "../../components/EnvironmentDialogs";
import { FilterBar } from "../../components/filters/FilterBar";
import { InstanceStateChip } from "../../components/InstanceState";
import { DeleteProjectConfirm, ProjectModal } from "../../components/ProjectDialogs";
import { isStackService } from "../../components/stacks/StackBadge";
import { TagEditor } from "../../components/TagEditor";
import { ServiceLogDrawer } from "./ServiceLogDrawer";
import { PinToggle, usePinAction } from "../../components/Pins";
import { Badge, Button, Confirm, CopyIcon, Status, Empty, ErrorNote, IconButton, Panel, RowMenu, type RowAction, Skeleton, ago, time, useRowMenu } from "../../components/ui";
import {
  filterTree,
  flattenVisible,
  locate,
  UNASSIGNED_KEY,
  type Rollout,
  type RunHealth,
  type TreeEnv,
  type TreeNode,
  type TreePlacement,
  type TreeProject,
  type TreeService,
  type VisibleRow,
} from "../../lib/explorerTree";

const EXPANDED_KEY = "forge.explorer.expanded";
const TAGS_SHOWN = 3;

function readExpanded(): Set<string> {
  try {
    const v = JSON.parse(localStorage.getItem(EXPANDED_KEY) ?? "[]");
    return new Set(Array.isArray(v) ? v.filter((x): x is string => typeof x === "string") : []);
  } catch {
    return new Set();
  }
}

function writeExpanded(s: Set<string>) {
  try {
    localStorage.setItem(EXPANDED_KEY, JSON.stringify([...s]));
  } catch {
    /* storage unavailable: expansion just isn't remembered */
  }
}

/** Debounce a fast-changing value (search box). */
function useDebounced<T>(value: T, ms: number): T {
  const [v, setV] = useState(value);
  useEffect(() => {
    const t = setTimeout(() => setV(value), ms);
    return () => clearTimeout(t);
  }, [value, ms]);
  return v;
}

// ------------------------------------------------------------------ dialogs

/** A dialog the explorer or the page header opens. Rendered by `ExplorerDialogs` only while set. */
export type ExplorerDialog =
  | { kind: "new-project" }
  | { kind: "edit-project"; project: Project }
  | { kind: "delete-project"; project: Project }
  | { kind: "new-env"; projectId?: string }
  | { kind: "rename-env"; env: Environment }
  | { kind: "move-env"; env: Environment }
  | { kind: "set-project"; env: Environment }
  | { kind: "delete-env"; env: Environment; serviceCount: number };

export function ExplorerDialogs({ dialog, onClose, onSelect }: { dialog: ExplorerDialog; onClose: () => void; onSelect: (sel: string | undefined) => void }) {
  switch (dialog.kind) {
    case "new-project":
      return <ProjectModal onClose={onClose} onCreated={(g) => g.id && onSelect(`project:${g.id}`)} />;
    case "edit-project":
      return <ProjectModal onClose={onClose} project={dialog.project} />;
    case "delete-project":
      return <DeleteProjectConfirm project={dialog.project} onClose={onClose} onDeleted={() => onSelect(undefined)} />;
    case "new-env":
      return <NewEnvironmentModal onClose={onClose} projectId={dialog.projectId} fixedProject={!!dialog.projectId} onCreated={(n) => onSelect(`env:${n.id}`)} />;
    case "rename-env":
      return <RenameEnvironmentModal env={dialog.env} onClose={onClose} />;
    case "move-env":
      return <MoveEnvironmentModal env={dialog.env} onClose={onClose} />;
    case "set-project":
      return <SetProjectModal env={dialog.env} onClose={onClose} />;
    case "delete-env":
      return <DeleteEnvironmentConfirm env={dialog.env} serviceCount={dialog.serviceCount} onClose={onClose} onDeleted={() => onSelect(undefined)} />;
  }
}

/** Full rows behind the tree's nodes (the dialogs need them) and the actions on them. */
function useNodeActions(onDialog: (d: ExplorerDialog) => void, onLogs: (serviceId: string) => void) {
  const navigate = useNavigate();
  const projects = useProjects().data ?? [];
  const envs = useAllEnvironments().data ?? [];
  const clusters = useClusters().data ?? [];
  const projectOf = (n: TreeProject) => (n.id ? projects.find((g) => g.id === n.id) : undefined);
  const envOf = (n: TreeEnv) => envs.find((e) => e.id === n.id);

  const pinAction = usePinAction();
  const projectActions = (n: TreeProject): RowAction[] => {
    const g = projectOf(n);
    return [
      ...(n.id ? [pinAction({ kind: "project", id: n.id, label: n.name })] : []),
      { label: "New environment…", icon: Plus, hidden: !n.id, onSelect: () => onDialog({ kind: "new-env", projectId: n.id ?? undefined }) },
      { label: "Edit…", icon: Pencil, hidden: !g, onSelect: () => g && onDialog({ kind: "edit-project", project: g }) },
      { label: "Delete…", icon: Trash2, danger: true, hidden: !g, disabled: n.envs.length > 0, hint: "Delete or move its environments first", onSelect: () => g && onDialog({ kind: "delete-project", project: g }) },
    ];
  };
  const envActions = (n: TreeEnv): RowAction[] => {
    const e = envOf(n);
    return [
      { label: "New service here", icon: Plus, disabled: !n.clusterId, hint: "Move it to a cluster first", onSelect: () => void navigate({ to: "/services/new", search: { environment: n.id } }) },
      { label: "Rename…", icon: Pencil, hidden: !e, onSelect: () => e && onDialog({ kind: "rename-env", env: e }) },
      { label: e?.project_id ? "Move to project…" : "Attach to project…", icon: Link2, hidden: !e, onSelect: () => e && onDialog({ kind: "set-project", env: e }) },
      {
        label: "Move to cluster…",
        icon: FolderInput,
        hidden: !e,
        disabled: clusters.filter((c) => c.id !== n.clusterId).length === 0,
        hint: "No other cluster",
        onSelect: () => e && onDialog({ kind: "move-env", env: e }),
      },
      { label: "Delete…", icon: Trash2, danger: true, hidden: !e, disabled: n.services.length > 0, hint: `Still has ${n.services.length} service${n.services.length === 1 ? "" : "s"}`, onSelect: () => e && onDialog({ kind: "delete-env", env: e, serviceCount: n.services.length }) },
    ];
  };
  const serviceActions = (n: TreeService): RowAction[] => [
    n.swarm
      ? { label: "Logs", icon: ScrollText, tone: "accent", onSelect: () => void navigate({ to: "/services/$serviceId", params: { serviceId: n.id } }) }
      : { label: "Logs", icon: ScrollText, tone: "accent", disabled: n.placements.length === 0, hint: n.placements.length === 0 ? "Not placed on any node" : undefined, onSelect: () => onLogs(n.id) },
    { label: "View", icon: ExternalLink, onSelect: () => void navigate({ to: "/services/$serviceId", params: { serviceId: n.id } }) },
    { label: "Edit configuration", icon: ServerIcon, onSelect: () => void navigate({ to: "/services/$serviceId", params: { serviceId: n.id }, search: { tab: "configuration" } }) },
    pinAction({ kind: "service", id: n.id, label: n.name }),
  ];
  const actionsFor = (n: TreeNode): RowAction[] => (n.kind === "project" ? projectActions(n) : n.kind === "env" ? envActions(n) : serviceActions(n));
  return { actionsFor, projectOf, envOf, clusters };
}

// ------------------------------------------------------------------ explorer

export interface ExplorerProps {
  /** URL state: selection (`project:…`, `env:…`, `service:…`), search and filters. */
  search: ProjectsSearch;
  onSearch: (patch: Partial<ProjectsSearch>) => void;
  onDialog: (d: ExplorerDialog) => void;
}

/**
 * Project Explorer (issue #74): Project → Environment → Service tree with
 * node placements, ancestry-preserving search and a details pane.
 */
export function ProjectExplorer({ search, onSearch, onDialog }: ExplorerProps) {
  const { tree, isLoading, error, healthError, healthTruncated } = useExplorerTree();
  const rm = useRowMenu();
  const [logsFor, setLogsFor] = useState<string>();
  const [expanded, setExpandedState] = useState<Set<string>>(readExpanded);
  const query = search.q ?? "";
  const debounced = useDebounced(query, 200);
  const selected = search.sel;
  const onSelect = (sel: string | undefined) => onSearch({ sel });
  const acts = useNodeActions(onDialog, setLogsFor);
  const health = search.health ?? "all";
  const runtime = search.runtime ?? "";
  const cluster = search.cluster ?? "";

  const setExpanded = (s: Set<string>) => {
    setExpandedState(s);
    writeExpanded(s);
  };
  const toggle = (key: string) => {
    const s = new Set(expanded);
    if (s.has(key)) s.delete(key);
    else s.add(key);
    setExpanded(s);
  };

  const result = useMemo(() => filterTree(tree, { query: debounced, health, runtime, cluster }), [tree, debounced, health, runtime, cluster]);
  // While searching or filtering, matches open with their ancestors; the user's own expansion is kept underneath.
  const effective = useMemo(() => new Set([...expanded, ...result.expand]), [expanded, result.expand]);
  const rows = useMemo(() => flattenVisible(result.tree, effective), [result.tree, effective]);
  const runtimes = useMemo(() => [...new Set(tree.flatMap((a) => a.envs.flatMap((e) => e.services.map((s) => s.runtime))))].sort(), [tree]);
  const found = selected ? locate(tree, selected) : null;
  const filtering = !!debounced.trim() || health !== "all" || !!runtime || !!cluster;
  const logService = logsFor ? (locate(tree, `service:${logsFor}`)?.node as TreeService | undefined) : undefined;
  const allKeys = () => new Set(tree.flatMap((a) => [a.key, ...a.envs.map((e) => e.key)]));
  const projectCount = result.tree.filter((p) => p.key !== UNASSIGNED_KEY).length;

  return (
    <>
      <FilterBar
        search={{ value: query, onChange: (q) => onSearch({ q: q || undefined }), placeholder: "Search projects, services, nodes" }}
        filters={[
          {
            id: "cluster",
            label: "Cluster",
            options: acts.clusters.map((c) => ({ value: c.id, label: c.name })),
            value: cluster ? [cluster] : [],
            onChange: (v) => onSearch({ cluster: v[0] }),
            hidden: acts.clusters.length < 2 && !cluster,
          },
          { id: "runtime", label: "Runtime", options: runtimes.map((r) => ({ value: r, label: r })), value: runtime ? [runtime] : [], onChange: (v) => onSearch({ runtime: v[0] }) },
          {
            id: "health",
            label: "Health",
            options: [
              { value: "unhealthy", label: "Unhealthy" },
              { value: "deploying", label: "Deploying" },
            ],
            value: search.health ? [search.health] : [],
            onChange: (v) => onSearch({ health: v[0] === "unhealthy" || v[0] === "deploying" ? v[0] : undefined }),
          },
        ]}
        onClear={() => onSearch({ q: undefined, cluster: undefined, runtime: undefined, health: undefined })}
      />
      <div className="grid min-h-[480px] gap-3 lg:grid-cols-[minmax(340px,2fr)_3fr]">
        <Panel
          className={`flex min-w-0 flex-col ${found ? "max-lg:hidden" : ""}`}
          title={isLoading ? "Projects" : `${projectCount} project${projectCount === 1 ? "" : "s"} · ${result.serviceCount} service${result.serviceCount === 1 ? "" : "s"}`}
          actions={
            <div className="flex items-center gap-1">
              <RefreshButton />
              <IconButton label="Expand all" onClick={() => setExpanded(allKeys())}>
                <ChevronsUpDown className="size-4" />
              </IconButton>
              <IconButton label="Collapse all" onClick={() => setExpanded(new Set())}>
                <ChevronsDownUp className="size-4" />
              </IconButton>
            </div>
          }
        >
          <ErrorNote error={error} />
          {(!!healthError || healthTruncated) && (
            <p className="border-b border-line px-3 py-1.5 text-[11px] text-warn">
              {healthError ? "Health checks could not be loaded; dots show the state nodes last reported." : "Health checks cover the first 500 services; the rest show the state nodes last reported."}
            </p>
          )}
          {isLoading ? (
            <Skeleton rows={6} />
          ) : error ? null : tree.length === 0 ? (
            <Empty
              icon={<AppWindow className="size-5" />}
              title="No projects yet"
              action={
                <Button variant="primary" onClick={() => onDialog({ kind: "new-project" })}>
                  <Plus className="size-4" /> New project
                </Button>
              }
            >
              A project is a product (e.g. <b>shop</b>) with environments (<b>test</b>, <b>prod</b>) that each hold its services.
            </Empty>
          ) : rows.length === 0 ? (
            <p className="px-4 py-6 text-center text-xs text-muted">{filtering ? "Nothing matches the search or filters." : "Nothing to show."}</p>
          ) : (
            <ResourceTree
              rows={rows}
              expanded={effective}
              selected={selected}
              onSelect={onSelect}
              onToggle={toggle}
              actionsFor={acts.actionsFor}
              rowProps={(n) => rm.rowProps(acts.actionsFor(n), `Actions for ${n.name}`)}
            />
          )}
        </Panel>
        <div className={`min-w-0 ${found ? "" : "max-lg:hidden"}`}>
          {found ? (
            <DetailsPane node={found.node} path={found.path} onSelect={onSelect} onLogs={setLogsFor} acts={acts} />
          ) : (
            <Panel title="Details">
              <p className="px-4 py-8 text-center text-xs text-muted">{selected && !isLoading ? "That resource no longer exists or you cannot view it." : "Select a project, environment or service."}</p>
            </Panel>
          )}
        </div>
      </div>
      {rm.menu}
      {logService && <ServiceLogDrawer service={logService} onClose={() => setLogsFor(undefined)} />}
    </>
  );
}

// ------------------------------------------------------------------ tree

function ResourceTree({
  rows,
  expanded,
  selected,
  onSelect,
  onToggle,
  actionsFor,
  rowProps,
}: {
  rows: VisibleRow[];
  expanded: ReadonlySet<string>;
  selected?: string;
  onSelect: (k: string) => void;
  onToggle: (k: string) => void;
  actionsFor: (n: TreeNode) => RowAction[];
  rowProps: (n: TreeNode) => { onContextMenu: (ev: React.MouseEvent<HTMLElement>) => void };
}) {
  const ref = useRef<HTMLUListElement>(null);
  const [focusKey, setFocusKey] = useState<string>();
  const current = rows.find((r) => r.node.key === (focusKey ?? selected)) ?? rows[0];

  const focus = (key: string) => {
    setFocusKey(key);
    ref.current?.querySelector<HTMLElement>(`[data-key="${CSS.escape(key)}"]`)?.focus();
  };

  const onKeyDown = (e: KeyboardEvent<HTMLUListElement>) => {
    const i = rows.findIndex((r) => r.node.key === current?.node.key);
    if (i < 0) return;
    const row = rows[i];
    if (!row) return;
    const open = expanded.has(row.node.key);
    const go = (j: number) => rows[j] && focus(rows[j].node.key);
    switch (e.key) {
      case "ArrowDown":
        go(i + 1);
        break;
      case "ArrowUp":
        go(i - 1);
        break;
      case "Home":
        go(0);
        break;
      case "End":
        go(rows.length - 1);
        break;
      case "ArrowRight":
        if (row.hasChildren && !open) onToggle(row.node.key);
        else if (row.hasChildren) go(i + 1);
        break;
      case "ArrowLeft":
        if (row.hasChildren && open) onToggle(row.node.key);
        else if (row.parentKey) focus(row.parentKey);
        break;
      case "Enter":
      case " ":
        onSelect(row.node.key);
        break;
      default:
        if (e.key.length === 1 && /\S/.test(e.key)) {
          // Type-ahead: next row whose name starts with the key.
          const k = e.key.toLowerCase();
          const order = [...rows.slice(i + 1), ...rows.slice(0, i + 1)];
          const hit = order.find((r) => r.node.name.toLowerCase().startsWith(k));
          if (hit) focus(hit.node.key);
        }
        return;
    }
    e.preventDefault();
  };

  return (
    <ul ref={ref} role="tree" aria-label="Projects" className="max-h-[70vh] overflow-auto py-1" onKeyDown={onKeyDown}>
      {rows.map((r) => (
        <TreeRow
          key={r.node.key}
          row={r}
          open={expanded.has(r.node.key)}
          selected={selected === r.node.key}
          tabbable={current?.node.key === r.node.key}
          onSelect={onSelect}
          onToggle={onToggle}
          onFocus={setFocusKey}
          actions={r.node.kind === "service" ? undefined : actionsFor(r.node)}
          menu={rowProps(r.node)}
        />
      ))}
    </ul>
  );
}

function TreeRow({
  row,
  open,
  selected,
  tabbable,
  onSelect,
  onToggle,
  onFocus,
  actions,
  menu,
}: {
  row: VisibleRow;
  open: boolean;
  selected: boolean;
  tabbable: boolean;
  onSelect: (k: string) => void;
  onToggle: (k: string) => void;
  onFocus: (k: string) => void;
  /** Row menu (projects and environments); services act from the details pane. */
  actions?: RowAction[];
  menu: { onContextMenu: (ev: React.MouseEvent<HTMLElement>) => void };
}) {
  const n = row.node;
  return (
    <li
      role="treeitem"
      aria-level={row.depth + 1}
      aria-expanded={row.hasChildren ? open : undefined}
      aria-selected={selected}
      data-key={n.key}
      tabIndex={tabbable ? 0 : -1}
      onFocus={() => onFocus(n.key)}
      onClick={() => onSelect(n.key)}
      {...menu}
      className={`group flex min-w-0 cursor-pointer items-center gap-1.5 py-1 pr-2 text-[13px] outline-none focus-visible:ring-2 focus-visible:ring-accent/60 ${selected ? "bg-accent-soft" : "hover:bg-panel-2/60"}`}
      style={{ paddingLeft: `${8 + row.depth * 16}px` }}
    >
      {row.hasChildren ? (
        <button
          type="button"
          tabIndex={-1}
          aria-hidden
          className="grid size-4 shrink-0 place-items-center text-muted hover:text-fg"
          onClick={(e) => {
            e.stopPropagation();
            onToggle(n.key);
          }}
        >
          {open ? <ChevronDown className="size-3.5" /> : <ChevronRight className="size-3.5" />}
        </button>
      ) : (
        <span className="size-4 shrink-0" />
      )}
      <KindIcon node={n} />
      <span className={`truncate ${n.kind === "service" ? "" : "font-medium"}`} title={n.name}>
        {n.name}
      </span>
      {n.kind === "env" && n.protected && <Lock className="size-3 shrink-0 text-warn" aria-label="protected" />}
      <HealthDot health={n.health} />
      {n.kind === "service" && <RolloutMark rollout={n.rollout} />}
      {n.kind === "service" && (
        <>
          <span className="ml-auto" />
          <PlacementTags service={n} />
        </>
      )}
      {n.kind !== "service" && <span className="ml-auto shrink-0 font-mono text-[11px] text-faint tabular-nums">{n.kind === "project" ? n.envs.length : n.services.length}</span>}
      {actions && (
        // Stops the click from selecting the row; Escape from the menu returns focus here.
        <span className="-my-1.5 flex h-5 shrink-0 items-center opacity-0 group-hover:opacity-100 group-focus-within:opacity-100 has-[[data-state=open]]:opacity-100" onClick={(e) => e.stopPropagation()} onKeyDown={(e) => e.stopPropagation()}>
          <RowMenu actions={actions} label={`Actions for ${n.name}`} />
        </span>
      )}
    </li>
  );
}

function KindIcon({ node }: { node: TreeNode }) {
  const cls = "size-3.5 shrink-0 text-accent";
  if (node.kind === "project") return <AppWindow className={cls} aria-hidden />;
  if (node.kind === "env") return <Layers className={cls} aria-hidden />;
  return <Boxes className={cls} aria-hidden />;
}

const HEALTH: Record<RunHealth, { cls: string; label: string }> = {
  ok: { cls: "bg-ok", label: "Running" },
  warn: { cls: "bg-warn", label: "Degraded" },
  bad: { cls: "bg-bad", label: "Failed" },
  stopped: { cls: "bg-warn/60", label: "Stopped" },
  unknown: { cls: "bg-faint", label: "Unknown" },
};

function HealthDot({ health }: { health: RunHealth }) {
  const h = HEALTH[health];
  return <span role="img" aria-label={h.label} title={h.label} className={`size-1.5 shrink-0 rounded-full ${h.cls}`} />;
}

/** Rollout state, shown apart from the running dot. */
function RolloutMark({ rollout }: { rollout: Rollout }) {
  if (rollout === "idle") return null;
  return <Badge tone={rollout === "failed" ? "bad" : "accent"}>{rollout === "failed" ? "deploy failed" : "deploying"}</Badge>;
}

function PlacementTags({ service }: { service: TreeService }) {
  const [all, setAll] = useState(false);
  const shown = all ? service.placements : service.placements.slice(0, TAGS_SHOWN);
  const more = service.placements.length - shown.length;
  if (service.swarm) return <span className="shrink-0 rounded bg-accent-soft px-1.5 font-mono text-[10px] text-accent" title="Placed by Docker Swarm">swarm</span>;
  if (service.placements.length === 0) return <span className="shrink-0 text-[11px] text-faint">not placed</span>;
  return (
    <span className="flex min-w-0 shrink flex-wrap justify-end gap-1">
      {shown.map((p) => (
        <PlacementTag key={p.key} p={p} />
      ))}
      {more > 0 && (
        <button
          type="button"
          className="rounded border border-line px-1 text-[11px] text-muted hover:text-fg"
          onClick={(e) => {
            e.stopPropagation();
            setAll(true);
          }}
        >
          +{more}
        </button>
      )}
    </span>
  );
}

function placementTitle(p: TreePlacement): string {
  const i = p.instance;
  return `${p.nodeName} · ${i.role} · ${i.status}${i.state ? ` · ${i.state}` : ""}${i.last_error ? `\n${i.last_error}` : ""}`;
}

function PlacementTag({ p }: { p: TreePlacement }) {
  return (
    <Link
      to="/nodes/$nodeId"
      params={{ nodeId: p.serverId }}
      onClick={(e) => e.stopPropagation()}
      title={placementTitle(p)}
      className={`inline-flex max-w-[120px] items-center gap-1 rounded border px-1 text-[11px] hover:border-accent hover:text-accent ${p.instance.role === "standby" ? "border-dashed border-line text-muted" : "border-line"}`}
    >
      <span className={`size-1.5 shrink-0 rounded-full ${HEALTH[p.health].cls}`} aria-hidden />
      <span className="truncate">{p.nodeName}</span>
    </Link>
  );
}

// ------------------------------------------------------------------ details

/** Refetches services (with node states), health checks and the tree's other lists on demand. */
function RefreshButton() {
  const qc = useQueryClient();
  const keys = [fleetKeys.services, fleetKeys.projects, fleetKeys.environments, ["servers"], explorerKeys.all];
  const busy = useIsFetching({ predicate: (q) => keys.some((k) => k[0] === q.queryKey[0]) }) > 0;
  const refresh = () => {
    for (const key of keys) void qc.invalidateQueries({ queryKey: key });
  };
  return (
    <IconButton label={busy ? "Refreshing…" : "Refresh"} onClick={refresh} disabled={busy} className="enabled:hover:bg-accent-soft enabled:hover:text-accent">
      <RefreshCw className={`size-4 ${busy ? "animate-spin" : ""}`} />
    </IconButton>
  );
}

type NodeActs = ReturnType<typeof useNodeActions>;

function DetailsPane({ node, path, onSelect, onLogs, acts }: { node: TreeNode; path: TreeNode[]; onSelect: (k: string | undefined) => void; onLogs: (serviceId: string) => void; acts: NodeActs }) {
  return (
    <Panel
      title={`${node.kind === "project" ? "Project" : node.kind === "env" ? "Environment" : "Service"} ${node.name}`}
      actions={
        <div className="flex items-center gap-1">
          {node.kind === "project" && node.id && <PinToggle kind="project" id={node.id} label={node.name} />}
          {node.kind === "service" && <PinToggle kind="service" id={node.id} label={node.name} />}
          <RefreshButton />
          {node.kind !== "service" && <RowMenu actions={acts.actionsFor(node)} label={`Actions for ${node.name}`} />}
        </div>
      }
      description={
        <span className="flex min-w-0 flex-wrap items-center gap-1">
          {path.slice(0, -1).map((p) => (
            <span key={p.key} className="inline-flex min-w-0 items-center gap-1">
              <button type="button" className="max-w-[200px] truncate hover:text-accent" title={p.name} onClick={() => onSelect(p.key)}>
                {p.name}
              </button>
              <ChevronRight className="size-3 shrink-0" aria-hidden />
            </span>
          ))}
          <span className="lg:hidden">
            <button type="button" className="text-accent" onClick={() => onSelect(undefined)}>
              Back to tree
            </button>
          </span>
        </span>
      }
    >
      <div className="space-y-3 p-4 text-[13px]">
        {node.kind === "project" && <ProjectDetails key={node.key} p={node} onSelect={onSelect} acts={acts} />}
        {node.kind === "env" && <EnvDetails key={node.key} env={node} onSelect={onSelect} acts={acts} />}
        {node.kind === "service" && <ServiceDetails key={node.id} s={node} onLogs={() => onLogs(node.id)} />}
      </div>
    </Panel>
  );
}

function Field({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="grid grid-cols-[120px_minmax(0,1fr)] items-baseline gap-2">
      <span className="text-xs text-muted">{label}</span>
      <span className="min-w-0">{children}</span>
    </div>
  );
}

/** Id with a copy button, one line. */
function IdLine({ id, label }: { id: string; label: string }) {
  return (
    <span className="inline-flex min-w-0 items-center gap-1">
      <span className="truncate font-mono text-[11px] text-muted select-all" title={id}>
        {id}
      </span>
      <CopyIcon text={id} label={label} />
    </span>
  );
}

/** The pane's main actions as buttons, from the same list as the row menu. */
function ActionButtons({ actions }: { actions: RowAction[] }) {
  return (
    <div className="flex flex-wrap gap-2">
      {actions
        .filter((a) => !a.hidden)
        .map((a) => (
          <Button key={a.label} variant={a.danger ? "danger" : undefined} disabled={a.disabled} title={a.disabled ? a.hint : undefined} onClick={a.onSelect}>
            {a.icon && <a.icon className="size-4" />} {a.label.replace(/…$/, "")}
          </Button>
        ))}
    </div>
  );
}

function ChildList<T extends TreeNode>({ items, onSelect, empty }: { items: T[]; onSelect: (k: string) => void; empty: string }) {
  if (items.length === 0) return <span className="text-xs text-faint">{empty}</span>;
  return (
    <ul className="flex flex-wrap gap-1">
      {items.map((c) => (
        <li key={c.key} className="min-w-0">
          <button type="button" className="inline-flex max-w-[220px] items-center gap-1 rounded border border-line px-1.5 py-0.5 text-[11px] hover:border-accent hover:text-accent" title={c.name} onClick={() => onSelect(c.key)}>
            <HealthDot health={c.health} /> <span className="truncate">{c.name}</span>
          </button>
        </li>
      ))}
    </ul>
  );
}

function ProjectDetails({ p, onSelect, acts }: { p: TreeProject; onSelect: (k: string) => void; acts: NodeActs }) {
  const g = acts.projectOf(p);
  const clusterName = (id: string | null) => acts.clusters.find((c) => c.id === id)?.name;
  const services = p.envs.reduce((n, e) => n + e.services.length, 0);
  return (
    <>
      {p.id ? (
        <ActionButtons actions={acts.actionsFor(p)} />
      ) : (
        <p className="rounded-md bg-warn-soft px-3 py-2 text-xs text-warn">These environments are not part of a project yet. Select one and use <b>Attach to project</b> in its menu.</p>
      )}
      {p.description && <p className="text-muted">{p.description}</p>}
      {p.id && (
        <Field label="Id">
          <IdLine id={p.id} label="Copy project id" />
        </Field>
      )}
      <Field label="Health">
        <HealthLabel health={p.health} />
      </Field>
      <Field label="Services">
        <span className="font-mono text-[11px] tabular-nums">{services}</span>
      </Field>
      <Field label="Created">{g?.created_at ? <span title={time(g.created_at)}>{ago(g.created_at)}</span> : <span className="text-faint">—</span>}</Field>
      <div>
        <h3 className="mb-1 text-xs text-muted">Environments</h3>
        {p.envs.length === 0 ? (
          <p className="text-xs text-faint">No environments yet. Add one (e.g. test or prod) with <b>New environment</b>; each lives in one cluster and holds the services.</p>
        ) : (
          <ul className="divide-y divide-line rounded-md border border-line">
            {p.envs.map((e) => (
              <li key={e.key} className="flex min-w-0 items-center gap-2 px-2 py-1.5 text-xs">
                <HealthDot health={e.health} />
                <button type="button" className="min-w-0 truncate font-medium hover:text-accent" title={e.name} onClick={() => onSelect(e.key)}>
                  {e.name}
                </button>
                {e.protected && <Lock className="size-3 shrink-0 text-warn" aria-label="protected" />}
                <span className="text-faint">→</span>
                {e.clusterId ? (
                  <Link to="/clusters/$clusterId" params={{ clusterId: e.clusterId }} className="inline-flex min-w-0 items-center gap-1 text-muted hover:text-accent" title={clusterName(e.clusterId) ?? e.clusterId}>
                    <ServerCog className="size-3.5 shrink-0" aria-hidden />
                    <span className="truncate">{clusterName(e.clusterId) ?? e.clusterId.slice(0, 8)}</span>
                  </Link>
                ) : (
                  <span className="text-warn">no cluster</span>
                )}
                <span className="ml-auto shrink-0 font-mono text-[11px] text-faint tabular-nums">
                  {e.services.length} service{e.services.length === 1 ? "" : "s"}
                </span>
              </li>
            ))}
          </ul>
        )}
      </div>
    </>
  );
}

function EnvDetails({ env, onSelect, acts }: { env: TreeEnv; onSelect: (k: string) => void; acts: NodeActs }) {
  const full = acts.envOf(env);
  const cl = acts.clusters.find((c) => c.id === env.clusterId);
  return (
    <>
      <ActionButtons actions={acts.actionsFor(env)} />
      <Field label="Id">
        <IdLine id={env.id} label="Copy environment id" />
      </Field>
      <Field label="Cluster">
        {env.clusterId ? (
          <Link to="/clusters/$clusterId" params={{ clusterId: env.clusterId }} className="inline-flex min-w-0 items-center gap-1 hover:text-accent" title={cl?.name ?? env.clusterId}>
            <ServerCog className="size-3.5 shrink-0 text-muted" aria-hidden />
            <span className="truncate">{cl?.name ?? env.clusterId.slice(0, 8)}</span>
          </Link>
        ) : (
          <span className="text-xs text-warn">Not in a cluster; use Move to cluster</span>
        )}
      </Field>
      <Field label="Protection">
        {env.protected ? (
          <Badge tone="warn">
            <Lock className="mr-1 size-3" /> protected
          </Badge>
        ) : (
          <span className="text-xs text-muted">standard</span>
        )}
      </Field>
      <Field label="Health">
        <HealthLabel health={env.health} />
      </Field>
      <Field label="Created">{full?.created_at ? <span title={time(full.created_at)}>{ago(full.created_at)}</span> : <span className="text-faint">—</span>}</Field>
      <Field label="Services">
        <ChildList items={env.services} onSelect={onSelect} empty="No services yet" />
      </Field>
      <Field label="Tags">
        <TagEditor kind="environment" id={env.id} />
      </Field>
    </>
  );
}

const CHECK_TONE = { healthy: "ok", degraded: "warn", unhealthy: "bad", unknown: "muted" } as const;

type NodeAction = "start" | "restart" | "stop";

function ServiceDetails({ s, onLogs }: { s: TreeService; onLogs: () => void }) {
  const terminals = useTerminals();
  const act = useServiceAction();
  const toast = useToast();
  const [busy, setBusy] = useState<string>();
  const [confirm, setConfirm] = useState<{ action: NodeAction; p: TreePlacement }>();
  const stack = isStackService(s.service);
  // `can_deploy` is a hint; absent (older server) means "try, the server decides".
  const denied = s.service.can_deploy === false;
  const blocked = denied ? "Needs the deploy permission on this environment" : stack ? "Managed by its compose stack; act on the stack" : busy ? "Another action is running" : undefined;
  const why = (p: TreePlacement): string | undefined => blocked ?? (p.instance.status === "pending" ? "Not deployed to this node yet" : p.instance.status.endsWith("ing") ? `Wait: ${p.instance.status}` : undefined);

  const qc = useQueryClient();
  // The node reports the new state shortly after the job; refetch a few times so the row follows.
  const refreshSoon = () => {
    for (const ms of [1500, 4000, 9000]) setTimeout(() => void qc.invalidateQueries({ queryKey: fleetKeys.services }).then(() => qc.invalidateQueries({ queryKey: explorerKeys.all })), ms);
  };
  const run = (p: TreePlacement, action: NodeAction) => {
    setBusy(`${p.serverId}:${action}`);
    act.mutate(
      { id: s.id, server: p.serverId, action },
      {
        onSuccess: (r) => (r.status === "failed" ? toast(r.error ?? `${action} failed`, "error") : toast(`${action} ${r.status === "running" ? "started" : "done"}: ${s.name} on ${p.nodeName}`)),
        onError: (e) => toast(errorText(e), "error"),
        onSettled: () => {
          setBusy(undefined);
          refreshSoon();
        },
      },
    );
  };
  return (
    <>
      <div className="flex flex-wrap gap-2">
        {s.swarm ? (
          <Link to="/services/$serviceId" params={{ serviceId: s.id }} className="inline-flex" title="Swarm logs are on the service page">
            <Button>
              <ScrollText className="size-4" /> Logs
            </Button>
          </Link>
        ) : (
          <Button onClick={onLogs} disabled={s.placements.length === 0} title={s.placements.length === 0 ? "Not placed on any node" : "Logs from one node or all of them"}>
            <ScrollText className="size-4" /> Logs
          </Button>
        )}
        <Link to="/services/$serviceId" params={{ serviceId: s.id }} className="inline-flex">
          <Button>
            <ExternalLink className="size-4" /> Open service
          </Button>
        </Link>
      </div>
      {s.swarm ? (
        <SwarmDetails id={s.id} />
      ) : (
        <Field label="Running">
          <HealthLabel health={s.health} />
        </Field>
      )}
      <Field label="Rollout">{s.rollout === "idle" ? <span className="text-muted">idle</span> : <RolloutMark rollout={s.rollout} />}</Field>
      <Field label="Runtime">
        <span className="font-mono text-xs">{s.runtime}</span>
      </Field>
      <Field label="Updated">{ago(s.service.updated_at)}</Field>
      <div hidden={s.swarm}>
        <h3 className="mb-1 text-xs text-muted">Placements</h3>
        {s.placements.length === 0 ? (
          <span className="text-xs text-faint">Not placed on any node</span>
        ) : (
          <ul className="divide-y divide-line rounded-md border border-line">
            {s.placements.map((p) => {
              const w = why(p);
              const pending = p.instance.status === "pending";
              return (
                <li key={p.key} className="flex flex-wrap items-center gap-x-2 gap-y-1 px-2 py-1.5 text-xs">
                  <ServerIcon className="size-3.5 text-muted" aria-hidden />
                  <Link to="/nodes/$nodeId" params={{ nodeId: p.serverId }} className="font-medium hover:text-accent">
                    {p.nodeName}
                  </Link>
                  <Badge tone={p.instance.role === "active" ? "accent" : "muted"}>{p.instance.role === "active" ? "replica" : "standby"}</Badge>
                  <span title={`Deploy: ${p.instance.status}`} className="inline-flex">
                    <Status value={p.instance.status === "deployed" ? "succeeded" : p.instance.status === "failed" ? "failed" : p.instance.status.endsWith("ing") ? "running" : "pending"} />
                  </span>
                  <InstanceStateChip s={p.live} />
                  {p.check && p.live.kind === "running" && <Badge tone={CHECK_TONE[p.check]}>{p.check}</Badge>}
                  <span className="ml-auto flex items-center">
                    <IconButton
                      label={pending ? "Not deployed to this node yet" : `Logs from ${p.nodeName}`}
                      className="size-7 enabled:hover:bg-accent-soft enabled:hover:text-accent disabled:cursor-not-allowed disabled:opacity-35 disabled:hover:bg-transparent disabled:hover:text-muted"
                      disabled={pending}
                      onClick={() => terminals.openLogs({ source: "service", appId: s.id, serverId: p.serverId, name: s.name, node: p.nodeName })}
                    >
                      <ScrollText className="size-3.5" />
                    </IconButton>
                    <IconButton
                      label={w ?? (p.live.kind === "running" ? `Already running on ${p.nodeName}` : `Start on ${p.nodeName}`)}
                      className="size-7 enabled:hover:bg-ok-soft enabled:hover:text-ok disabled:cursor-not-allowed disabled:opacity-35 disabled:hover:bg-transparent disabled:hover:text-muted"
                      disabled={!!w || p.live.kind === "running"}
                      onClick={() => run(p, "start")}
                    >
                      <Play className="size-3.5" />
                    </IconButton>
                    <IconButton
                      label={w ?? `Restart on ${p.nodeName}`}
                      className="size-7 enabled:hover:bg-warn-soft enabled:hover:text-warn disabled:cursor-not-allowed disabled:opacity-35 disabled:hover:bg-transparent disabled:hover:text-muted"
                      disabled={!!w}
                      onClick={() => setConfirm({ action: "restart", p })}
                    >
                      <RotateCw className="size-3.5" />
                    </IconButton>
                    <IconButton
                      label={w ?? (p.live.kind === "stopped" ? `Already stopped on ${p.nodeName}` : `Stop on ${p.nodeName}`)}
                      className="size-7 enabled:hover:bg-bad-soft enabled:hover:text-bad disabled:cursor-not-allowed disabled:opacity-35 disabled:hover:bg-transparent disabled:hover:text-muted"
                      disabled={!!w || p.live.kind === "stopped"}
                      onClick={() => setConfirm({ action: "stop", p })}
                    >
                      <Square className="size-3.5" />
                    </IconButton>
                  </span>
                  {p.instance.last_error && (
                    <span className="w-full truncate pl-5 text-bad" title={p.instance.last_error}>
                      {p.instance.last_error}
                    </span>
                  )}
                </li>
              );
            })}
          </ul>
        )}
      </div>
      <Confirm
        open={!!confirm}
        onOpenChange={(o) => !o && setConfirm(undefined)}
        title={confirm ? `${confirm.action === "stop" ? "Stop" : "Restart"} ${s.name} on ${confirm.p.nodeName}?` : ""}
        body={
          confirm?.action === "stop"
            ? "The service stops on this node until you start it again. Other nodes keep serving."
            : "The service restarts on this node; expect a short interruption for requests routed here."
        }
        confirmLabel={confirm?.action === "stop" ? "Stop" : "Restart"}
        onConfirm={() => {
          const c = confirm;
          setConfirm(undefined);
          if (c) run(c.p, c.action);
        }}
      />
    </>
  );
}

function HealthLabel({ health }: { health: RunHealth }) {
  return (
    <span className="inline-flex items-center gap-1.5 text-xs">
      <HealthDot health={health} /> {HEALTH[health].label}
    </span>
  );
}

/** A Swarm service: its Docker name, running tasks and where they run. */
function SwarmDetails({ id }: { id: string }) {
  const q = useServiceSwarm(id);
  const v = q.data;
  const cluster = useSwarm(v?.cluster_id ?? "", !!v?.cluster_id).data;
  if (q.isLoading) return <Skeleton rows={3} />;
  if (q.error || !v) return <ErrorNote error={q.error ?? "No swarm status"} />;
  const st = v.status;
  const host = (nodeId?: string) => cluster?.nodes?.find((n) => n.id === nodeId)?.hostname ?? cluster?.members.find((m) => m.node_id === nodeId)?.hostname ?? nodeId?.slice(0, 10) ?? "—";
  const tasks = (st?.tasks ?? []).filter((t) => t.desired_state === "running" || t.state === "running").sort((a, b) => (a.slot ?? 0) - (b.slot ?? 0));
  return (
    <>
      <Field label="Running">
        {!st ? (
          <span className="text-xs text-warn">{v.status_error ? `Swarm status unavailable: ${v.status_error}` : "Unknown"}</span>
        ) : !st.exists ? (
          <span className="text-xs text-faint">Not deployed to the swarm yet</span>
        ) : (
          <ReplicaBar v={v} serviceId={id} hostOf={host} />
        )}
      </Field>
      <Field label="Docker name">
        <span className="inline-flex min-w-0 items-center gap-1">
          <span className="truncate font-mono text-xs" title={v.name}>
            {v.name}
          </span>
          <CopyIcon text={v.name} label="Copy Docker name" />
        </span>
      </Field>
      {tasks.length > 0 && (
        <div>
          <h3 className="mb-1 text-xs text-muted">Tasks</h3>
          <ul className="divide-y divide-line rounded-md border border-line">
            {tasks.map((t) => (
              <li key={t.id} className="flex items-center gap-2 px-2 py-1.5 text-xs" title={`${v.name}.${t.slot ?? ""}.${t.id}`}>
                <span className="font-mono text-faint">#{t.slot ?? "·"}</span>
                <ServerIcon className="size-3.5 text-muted" aria-hidden />
                <span className="font-medium">{host(t.node_id)}</span>
                <Badge tone={t.state === "running" ? "ok" : t.state === "failed" || t.state === "rejected" ? "bad" : "muted"}>{t.state}</Badge>
                {t.error && (
                  <span className="min-w-0 truncate text-bad" title={t.error}>
                    {t.error}
                  </span>
                )}
              </li>
            ))}
          </ul>
        </div>
      )}
    </>
  );
}
