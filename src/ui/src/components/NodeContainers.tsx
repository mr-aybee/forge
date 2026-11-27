import { Link, useNavigate } from "@tanstack/react-router";
import * as DropdownMenu from "@radix-ui/react-dropdown-menu";
import { Activity, Boxes, ChevronDown, Container, FileCode2, FileText, Layers, Play, Plus, RefreshCw, RotateCw, Search, Square, SquareTerminal, Trash2 } from "lucide-react";
import { useMemo, useState } from "react";

import {
  type ContainerSummary,
  type DockerInfo,
  containerCall,
  useContainerAction,
  useContainerQuery,
} from "../api/hooks";
import type { Server } from "../api/types";
import { errorText, useToast } from "../app/toast";
import { useStacks } from "../api/stacks";
import { AdoptContainerDialog, isForgeServiceContainer } from "./AdoptContainer";
import { StoredContainerLogs } from "./containers/StoredContainerLogs";
import { StackTable } from "./stacks/StackList";
import { RowMenu, type RowAction, useRowMenu, Button, Confirm, Empty, ErrorNote, IconButton, Modal, Mono, Panel, Skeleton, Status, Table, Td, bytes } from "./ui";

/** Binary units; `—` for 0 or unknown (Docker reports 0 for "no limit"). */
const fmtBytes = (n?: number | null) => (n ? bytes(n) : "—");

