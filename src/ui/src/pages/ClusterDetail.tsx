import * as Tabs from "@radix-ui/react-tabs";
import { Link, useNavigate, useParams, useSearch } from "@tanstack/react-router";
import { IpTags, useClusterFabric } from "../components/IpTag";
import { nodeName } from "../api/types";
import { Container, Gauge, Network, Pencil, Plus, Server, UserMinus } from "lucide-react";
import { useState } from "react";

import { useServices, useAssignServer, useClusters, useOnboardingNodes, useServers } from "../api/hooks";
import type { ClusterTab } from "../app/router";
import { ClusterContainersTab } from "../components/cluster/ClusterContainersTab";
import { ClusterNetworkTab } from "../components/cluster/ClusterNetworkTab";
import { errorText, useToast } from "../app/toast";
import { emptyNodeFilter, NodeList, type NodeFilter } from "../components/NodeList";
import { useLatestMetrics } from "../components/NodeTable";
import { fmtBytes } from "../components/charts/TimeSeries";
import { PinToggle } from "../components/Pins";
import { Button, Empty, ErrorNote, Modal, PageHeader, Select, Skeleton, Stat, tabTrigger } from "../components/ui";
import { TagEditor } from "../components/TagEditor";
import { ClusterForm } from "./Clusters";

