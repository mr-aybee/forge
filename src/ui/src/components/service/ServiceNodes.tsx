import { Link } from "@tanstack/react-router";
import { ArrowDownUp, Container, FileText, Play, Plus, RotateCw, ScrollText, Square, Trash2 } from "lucide-react";

import type { ServiceAction } from "../../api/hooks";
import { type InstanceHealth, type Server, type Service, type ServiceInstance, nodeName } from "../../api/types";
import { InstanceHealthBadge } from "../health/InstanceHealth";
import { InstanceStateChip, liveState } from "../InstanceState";
import { Badge, RowMenu, type RowAction, ago, time, useRowMenu } from "../ui";

/** The deploy result chip: what the last job did on the node. */
function DeployChip({ i }: { i: ServiceInstance }) {
  const busy = i.status.endsWith("ing");
  const tone =
    i.status === "deployed" ? "ok" : i.status === "failed" || i.status === "stranded" ? "bad" : busy || i.status === "drifted" ? "warn" : "muted";
  const label = i.status === "pending" ? "not deployed" : i.status || "—";
  return (
    <span className="inline-flex" title={`Deploy: ${label}${i.updated_at ? ` · ${time(i.updated_at)} (${ago(i.updated_at)})` : ""}${i.last_error ? `\n${i.last_error}` : ""}`}>
      <Badge tone={tone}>
        <span aria-hidden className={`mr-1.5 size-1.5 rounded-full bg-current ${busy ? "animate-pulse" : ""}`} />
        {label}
      </Badge>
    </span>
  );
}

/**
 * Where a service runs: one compact line per node (name, role, deploy result,
 * live state, health, Logs and a ⋯ menu, also on right-click), and the nodes
 * it can be added to. Wraps instead of scrolling sideways.
 */