/** Docker containers on one node, managed through the node's root helper. */
export function NodeContainers({ node }: { node: Server }) {
  const online = node.status === "online";
  const toast = useToast();
  const info = useContainerQuery<DockerInfo>(node.id, "info", online);
  const hasDocker = info.data?.status === "succeeded";
  const list = useContainerQuery<ContainerSummary[]>(node.id, "list", online && hasDocker);
  const act = useContainerAction(node.id);
  const [tab, setTab] = useState<"containers" | "stacks">("containers");
  const stacks = useStacks({ server: node.id }, online);
  const [q, setQ] = useState("");
  const [removing, setRemoving] = useState<ContainerSummary>();
  const [adopting, setAdopting] = useState<ContainerSummary>();
  const rm = useRowMenu();
  const navigate = useNavigate();
  const openContainer = (c: ContainerSummary, tab?: "logs" | "shell" | "environment") =>
    void navigate({ to: "/nodes/$nodeId/containers/$containerId", params: { nodeId: node.id, containerId: c.id }, search: tab ? { tab } : {} });
  const [stats, setStats] = useState<Record<string, { cpu_percent: number; memory_bytes: number; memory_limit: number }>>({});

  const rows = useMemo(
    () => (list.data?.result ?? []).filter((c) => `${c.name} ${c.image} ${c.state}`.toLowerCase().includes(q.toLowerCase())),
    [list.data, q],
  );

  const run = (op: "start" | "stop" | "restart" | "remove", c?: ContainerSummary) =>
    act.mutate(
      { op, id: c?.id },
      {
        onSuccess: (r) => {
          if (r.status === "failed") toast(r.error ?? `${op} failed`, "error");
          else toast(r.status === "running" ? `${op} started — follow it in Jobs` : `${op} ${c?.name ?? ""} done`);
          void list.refetch();
        },
        onError: (e) => toast(errorText(e), "error"),
      },
    );

  const loadStats = async (c: ContainerSummary) => {
    const r = await containerCall<{ cpu_percent: number; memory_bytes: number; memory_limit: number }>(node.id, { op: "stats", id: c.id });
    if (r.result) setStats((s) => ({ ...s, [c.id]: r.result! }));
    else toast(r.error ?? "stats unavailable", "error");
  };

  if (!online) {
    return (
      <Panel title="Containers">
        <p className="px-4 py-6 text-muted">The agent must be online to manage containers.</p>
      </Panel>
    );
  }
  if (info.isLoading) {
    return (
      <Panel title="Containers" description="Checking for Docker…">
        <Skeleton rows={3} />
      </Panel>
    );
  }
  if (info.data?.status === "running") {
    return (
      <Panel title="Containers" actions={<CreateActions node={node} />}>
        <div className="flex items-center justify-between gap-3 px-4 py-5">
          <p className="text-[13px] text-muted">The node is still answering the Docker check.</p>
          <Button onClick={() => void info.refetch()} disabled={info.isFetching}>
            <RefreshCw className={`size-4 ${info.isFetching ? "animate-spin" : ""}`} /> Check again
          </Button>
        </div>
      </Panel>
    );
  }
  if (!hasDocker) {
    const detail = info.data?.error ?? (info.error ? errorText(info.error) : undefined);
    return (
      <Panel title="Containers" actions={<CreateActions node={node} disabled="Install Docker Engine on this node first" />}>
        <div className="space-y-2 px-4 py-5">
          <div className="flex flex-wrap items-center justify-between gap-3">
            <p className="text-[13px]">
              Docker is not installed on this node; install Docker Engine from the{" "}
              <Link to="/app-store" search={{ node: node.id }} className="text-accent hover:underline">
                App Store
              </Link>
              .
            </p>
            <Link to="/app-store" search={{ node: node.id }}>
              <Button variant="primary">
                <Boxes className="size-4" /> Install Docker Engine
              </Button>
            </Link>
          </div>
          {detail && (
            <p className="truncate font-mono text-[11px] text-faint" title={detail}>
              {detail}
            </p>
          )}
        </div>
      </Panel>
    );
  }

  const d = info.data?.result;
  return (
    <Panel
      title="Containers"
      description={d ? `Docker ${d.version} · ${d.running}/${d.containers} running · ${d.images} images${d.swarm === "active" ? " · swarm manager" : ""}` : undefined}
      actions={
        <div className="flex items-center gap-1">
          <CreateActions node={node} />
          <div className="mx-2 flex rounded-md border border-line p-0.5 text-xs">
            {(["containers", "stacks"] as const).map((t) => (
              <button
                key={t}
                onClick={() => setTab(t)}
                className={`rounded px-2.5 py-1 capitalize ${tab === t ? "bg-accent-soft text-accent" : "text-muted"}`}
              >
                {t}
                {t === "stacks" && (stacks.data?.length ?? 0) > 0 && <span className="ml-1 font-mono text-[10px] tabular-nums">{stacks.data?.length}</span>}
              </button>
            ))}
          </div>
          <IconButton label="Refresh" onClick={() => void (tab === "stacks" ? stacks.refetch() : list.refetch())}>
            <RefreshCw className={`size-4 ${list.isFetching || stacks.isFetching ? "animate-spin" : ""}`} />
          </IconButton>
        </div>
      }
    >
      {tab === "stacks" ? (
        <>
          <ErrorNote error={stacks.error} />
          {stacks.isLoading ? (
            <Skeleton rows={3} />
          ) : (
            <StackTable
              rows={stacks.data ?? []}
              empty={
                <Empty icon={<Layers className="size-5" />} title="No stacks on this node">
                  Use <b>Create</b> → <b>Create from Compose</b> or <b>Create container</b>.
                </Empty>
              }
            />
          )}
        </>
      ) : (
        <>
          <div className="flex items-center gap-2 border-b border-line px-4 py-2">
            <Search className="size-4 text-faint" />
            <input data-bare className="min-w-0 flex-1 bg-transparent text-[13px] outline-none" placeholder="Filter containers" aria-label="Filter containers" value={q} onChange={(e) => setQ(e.target.value)} />
          </div>
          {list.data?.status === "failed" && <ErrorNote error={list.data.error ?? "Listing the containers failed"} />}
          <ErrorNote error={list.error} />
          {rows.length === 0 ? (
            list.isLoading || (list.isFetching && !list.data) ? (
              <Skeleton rows={3} />
            ) : (
              <Empty icon={<Container className="size-5" />} title={q ? "No container matches the filter" : "No containers"}>
                {q ? "Clear the filter to see every container." : (
                  <>
                    Use <b>Create</b> → <b>Create container</b> to run one.
                  </>
                )}
              </Empty>
            )
          ) : (
            <Table head={["Name", "Image", "State", "Ports", "CPU / memory", ""]}>
              {rows.map((c) => {
                const up = c.state === "running";
                const s = stats[c.id];
                const wait = act.isPending ? "Another container action is running" : undefined;
                const actions: RowAction[] = [
                  { label: "Open", icon: Container, onSelect: () => openContainer(c) },
                  { label: "Logs", icon: FileText, tone: "accent", onSelect: () => openContainer(c, "logs") },
                  { label: "Shell", icon: SquareTerminal, disabled: !up, hint: up ? undefined : "The container is not running", onSelect: () => openContainer(c, "shell") },
                  up
                    ? { label: "Stop", icon: Square, tone: "bad", disabled: act.isPending, hint: wait, onSelect: () => run("stop", c) }
                    : { label: "Start", icon: Play, tone: "ok", disabled: act.isPending, hint: wait, onSelect: () => run("start", c) },
                  { label: "Restart", icon: RotateCw, tone: "warn", disabled: act.isPending, hint: wait, onSelect: () => run("restart", c) },
                  { label: "Adopt as service", icon: Boxes, hidden: isForgeServiceContainer(c.name), onSelect: () => setAdopting(c) },
                  { label: "Remove", icon: Trash2, danger: true, disabled: act.isPending, hint: wait, onSelect: () => setRemoving(c) },
                ];
                return (
                  <tr key={c.id} className="hover:bg-panel-2/60" {...rm.rowProps(actions, `Actions for ${c.name}`)}>
                    <Td className="max-w-[260px]">
                      <Link
                        to="/nodes/$nodeId/containers/$containerId"
                        params={{ nodeId: node.id, containerId: c.id }}
                        className="block truncate font-medium hover:text-accent"
                        title={`Open ${c.name}`}
                      >
                        {c.name}
                      </Link>
                      <Mono>{c.id}</Mono>
                    </Td>
                    <Td className="max-w-[220px] truncate text-muted" >
                      <span title={c.image}>{c.image}</span>
                    </Td>
                    <Td>
                      <Status value={up ? "online" : c.state === "exited" ? "offline" : c.state} />
                      <div className="text-xs text-faint">{c.status}</div>
                    </Td>
                    <Td className="text-xs text-muted">{c.ports.length ? c.ports.join(", ") : "—"}</Td>
                    <Td className="text-xs text-muted">
                      {s ? (
                        `${s.cpu_percent}% · ${fmtBytes(s.memory_bytes)} / ${fmtBytes(s.memory_limit)}`
                      ) : (
                        <button className="hover:text-accent disabled:opacity-40" disabled={!up} title={up ? "Sample CPU and memory once" : "The container is not running"} onClick={() => void loadStats(c)}>
                          <Activity className="inline size-3.5" /> sample
                        </button>
                      )}
                    </Td>
                    <Td className="text-right">
                      <RowMenu actions={actions} label={`Actions for ${c.name}`} />
                    </Td>
                  </tr>
                );
              })}
            </Table>
          )}
        </>
      )}

      {rm.menu}
      {adopting && <AdoptContainerDialog serverId={node.id} container={adopting.name} onClose={() => setAdopting(undefined)} />}
      <Confirm
        open={!!removing}
        onOpenChange={(o) => !o && setRemoving(undefined)}
        title={`Remove container ${removing?.name}?`}
        body="The container is stopped and deleted. Named volumes and bind-mounted data are kept."
        confirmLabel="Remove"
        onConfirm={() => {
          if (removing) run("remove", removing);
          setRemoving(undefined);
        }}
      />
    </Panel>
  );
}

