import { useNavigate, useSearch } from "@tanstack/react-router";
import { Network, Plus, Server } from "lucide-react";

import { useClusters, useOnboardingNodes, useServers } from "../api/hooks";
import type { NodesSearch } from "../app/router";
import { csv, toCsv } from "../lib/search";
import { NodeList, type NodeFilter } from "../components/NodeList";
import { Button, Empty, ErrorNote, PageHeader, VIEW_LIST, ViewToggle, groupedView, usePageView } from "../components/ui";

const VIEWS = [VIEW_LIST, groupedView("cluster", "By cluster", Network)] as const;

/** Every node; filters and layout live in the URL (`/nodes?cluster=&role=&os=&tag=&status=&q=&view=`). */
export function Nodes() {
  const navigate = useNavigate();
  const search = useSearch({ from: "/nodes" });
  const patch = (p: Partial<NodesSearch>) => void navigate({ from: "/nodes", to: "/nodes", search: (s) => ({ ...s, ...p }), replace: true });
  const [view, setView] = usePageView("forge.nodes.view", VIEWS, search.view, (v) => patch({ view: v }));
  const filter: NodeFilter = { cluster: search.cluster, role: search.role, os: search.os, tags: csv(search.tag), status: search.status ?? "active", q: search.q ?? "" };
  const onFilterChange = (p: Partial<NodeFilter>) => {
    const { tags, q, status, ...rest } = p;
    patch({
      ...rest,
      ...(tags !== undefined && { tag: toCsv(tags) }),
      ...(q !== undefined && { q: q || undefined }),
      ...(status !== undefined && { status: status === "active" ? undefined : status }),
    });
  };
  const nodes = useServers();
  const clusters = useClusters().data ?? [];
  const onboarding = useOnboardingNodes().data ?? [];

  return (
    <>
      <PageHeader
        eyebrow="Infrastructure"
        title="Nodes"
        description="Machines running the Forge agent, including the control-plane node that hosts Forge itself."
        actions={
          <Button variant="primary" onClick={() => void navigate({ to: "/nodes/add" })}>
            <Plus className="size-4" /> Add node
          </Button>
        }
      />
      <ErrorNote error={nodes.error} />
      <NodeList
        nodes={nodes.data ?? []}
        clusters={clusters}
        loading={nodes.isLoading}
        onboarding={onboarding}
        filter={filter}
        onFilterChange={onFilterChange}
        grouped={view === "cluster"}
        viewToggle={<ViewToggle view={view} onChange={setView} modes={VIEWS} />}
        empty={
          nodes.error ? (
            <ErrorNote error={nodes.error} />
          ) : (
            <Empty
              icon={<Server className="size-5" />}
              title="No nodes yet"
              action={
                <Button variant="primary" onClick={() => void navigate({ to: "/nodes/add" })}>
                  Add a node
                </Button>
              }
            >
              Give Forge SSH access to a VM, or run the one-line installer on it.
            </Empty>
          )
        }
      />
    </>
  );
}