export function ServiceNodes({
  a,
  nodes,
  health,
  containerName,
  onLogs,
  onAction,
  onRole,
  onRemove,
  onDeployOutput,
  onAdd,
}: {
  a: Service;
  nodes: Server[];
  /** Per-node health; undefined hides the health chips (no endpoint or no access). */
  health?: InstanceHealth[];
  /** Docker name of the container, for container services. */
  containerName?: string;
  onLogs: (serverId: string) => void;
  onAction: (serverId: string, action: ServiceAction) => void;
  onRole: (serverId: string, role: "active" | "standby") => void;
  /** Removes the placement (pending) or asks to remove the service from the node. */
  onRemove: (i: ServiceInstance) => void;
  onDeployOutput: (serverId: string) => void;
  onAdd: (node: Server) => void;
}) {
  const rm = useRowMenu();
  const instances = Array.isArray(a.instances) ? a.instances : [];
  const byId = new Map(nodes.map((n) => [n.id, n]));
  const nameOf = (id: string) => {
    const n = byId.get(id);
    return n ? nodeName(n) : id.slice(0, 8);
  };
  const placed = new Set(instances.map((i) => i.server_id));
  const addable = nodes.filter((n) => !placed.has(n.id) && n.status !== "revoked");

  return (
    <div>
      {instances.length === 0 ? (
        <p className="px-3 py-2.5 text-xs text-muted">Not placed on any node yet. Add a node below, then Deploy.</p>
      ) : (
        <ul className="divide-y divide-line/70">
          {instances.map((i) => {
            const node = nameOf(i.server_id);
            const busy = i.status.endsWith("ing");
            const pending = i.status === "pending";
            const why = busy ? `Wait: ${i.status}` : pending ? "Not deployed to this node yet" : undefined;
            const live = liveState(i, byId.get(i.server_id));
            const standby = i.role === "standby";
            const actions: RowAction[] = [
              { label: "Logs", icon: ScrollText, tone: "accent", disabled: pending, hint: pending ? "Not deployed to this node yet" : undefined, onSelect: () => onLogs(i.server_id) },
              { label: "Deployment output", icon: FileText, tone: "accent", onSelect: () => onDeployOutput(i.server_id) },
              { label: "Start", icon: Play, tone: "ok", disabled: busy || pending || live.kind === "running", hint: why ?? (live.kind === "running" ? "Already running on this node" : undefined), onSelect: () => onAction(i.server_id, "start") },
              { label: "Stop", icon: Square, tone: "bad", disabled: busy || pending || live.kind === "stopped", hint: why ?? (live.kind === "stopped" ? "Already stopped on this node" : undefined), onSelect: () => onAction(i.server_id, "stop") },
              { label: "Restart", icon: RotateCw, tone: "warn", disabled: busy || pending, hint: why, onSelect: () => onAction(i.server_id, "restart") },
              { label: "Log tail (one-off)", icon: FileText, tone: "accent", disabled: pending, hint: pending ? "Not deployed to this node yet" : undefined, onSelect: () => onAction(i.server_id, "logs") },
              {
                label: standby ? "Make it a replica" : "Make it a standby",
                icon: ArrowDownUp,
                disabled: busy,
                hint: busy ? `Wait: ${i.status}` : "Apply the endpoint again after a change",
                onSelect: () => onRole(i.server_id, standby ? "active" : "standby"),
              },
              { label: pending ? "Remove placement" : "Remove from node", icon: Trash2, danger: true, disabled: busy, hint: busy ? `Wait: ${i.status}` : undefined, onSelect: () => onRemove(i) },
            ];
            return (
              <li key={i.server_id} className="flex flex-wrap items-center gap-x-2 gap-y-1 px-3 py-1.5 text-[13px] hover:bg-panel-2/50" {...rm.rowProps(actions, `Actions for ${node}`)}>
                <div className="min-w-0 flex-1 basis-32">
                  <div className="flex min-w-0 items-center gap-2">
                    <Link to="/nodes/$nodeId" params={{ nodeId: i.server_id }} className="min-w-0 truncate font-medium hover:text-accent" title={`Open node ${node}`}>
                      {node}
                    </Link>
                    <span
                      className={`shrink-0 rounded px-1 text-[10px] leading-4 ${standby ? "bg-warn-soft text-warn" : "bg-panel-2 text-muted"}`}
                      title={standby ? "Standby: an Nginx backup, used only when every replica is down" : "Replica (active): serves traffic"}
                    >
                      {standby ? "standby" : "replica"}
                    </span>
                  </div>
                  {containerName && !pending && (
                    <Link
                      to="/nodes/$nodeId/containers/$containerId"
                      params={{ nodeId: i.server_id, containerId: containerName }}
                      className="flex min-w-0 items-center gap-1 font-mono text-[11px] text-muted hover:text-accent"
                      title={`Open container ${containerName} on ${node}`}
                    >
                      <Container className="size-3 shrink-0" aria-hidden />
                      <span className="truncate">{containerName}</span>
                    </Link>
                  )}
                  {i.last_error && (
                    <div className="truncate text-xs text-bad" title={i.last_error}>
                      {i.last_error}
                    </div>
                  )}
                </div>
                <div className="flex shrink-0 flex-wrap items-center gap-1.5">
                  <DeployChip i={i} />
                  <InstanceStateChip s={live} />
                  {health && <InstanceHealthBadge health={health.find((h) => h.server_id === i.server_id)} nodeName={node} />}
                </div>
                <div className="flex shrink-0 items-center">
                  <button
                    type="button"
                    disabled={pending}
                    title={pending ? "Not deployed to this node yet" : `Follow ${a.name} logs on ${node} in the bottom panel`}
                    onClick={() => onLogs(i.server_id)}
                    className="press inline-flex h-7 items-center gap-1 rounded-md px-2 text-xs text-muted hover:bg-panel-2 hover:text-fg focus-visible:outline-2 focus-visible:outline-accent disabled:cursor-not-allowed disabled:opacity-50"
                  >
                    <ScrollText className="size-3.5" /> Logs
                  </button>
                  <RowMenu actions={actions.slice(1)} label={`Actions for ${node}`} />
                </div>
              </li>
            );
          })}
        </ul>
      )}
      {rm.menu}
      <div className="flex flex-wrap items-center gap-1.5 border-t border-line/70 px-3 py-1.5">
        <span className="text-xs text-faint">Add node</span>
        {addable.length === 0 ? (
          <span className="text-xs text-faint">— runs on every node</span>
        ) : (
          addable.map((n) => (
            <button
              key={n.id}
              type="button"
              className="press inline-flex h-6 items-center gap-1 rounded-full border border-line px-2 text-xs text-muted hover:border-accent hover:text-accent focus-visible:outline-2 focus-visible:outline-accent"
              title={instances.length ? `Run ${a.name} on ${nodeName(n)} as a replica or a standby` : `Run ${a.name} on ${nodeName(n)}`}
              onClick={() => onAdd(n)}
            >
              <Plus className="size-3" /> {nodeName(n)}
            </button>
          ))
        )}
      </div>
    </div>
  );
}