/** Create container / Create from Compose for this node; disabled, with the reason, without Docker. */
function CreateActions({ node, disabled }: { node: Server; disabled?: string }) {
  const navigate = useNavigate();
  const item = "flex cursor-pointer items-start gap-2 rounded px-2.5 py-1.5 text-[13px] outline-none select-none data-[highlighted]:bg-panel-2";
  return (
    <>
      <DropdownMenu.Root modal={false}>
        <span title={disabled} className="inline-flex">
          <DropdownMenu.Trigger asChild disabled={!!disabled}>
            <Button disabled={!!disabled} aria-label={disabled ? `Create (${disabled})` : "Create"}>
              <Plus className="size-4" /> Create <ChevronDown className="size-3.5 opacity-70" />
            </Button>
          </DropdownMenu.Trigger>
        </span>
        <DropdownMenu.Portal>
          <DropdownMenu.Content align="end" sideOffset={4} className="animate-in z-50 min-w-[220px] rounded-lg border border-line bg-panel p-1 shadow-[var(--shadow-pop)]">
            <DropdownMenu.Item onSelect={() => void navigate({ to: "/stacks/new-container", search: { node: node.id, environment: node.environment_id ?? undefined } })} className={item}>
              <Container className="mt-0.5 size-4 shrink-0 opacity-80" />
              <span>
                Create container
                <span className="block text-[11px] text-muted">One image with ports, env and volumes</span>
              </span>
            </DropdownMenu.Item>
            <DropdownMenu.Item onSelect={() => void navigate({ to: "/stacks/new", search: { node: node.id, environment: node.environment_id ?? undefined } })} className={item}>
              <FileCode2 className="mt-0.5 size-4 shrink-0 opacity-80" />
              <span>
                Create from Compose
                <span className="block text-[11px] text-muted">Paste or upload a compose file</span>
              </span>
            </DropdownMenu.Item>
          </DropdownMenu.Content>
        </DropdownMenu.Portal>
      </DropdownMenu.Root>
    </>
  );
}

/** Container logs in a modal: stored history, or live with the Live toggle (the container page's Logs tab shows the same). */
export function LogsModal({ server, container, onClose }: { server: string; container: ContainerSummary; onClose: () => void }) {
  return (
    <Modal open onOpenChange={(o) => !o && onClose()} title={`Logs · ${container.name}`} description={container.image} wide>
      <StoredContainerLogs server={server} name={container.name} containerId={container.id} />
    </Modal>
  );
}
