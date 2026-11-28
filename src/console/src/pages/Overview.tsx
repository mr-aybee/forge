import { Link, useNavigate } from "@tanstack/react-router";
import { nodeName } from "../api/types";
import { AlertTriangle, ArrowLeftRight, Boxes, CircleCheck, KeyRound, Plus, Server, ServerCog, ShieldCheck, Sparkles, Info, Radio, SquareTerminal, Workflow } from "lucide-react";
import { useMemo, useState } from "react";

import { useClusters, useJobs, useMe, useSecrets, useServerCommand, useServers } from "../api/hooks";
import type { Cluster, Server as Node } from "../api/types";
import { useTerminals } from "../app/terminals";
import { errorText, useToast } from "../app/toast";
import { ClusterWizard } from "../components/setup/ClusterWizard";
import { NodeTrafficCell, bytesRate, useTraffic } from "../components/Traffic";
import { Button, RowMenu, type RowAction, Empty, Mono, PageHeader, Panel, Stat, Status, Table, Td, ago, useRowMenu } from "../components/ui";

const DAY = 86_400_000;

function ClusterMap({ clusters, nodes }: { clusters: Cluster[]; nodes: Node[] }) {
  const groups = [
    ...clusters.map((c) => ({ id: c.id, name: c.name, nodes: nodes.filter((n) => n.cluster_id === c.id) })),
    { id: "", name: "Unassigned", nodes: nodes.filter((n) => !n.cluster_id) },
  ].filter((g) => g.id || g.nodes.length);
  if (groups.length === 0) return <Empty icon={<ServerCog className="size-5" />} title="No clusters or nodes yet">Create a cluster, then add nodes to it.</Empty>;
  return (
    <div className="grid gap-3 p-4 sm:grid-cols-2 xl:grid-cols-3">
      {groups.map((g) => {
        const online = g.nodes.filter((n) => n.status === "online").length;
        return (
          <div key={g.id || "none"} className="rounded-lg border border-line bg-panel-2/50 p-3">
            <div className="mb-2 flex items-center justify-between">
              {g.id ? (
                <Link to="/clusters/$clusterId" params={{ clusterId: g.id }} className="font-medium hover:text-accent">
                  {g.name}
                </Link>
              ) : (
                <span className="font-medium text-muted">{g.name}</span>
              )}
              <span className="text-xs text-faint">
                {online}/{g.nodes.length} online
              </span>
            </div>
            <div className="flex flex-wrap gap-1.5">
              {g.nodes.map((n) => (
                <Link
                  key={n.id}
                  to="/nodes/$nodeId"
                  params={{ nodeId: n.id }}
                  title={`${nodeName(n)} · ${n.status}`}
                  className={`flex items-center gap-1 rounded border px-1.5 py-0.5 text-xs hover:border-accent ${
                    n.status === "online" ? "border-ok/30 bg-ok-soft text-ok" : n.status === "offline" ? "border-warn/30 bg-warn-soft text-warn" : "border-line text-muted"
                  }`}
                >
                  <Server className="size-3" />
                  {nodeName(n)}
                </Link>
              ))}
              {g.nodes.length === 0 && <span className="text-xs text-faint">No nodes</span>}
            </div>
          </div>
        );
      })}
    </div>
  );
}

