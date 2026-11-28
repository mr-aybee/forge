import { useQueries } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { Container, RefreshCw } from "lucide-react";
import { useMemo, useState } from "react";

import { type ContainerResult, type ContainerSummary, type DockerInfo, containerCall, useServers } from "../../api/hooks";
import { nodeName, type Server } from "../../api/types";
import { useOwnerResolver } from "../containers/owner";
import { OwnerLink } from "../containers/OwnerLink";
import { StateChip } from "../stacks/parts";
import { Badge, Empty, ErrorNote, IconButton, SectionBar, Panel, SearchInput, Skeleton, Table, Td } from "../ui";

type Filter = "all" | "running" | "stopped";

/**
 * Every Docker container on the cluster's nodes, in one table. Each online
 * node is asked for `info` (has Docker?) and then `list`; nodes without Docker
 * or offline are summarized, never an error.
 */
export function ClusterContainersTab({ clusterId }: { clusterId: string }) {
  const all = useServers();
  const nodes = useMemo(
    () => (all.data ?? []).filter((n) => n.cluster_id === clusterId || (n.role === "control" && n.status !== "revoked")),
    [all.data, clusterId],
  );
  const online = nodes.filter((n) => n.status === "online");
  const [q, setQ] = useState("");
  const [filter, setFilter] = useState<Filter>("all");
  const owner = useOwnerResolver();

  const infos = useQueries({
    queries: online.map((n) => ({
      queryKey: ["containers", n.id, "info"],
      queryFn: () => containerCall<DockerInfo>(n.id, { op: "info" }),
      retry: false,
      staleTime: 60_000,
    })),
  });
  const docker = online.filter((_, i) => infos[i]?.data?.status === "succeeded");
  const lists = useQueries({
    queries: docker.map((n) => ({
      queryKey: ["containers", n.id, "list"],
      queryFn: () => containerCall<ContainerSummary[]>(n.id, { op: "list" }),
      retry: false,
      staleTime: 10_000,
      refetchInterval: 20_000,
    })),
  });

  const rows = useMemo(() => {
    const out: { node: Server; c: ContainerSummary }[] = [];
    docker.forEach((n, i) => {
      const r = lists[i]?.data as ContainerResult<ContainerSummary[]> | undefined;
      for (const c of Array.isArray(r?.result) ? r.result : []) out.push({ node: n, c });
    });
    const needle = q.trim().toLowerCase();
    return out
      .filter(({ c }) => filter === "all" || (filter === "running") === (c.state === "running"))
      .filter(({ node, c }) => !needle || `${c.name} ${c.image} ${c.state} ${nodeName(node)}`.toLowerCase().includes(needle))
      .sort((a, b) => nodeName(a.node).localeCompare(nodeName(b.node)) || a.c.name.localeCompare(b.c.name));
  }, [docker, lists, q, filter]);

  const loading = all.isLoading || infos.some((i) => i.isLoading) || lists.some((l) => l.isLoading);
  const fetching = infos.some((i) => i.isFetching) || lists.some((l) => l.isFetching);
  const noDocker = online.filter((_, i) => infos[i]?.data && infos[i].data?.status !== "succeeded");
  const offline = nodes.filter((n) => n.status !== "online");
  const errors = lists.flatMap((l, i) => {
    const node = docker[i];
    const e = l.error ?? (l.data?.status === "failed" ? l.data.error : undefined);
    return node && e ? [{ node, e }] : [];
  });
  const running = rows.filter((r) => r.c.state === "running").length;
  // One section per node with Docker. While filtering, nodes without a match are left out.
  const filtering = q.trim() !== "" || filter !== "all";
  const groups = docker
    .map((node) => ({ node, items: rows.filter((r) => r.node.id === node.id) }))
    .filter((g) => !filtering || g.items.length > 0)
    .sort((a, b) => nodeName(a.node).localeCompare(nodeName(b.node)));

  const refresh = () => {
    for (const i of infos) void i.refetch();
    for (const l of lists) void l.refetch();
  };

  return (
    <>
      <SectionBar
        description={`Docker containers on every node of this cluster · ${docker.length} of ${nodes.length} node${nodes.length === 1 ? "" : "s"} with Docker${offline.length ? ` · ${offline.length} offline` : ""}`}
        actions={
          <IconButton label="Refresh" onClick={refresh}>
            <RefreshCw className={`size-4 ${fetching ? "animate-spin" : ""}`} />
          </IconButton>
        }
      />
      <Panel
        title={loading && rows.length === 0 ? "Containers" : `${rows.length} container${rows.length === 1 ? "" : "s"} · ${running} running`}
        actions={
          <div className="flex flex-wrap items-center gap-2">
            <div role="tablist" aria-label="State" className="flex rounded-md border border-line p-0.5 text-xs">
              {(["all", "running", "stopped"] as const).map((f) => (
                <button key={f} role="tab" aria-selected={filter === f} onClick={() => setFilter(f)} className={`rounded px-2.5 py-1 capitalize ${filter === f ? "bg-accent-soft text-accent" : "text-muted"}`}>
                  {f}
                </button>
              ))}
            </div>
            <SearchInput value={q} onChange={setQ} placeholder="Filter by name, image, node" />
          </div>
        }
      >
        {errors.map(({ node, e }) => (
          <ErrorNote key={node.id} error={`${nodeName(node)}: ${typeof e === "string" ? e : (e as Error)?.message ?? "listing failed"}`} />
        ))}
        {rows.length === 0 ? (
          loading ? (
            <Skeleton rows={4} />
          ) : (
            <Empty icon={<Container className="size-5" />} title={q || filter !== "all" ? "No container matches" : docker.length === 0 ? "No node in this cluster has Docker" : "No containers"}>
              {q || filter !== "all" ? (
                "Clear the filter to see every container."
              ) : docker.length === 0 ? (
                <>
                  Install <b>Docker Engine</b> from the <Link to="/app-store" className="text-accent hover:underline">App Store</Link> on a node of this cluster.
                </>
              ) : (
                <>
                  Create one from a node's <b>Containers</b> tab, or deploy a <Link to="/stacks" className="text-accent hover:underline">stack</Link>.
                </>
              )}
            </Empty>
          )
        ) : (
          <Table head={["Name", "Image", "State", "Ports", "Service"]}>
            {groups.map(({ node, items }) => (
              <NodeSection key={node.id} node={node} items={items.map((r) => r.c)} owner={owner} />
            ))}
          </Table>
        )}
        {(noDocker.length > 0 || offline.length > 0) && (
          <div className="flex flex-wrap items-center gap-x-3 gap-y-1 border-t border-line px-4 py-2 text-[11px] text-faint">
            {noDocker.length > 0 && (
              <span>
                No Docker:{" "}
                {noDocker.map((n) => (
                  <Link key={n.id} to="/nodes/$nodeId" params={{ nodeId: n.id }} className="mr-1 hover:text-accent">
                    {nodeName(n)}
                  </Link>
                ))}
              </span>
            )}
            {offline.length > 0 && (
              <span>
                Offline:{" "}
                {offline.map((n) => (
                  <Link key={n.id} to="/nodes/$nodeId" params={{ nodeId: n.id }} className="mr-1 hover:text-accent">
                    {nodeName(n)} <Badge>{n.status}</Badge>
                  </Link>
                ))}
              </span>
            )}
          </div>
        )}
      </Panel>
    </>
  );
}