export function ClusterDetail() {
  const { clusterId } = useParams({ strict: false }) as { clusterId: string };
  const navigate = useNavigate();
  const { tab = "overview" } = useSearch({ from: "/clusters/$clusterId" });
  const setTab = (t: string) =>
    void navigate({ to: "/clusters/$clusterId", params: { clusterId }, search: { tab: t === "overview" ? undefined : (t as ClusterTab) }, replace: true });
  const services = useServices(undefined, clusterId);
  const metrics = useLatestMetrics().data ?? {};
  const clustersQ = useClusters();
  const clusters = clustersQ.data ?? [];
  const allNodes = useServers().data ?? [];
  const joining = (useOnboardingNodes().data ?? []).filter((o) => o.cluster_id === clusterId);
  const assign = useAssignServer();
  const toast = useToast();
  const [editing, setEditing] = useState(false);
  const [adding, setAdding] = useState("");
  const [filter, setFilter] = useState<NodeFilter>(() => emptyNodeFilter());

  const cluster = clusters.find((c) => c.id === clusterId);
  const fabric = useClusterFabric(clusterId).data;
  // The control-plane node takes part in every cluster: list it with the members.
  const nodes = allNodes.filter(
    (n) =>
      n.cluster_id === clusterId ||
      (n.role === "control" && n.status !== "revoked"),
  );
  const workers = nodes.filter((n) => n.role !== "control");
  const unassigned = allNodes.filter(
    (n) => !n.cluster_id && n.role !== "control" && n.status !== "revoked",
  );
  const live = nodes
    .map((n) => (n.status === "online" ? metrics[n.id] : undefined))
    .filter((m) => m !== undefined);
  const avgCpu = live.length
    ? live.reduce((a, m) => a + m.cpu, 0) / live.length
    : undefined;
  const memUsed = live.reduce((a, m) => a + m.mem_used, 0);
  const memTotal = live.reduce((a, m) => a + m.mem_total, 0);

  if (clustersQ.error) return <ErrorNote error={clustersQ.error} />;
  if (clustersQ.isLoading) return <Skeleton rows={6} />;
  if (!cluster)
    return (
      <Empty icon={<Server className="size-5" />} title="Cluster not found" action={<Link to="/clusters" className="text-accent hover:underline">All clusters</Link>}>
        It may have been deleted.
      </Empty>
    );
  const online = nodes.filter((n) => n.status === "online").length;
  const offline = nodes.filter((n) => n.status === "offline").length;
  const serviceCount = services.data?.length;

  const move = (
    id: string,
    to: string | null,
    env: string | null,
    msg: string,
  ) =>
    assign.mutate(
      { id, cluster_id: to, environment_id: env },
      {
        onSuccess: () => toast(msg),
        onError: (e) => toast(errorText(e), "error"),
      },
    );

  return (
    <>
      <PageHeader
        eyebrow={<Link to="/clusters" className="hover:text-accent">Clusters</Link>}
        title={cluster.name}
        description={cluster.description || "No description"}
        actions={
          <>
            <PinToggle kind="cluster" id={cluster.id} label={cluster.name} />
            <Button onClick={() => setEditing(true)}>
              <Pencil className="size-4" /> Edit
            </Button>
            <Button
              variant="primary"
              onClick={() =>
                void navigate({ href: `/nodes/add?cluster=${cluster.id}` })
              }
            >
              <Plus className="size-4" /> Add new node
            </Button>
          </>
        }
      />
      <div className="-mt-1 mb-3 flex min-w-0 flex-wrap items-center gap-x-4 gap-y-1">
        <TagEditor kind="cluster" id={clusterId} />
        <IpTags
          tags={[
            { label: "Subnet", value: fabric?.spec?.subnet },
            { label: "Domain", value: fabric?.spec?.domain },
            { label: "Mode", value: fabric?.spec ? "encrypted mesh" : undefined },
          ]}
        />
      </div>
      <Tabs.Root value={tab} onValueChange={setTab}>
        <Tabs.List className="mb-4 flex gap-1 overflow-x-auto border-b border-line" aria-label="Cluster sections">
          <Tabs.Trigger value="overview" className={tabTrigger}>
            <Gauge className="size-4" /> Overview
          </Tabs.Trigger>
          <Tabs.Trigger value="network" className={tabTrigger}>
            <Network className="size-4" /> Network
          </Tabs.Trigger>
          <Tabs.Trigger value="containers" className={tabTrigger}>
            <Container className="size-4" /> Containers
          </Tabs.Trigger>
        </Tabs.List>
        <Tabs.Content value="network">
          <ClusterNetworkTab clusterId={clusterId} />
        </Tabs.Content>
        <Tabs.Content value="containers">
          <ClusterContainersTab clusterId={clusterId} />
        </Tabs.Content>
        <Tabs.Content value="overview">
      <div className="mb-4 grid grid-cols-2 gap-3 md:grid-cols-3 xl:grid-cols-5">
        <Stat
          label="Nodes"
          value={`${online} / ${nodes.length}`}
          hint={`online · 1 control plane · ${workers.length} worker${workers.length === 1 ? "" : "s"}${offline ? ` · ${offline} disconnected` : ""}`}
          tone={nodes.length && online === nodes.length ? "ok" : "warn"}
          onClick={() => void navigate({ to: "/nodes", search: { cluster: clusterId } })}
        />
        <Stat
          label="Services"
          value={serviceCount ?? "—"}
          hint="in this cluster's environments"
          onClick={() => void navigate({ to: "/services", search: { cluster: clusterId } })}
        />
        <Stat
          label="Ingress"
          value="Open →"
          hint="edges, routes and certificates"
          onClick={() => void navigate({ to: "/ingress", search: { cluster: clusterId } })}
        />
        <Stat
          label="Average CPU"
          value={avgCpu === undefined ? "—" : `${avgCpu.toFixed(0)}%`}
          hint="across online nodes"
        />
        <Stat
          label="Memory in use"
          value={memTotal ? `${((memUsed / memTotal) * 100).toFixed(0)}%` : "—"}
          hint={
            memTotal
              ? `${fmtBytes(memUsed)} of ${fmtBytes(memTotal)}`
              : undefined
          }
        />
      </div>
      <NodeList
        nodes={nodes}
        clusters={clusters}
        showCluster={false}
        filter={filter}
        onFilterChange={(p) => setFilter((f) => ({ ...f, ...p }))}
        onboarding={joining}
        title={(c, v) => `Members · ${c} ${v}node${c === 1 ? "" : "s"}`}
        headerActions={
          unassigned.length > 0 && (
            <form
              className="flex gap-2"
              onSubmit={(e) => {
                e.preventDefault();
                const n = unassigned.find((x) => x.id === adding);
                if (n)
                  move(
                    n.id,
                    cluster.id,
                    n.environment_id ?? cluster.environment_id,
                    `${nodeName(n)} added to ${cluster.name}`,
                  );
                setAdding("");
              }}
            >
              <Select
                className="w-auto py-1.5"
                value={adding}
                onChange={(e) => setAdding(e.target.value)}
                aria-label="Existing node"
              >
                <option value="">Add an existing node…</option>
                {unassigned.map((n) => (
                  <option key={n.id} value={n.id}>
                    {nodeName(n)}
                  </option>
                ))}
              </Select>
              <Button disabled={!adding || assign.isPending}>Add</Button>
            </form>
          )
        }
        empty={
          <Empty
            icon={<Server className="size-5" />}
            title="No nodes in this cluster"
          >
            Add an existing node above, or{" "}
            <Link to="/nodes/add" className="text-accent">
              onboard a new one
            </Link>
            .
          </Empty>
        }
        extra={(n) => [
          {
            label: "Remove from cluster",
            icon: UserMinus,
            danger: true,
            hidden: n.cluster_id !== cluster.id,
            onSelect: () =>
              move(
                n.id,
                null,
                n.environment_id,
                `${nodeName(n)} removed from ${cluster.name}`,
              ),
          },
        ]}
      />
        </Tabs.Content>
      </Tabs.Root>
      <Modal open={editing} onOpenChange={setEditing} title="Edit cluster">
        <ClusterForm initial={cluster} onDone={() => setEditing(false)} />
      </Modal>
    </>
  );
}