export function Overview() {
  const traffic = useTraffic().data;
  const navigate = useNavigate();
  const toast = useToast();
  const me = useMe();
  const allNodes = useServers().data ?? [];
  const nodes = allNodes;
  const clusters = useClusters().data ?? [];
  const jobs = useJobs().data ?? [];
  const secrets = useSecrets().data ?? [];
  const ping = useServerCommand();
  const terminals = useTerminals();
  const [wizard, setWizard] = useState(false);
  const rm = useRowMenu();
  const nodeActions = (n: Node): RowAction[] => [
    { label: "Shell", icon: SquareTerminal, disabled: n.status === "revoked", hint: "The node's agent is revoked", onSelect: () => terminals.open(n) },
    {
      label: "Ping",
      icon: Radio,
      disabled: n.status !== "online" || ping.isPending,
      hint: n.status !== "online" ? "The node is not online" : "A ping is on its way",
      onSelect: () => ping.mutate({ server: n.id, command: { type: "ping" } }, { onSuccess: () => toast(`Ping sent to ${nodeName(n)}`), onError: (e) => toast(errorText(e), "error") }),
    },
    { label: "Details", icon: Info, onSelect: () => void navigate({ to: "/nodes/$nodeId", params: { nodeId: n.id } }) },
  ];

  const online = nodes.filter((n) => n.status === "online").length;
  const active = jobs.filter((j) => j.status === "running" || j.status === "queued").length;
  const failed24 = jobs.filter((j) => j.status === "failed" && j.updated_at > Date.now() - DAY);
  const succeeded24 = jobs.filter((j) => j.status === "succeeded" && j.updated_at > Date.now() - DAY).length;
  const expiringCerts = nodes.filter((n) => n.cert_not_after && n.cert_not_after < Date.now() + 6 * 3600_000);
  const kept = secrets.filter((s) => s.expires_at === null);

  const attention = useMemo(
    () => [
      ...nodes.filter((n) => n.status === "offline").map((n) => ({ key: n.id, sev: "warn" as const, title: `${nodeName(n)} is offline`, body: `Last heartbeat ${ago(n.last_seen_at)}. Check the host and the agent service.`, to: `/nodes/${n.id}` })),
      ...failed24.slice(0, 4).map((j) => ({ key: j.id, sev: "bad" as const, title: `${j.kind} failed`, body: j.last_error ?? "See job steps for details.", to: "/jobs" })),
      ...(me.data && !me.data.mfa_enrolled
        ? [{ key: "mfa", sev: "warn" as const, title: "Enable MFA", body: "Onboarding nodes and other sensitive actions require an MFA-verified session.", to: "/profile/security" }]
        : []),
      ...kept.map((s) => ({ key: s.id, sev: "warn" as const, title: `Credential kept for ${s.label}`, body: "Stored for repairs. Delete it when no longer needed.", to: "/vault" })),
    ],
    [nodes, failed24, me.data, kept],
  );

  return (
    <>
      <PageHeader
        title="Overview"
        description="Health of your clusters and nodes, what's running now, and what needs attention."
        actions={
          <>
            <Button onClick={() => setWizard(true)}>
              <ServerCog className="size-4" /> Set up a cluster
            </Button>
            <Button variant="primary" onClick={() => void navigate({ to: "/nodes/add" })}>
              <Plus className="size-4" /> Add node
            </Button>
          </>
        }
      />

      {clusters.length === 0 && allNodes.length > 0 && (
        <div className="mb-5 flex flex-wrap items-center gap-4 rounded-lg border border-accent/30 bg-accent-soft p-4">
          <Sparkles className="size-6 text-accent" />
          <div className="flex-1">
            <div className="font-semibold">Set up your first cluster</div>
            <p className="text-[13px] text-muted">Pick the members, the cluster network and how Nginx takes traffic in (single edge, active-standby or active-active).</p>
          </div>
          <Button variant="primary" onClick={() => setWizard(true)}>
            Start setup
          </Button>
        </div>
      )}
      <ClusterWizard open={wizard} onOpenChange={setWizard} onDone={() => void navigate({ to: "/ingress" })} />

      <div className="mb-5 grid grid-cols-2 gap-3 md:grid-cols-4 xl:grid-cols-7">
        <Stat label="Nodes online" value={<>{online}<span className="text-sm text-faint"> / {nodes.length}</span></>} hint="Agents connected over mTLS" icon={<Server className="size-4" />} tone={online === nodes.length ? "ok" : "warn"} onClick={() => void navigate({ to: "/nodes" })} />
        <Stat label="Clusters" value={clusters.length} hint={`${allNodes.filter((n) => !n.cluster_id).length} nodes unassigned`} icon={<ServerCog className="size-4" />} onClick={() => void navigate({ to: "/clusters" })} />
        <Stat
          label="Route traffic"
          value={bytesRate((traffic?.total.in_bps ?? 0) + (traffic?.total.out_bps ?? 0))}
          hint={traffic ? `${Object.keys(traffic.nodes).length} node${Object.keys(traffic.nodes).length === 1 ? "" : "s"} receiving · ${(traffic.total.requests / (traffic.window_secs || 60)).toFixed(1)} req/s` : "Through the Nginx edges, last minute"}
          icon={<ArrowLeftRight className="size-4" />}
          onClick={() => void navigate({ to: "/nodes" })}
        />
        <Stat label="Jobs running" value={active} hint="Durable, resumable" icon={<Workflow className="size-4" />} onClick={() => void navigate({ to: "/jobs" })} />
        <Stat label="Succeeded (24h)" value={succeeded24} hint="Completed operations" icon={<CircleCheck className="size-4" />} tone="ok" onClick={() => void navigate({ to: "/jobs" })} />
        <Stat label="Failed (24h)" value={failed24.length} hint="Inspect steps to fix" icon={<AlertTriangle className="size-4" />} tone={failed24.length ? "bad" : undefined} onClick={() => void navigate({ to: "/jobs" })} />
        <Stat label="Needs attention" value={attention.length} hint={attention.length ? "See list →" : "All clear"} icon={<ShieldCheck className="size-4" />} tone={attention.length ? "warn" : "ok"} />
      </div>

      <div className="grid gap-4 xl:grid-cols-12">
        <Panel
          className="xl:col-span-8"
          title="Cluster topology"
          description="Nodes grouped by cluster. Click a cluster or node for details."
          actions={<Link to="/clusters" className="text-xs text-accent">All clusters →</Link>}
        >
          <ClusterMap clusters={clusters} nodes={nodes} />
        </Panel>

        <Panel className="xl:col-span-4" title="Needs attention" description="Prioritised by impact">
          {attention.length === 0 ? (
            <Empty icon={<CircleCheck className="size-5" />} title="Nothing needs attention" />
          ) : (
            <ul className="divide-y divide-line">
              {attention.slice(0, 8).map((a) => (
                <li key={a.key}>
                  <Link to={a.to} className="flex gap-3 px-4 py-3 hover:bg-panel-2">
                    {a.sev === "bad" ? <AlertTriangle className="mt-0.5 size-4 shrink-0 text-bad" /> : a.key === "mfa" || a.title.startsWith("Credential") ? <KeyRound className="mt-0.5 size-4 shrink-0 text-warn" /> : <AlertTriangle className="mt-0.5 size-4 shrink-0 text-warn" />}
                    <span className="min-w-0">
                      <span className="block font-medium">{a.title}</span>
                      <span className="line-clamp-2 text-xs text-muted">{a.body}</span>
                    </span>
                  </Link>
                </li>
              ))}
            </ul>
          )}
        </Panel>

        <Panel
          className="xl:col-span-7"
          title="Managed fleet"
          description="Windows and Linux nodes with outbound agents"
          actions={<Link to="/nodes" className="text-xs text-accent">Full fleet →</Link>}
        >
          {nodes.length === 0 ? (
            <Empty icon={<Server className="size-5" />} title="No nodes yet" action={<Button variant="primary" onClick={() => void navigate({ to: "/nodes/add" })}>Add your first node</Button>}>
              Give Forge SSH access to a VM, or run the one-line installer on it.
            </Empty>
          ) : (
            <Table head={["Node", "OS", "Status", "Traffic", "Last seen", ""]}>
              {nodes.slice(0, 8).map((n) => (
                <tr key={n.id} className="hover:bg-panel-2/60" {...rm.rowProps(nodeActions(n), `Actions for ${nodeName(n)}`)}>
                  <Td>
                    <Link to="/nodes/$nodeId" params={{ nodeId: n.id }} className="font-medium hover:text-accent">
                      {nodeName(n)}
                    </Link>
                  </Td>
                  <Td className="text-muted">{n.os}/{n.arch}</Td>
                  <Td><Status value={n.status} /></Td>
                  <Td><NodeTrafficCell flow={traffic?.nodes?.[n.id]} /></Td>
                  <Td className="text-muted">{ago(n.last_seen_at)}</Td>
                  <Td className="text-right">
                    <RowMenu actions={nodeActions(n)} label={`Actions for ${nodeName(n)}`} />
                  </Td>
                </tr>
              ))}
            </Table>
          )}
          {rm.menu}
        </Panel>

        <Panel
          className="xl:col-span-5"
          title="Recent operations"
          description="Every job is durable and audited"
          actions={<Link to="/jobs" className="text-xs text-accent">All jobs →</Link>}
        >
          {jobs.length === 0 ? (
            <Empty icon={<Boxes className="size-5" />} title="No operations yet" />
          ) : (
            <ul className="divide-y divide-line">
              {jobs.slice(0, 7).map((j) => (
                <li key={j.id} className="flex items-center justify-between gap-3 px-4 py-2.5">
                  <span className="min-w-0">
                    <Mono>{j.kind}</Mono>
                    <span className="block truncate text-xs text-faint">{j.last_error ?? `${j.resource.kind} · ${ago(j.created_at)}`}</span>
                  </span>
                  <Status value={j.status} />
                </li>
              ))}
            </ul>
          )}
          {expiringCerts.length > 0 && (
            <div className="border-t border-line px-4 py-2 text-xs text-muted">
              {expiringCerts.length} agent certificate(s) renew automatically within 6 hours.
            </div>
          )}
        </Panel>
      </div>
    </>
  );
}