/** A node's header row and its containers, inside the cluster table. */
function NodeSection({ node, items, owner }: { node: Server; items: ContainerSummary[]; owner: ReturnType<typeof useOwnerResolver> }) {
  const up = items.filter((c) => c.state === "running").length;
  return (
    <>
      <tr className="bg-panel-2/70">
        <td colSpan={5} className="px-4 py-1.5">
          <div className="flex items-center gap-2 text-xs">
            <Link to="/nodes/$nodeId" params={{ nodeId: node.id }} search={{ tab: "containers" }} className="font-semibold hover:text-accent" title={`Open ${nodeName(node)}`}>
              {nodeName(node)}
            </Link>
            {node.display_name && node.display_name !== node.hostname && <span className="font-mono text-[11px] text-faint">{node.hostname}</span>}
            <span className="ml-auto text-[11px] text-muted tabular-nums">
              {items.length ? `${up} of ${items.length} running` : "No containers"}
            </span>
          </div>
        </td>
      </tr>
      {items.map((c) => (
        <tr key={`${node.id}-${c.id}`} className="hover:bg-panel-2/60">
          <Td className="max-w-[260px]">
            <Link to="/nodes/$nodeId/containers/$containerId" params={{ nodeId: node.id, containerId: c.id }} className="block truncate font-medium hover:text-accent" title={c.name}>
              {c.name}
            </Link>
            <span className="font-mono text-[11px] text-faint">{c.id.slice(0, 12)}</span>
          </Td>
          <Td className="max-w-[220px]">
            <span className="block truncate font-mono text-[11px] text-muted" title={c.image}>
              {c.image}
            </span>
          </Td>
          <Td>
            <StateChip value={c.state} />
            <div className="max-w-[160px] truncate text-[11px] text-faint" title={c.status}>
              {c.status}
            </div>
          </Td>
          <Td className="max-w-[200px]">
            <span className="block truncate font-mono text-[11px] tabular-nums" title={(c.ports ?? []).join(", ")}>
              {c.ports?.length ? c.ports.join(", ") : <span className="text-faint">—</span>}
            </span>
          </Td>
          <Td className="max-w-[220px]">
            <OwnerLink owner={owner(c.name, c.labels)} />
          </Td>
        </tr>
      ))}
    </>
  );
}
