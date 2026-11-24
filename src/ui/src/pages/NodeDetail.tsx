import * as Tabs from "@radix-ui/react-tabs";
import { IpTags, leaseOf, useClusterFabric } from "../components/IpTag";
import { Credentials } from "../components/vault/Credentials";
import { Link, useNavigate, useParams, useSearch } from "@tanstack/react-router";
import { Activity, ArrowRightLeft, Boxes, Container, Cpu, FolderOpen, Gauge, Info, KeyRound, Layers, Package, Pencil, Radio, ShieldX, SquareTerminal, Trash2 } from "lucide-react";
import { useState } from "react";

import { useAllEnvironments, useAssignServer, useClusters, useJobs, useMe, useServer, useServerCommand } from "../api/hooks";
import { nodeFacts, nodeName, type Role, type Server } from "../api/types";
import { isShell, sessionLabel, useTerminals } from "../app/terminals";
import { errorText, useToast } from "../app/toast";
import { JobDetail } from "../components/JobsDrawer";
import { jobTitle, useJobNames } from "../components/jobMeta";
import { DockerWorkspace } from "../components/docker/DockerWorkspace";
import { NodePackages } from "../components/NodePackages";
import { SshAccess } from "../components/SshAccess";
import { MoveClusterDialog } from "../components/node/MoveClusterDialog";
import { RenameNodeDialog, RevokeNodeDialog } from "../components/node/NodeDialogs";
import { ContainerTelemetryPanel } from "../components/node/ContainerTelemetryPanel";
import { NodeOverview } from "../components/node/NodeOverview";
import { NodeServices } from "../components/node/NodeServices";
import { nodeSpecs, useLatestMetrics } from "../components/NodeTable";
import { TagEditor } from "../components/TagEditor";
import { FileExplorer } from "./Files";
import { PinToggle } from "../components/Pins";
import { Badge, Button, ErrorNote, Field, IconButton, Input, Mono, Panel, Select, Skeleton, Status, ago, bytes, time } from "../components/ui";
import { ProcessPanel } from "../components/ProcessPanel";
import { AddAlertButton } from "../components/alerts/AddAlert";

export type NodeTab = "overview" | "processes" | "services" | "packages" | "containers" | "files" | "terminal" | "credentials" | "details";
export const NODE_TABS: NodeTab[] = ["overview", "processes", "services", "packages", "containers", "files", "terminal", "credentials", "details"];

/** Roles that grant `server.manage` (Members page). The server enforces it; this only hides the editor. */
const NODE_MANAGERS: Role[] = ["platform_admin", "workspace_admin", "operator"];

/** One fact: label above a mono value, `—` when unknown. */
function Fact({ k, v, hint }: { k: string; v?: string; hint?: string }) {
  return (
    <div className="min-w-0">
      <dt className="text-[11px] text-muted">{k}</dt>
      <dd className={`truncate font-mono text-xs tabular-nums ${v ? "text-fg" : "text-faint"}`} title={v ? (hint ? `${v} (${hint})` : v) : undefined}>
        {v ?? "—"}
      </dd>
    </div>
  );
}

/** OS, kernel, arch, CPU, RAM, disk and agent from the node's facts (live telemetry as fallback), plus its tags. */
function SystemPanel({ node, canEdit }: { node: Server; canEdit: boolean }) {
  const f = nodeFacts(node);
  const live = useLatestMetrics().data;
  const sample = live && typeof live === "object" ? live[node.id] : undefined;
  const specs = nodeSpecs(node, sample);
  const fromLive = (fact: number | undefined, v: number | undefined) => (!fact && v ? "live telemetry" : undefined);
  return (
    <Panel
      title="System"
      description={
        f.collected_at_ms ? (
          <span title={time(f.collected_at_ms)}>Facts collected {ago(f.collected_at_ms)}</span>
        ) : (
          "No facts collected yet. Forge collects them when the agent connects, then daily."
        )
      }
    >
      <dl className="grid grid-cols-2 gap-x-6 gap-y-3 px-4 py-3 sm:grid-cols-4 xl:grid-cols-7">
        <Fact k="OS" v={f.os_version ?? f.os ?? (node.os?.trim() || undefined)} />
        <Fact k="Kernel" v={f.kernel} />
        <Fact k="Arch" v={f.arch ?? (node.arch?.trim() || undefined)} />
        <Fact k="CPU" v={specs.cpus ? `${specs.cpus} vCPU` : undefined} hint={fromLive(f.cpus, specs.cpus)} />
        <Fact k="RAM" v={specs.mem ? bytes(specs.mem) : undefined} hint={fromLive(f.mem_total_bytes, specs.mem)} />
        <Fact k="Disk" v={specs.disk ? bytes(specs.disk) : undefined} hint={fromLive(f.disk_total_bytes, specs.disk)} />
        <Fact k="Agent" v={f.agent_version ?? (node.agent_version?.trim() || undefined)} />
      </dl>
      <div className="flex min-w-0 flex-wrap items-center gap-3 border-t border-line px-4 py-2">
        <span className="text-[11px] text-muted">Tags</span>
        <div className="min-w-0 flex-1">
          <TagEditor kind="server" id={node.id} canEdit={canEdit} />
        </div>
        {!canEdit && <span className="text-[11px] text-faint">Changing tags needs server.manage</span>}
      </div>
    </Panel>
  );
}

function Row({ k, v }: { k: string; v: React.ReactNode }) {
  return (
    <div className="flex justify-between gap-4 py-2">
      <dt className="text-muted">{k}</dt>
      <dd className="min-w-0 truncate text-right">{v}</dd>
    </div>
  );
}

const trigger =
  "flex items-center gap-2 border-b-2 border-transparent px-3 py-2.5 text-[13px] text-muted transition hover:text-fg data-[state=active]:border-accent data-[state=active]:font-medium data-[state=active]:text-fg";

export function NodeDetail() {
  const { nodeId } = useParams({ from: "/nodes/$nodeId" });
  const { tab, root: rootParam, docker: dockerTab } = useSearch({ from: "/nodes/$nodeId" });
  const navigate = useNavigate();
  const node = useServer(nodeId);
  const clusters = useClusters().data ?? [];
  const terminals = useTerminals();
  const [confirmRevoke, setConfirmRevoke] = useState(false);
  const [confirmRemove, setConfirmRemove] = useState(false);
  const [renaming, setRenaming] = useState(false);
  // Hooks before the early return below (rules of hooks).
  const bindings = useMe().data?.bindings;
  const platformAdmin = (Array.isArray(bindings) ? bindings : []).some((b) => b.role === "platform_admin");
  const canManageNode = (Array.isArray(bindings) ? bindings : []).some((b) => NODE_MANAGERS.includes(b.role));
  const fabric = useClusterFabric(node.data?.cluster_id).data;

  const n = node.data;
  if (!n) return node.error ? <ErrorNote error={node.error} /> : <Skeleton rows={6} />;
  const cluster = clusters.find((c) => c.id === n.cluster_id);
  const sessions = terminals.sessions.filter((s) => isShell(s) && s.serverId === n.id);
  const setTab = (t: string) => void navigate({ to: "/nodes/$nodeId", params: { nodeId }, search: { tab: t === "overview" ? undefined : (t as NodeTab) }, replace: true });

  return (
    <>
      {/* Header */}
      <div className="mb-3 flex flex-wrap items-start gap-x-4 gap-y-2">
        <div className="min-w-0 flex-1">
          <div className="text-[10px] leading-4 font-semibold tracking-wider text-faint uppercase">
            <Link to="/nodes" className="hover:text-accent">
              Nodes
            </Link>
            {cluster && (
              <>
                {" / "}
                <Link to="/clusters/$clusterId" params={{ clusterId: cluster.id }} className="hover:text-accent">
                  {cluster.name}
                </Link>
              </>
            )}
          </div>
          <h1 className="flex flex-wrap items-center gap-2 text-lg leading-7 font-semibold tracking-tight">
            {nodeName(n)}
            <IconButton label="Rename node" onClick={() => setRenaming(true)}>
              <Pencil className="size-4" />
            </IconButton>
            <PinToggle kind="node" id={n.id} label={nodeName(n)} sublabel={cluster?.name} />
            <Badge tone={n.role === "control" ? "accent" : "muted"}>{n.role === "control" ? "Control plane" : "Worker"}</Badge>
            <Status value={n.status} />
          </h1>
          <p className="truncate text-xs text-muted">
            {n.display_name && <span className="font-mono">{n.hostname} · </span>}
            {n.os}/{n.arch} · agent {n.agent_version} · last heartbeat {ago(n.last_seen_at)}
            {cluster ? ` · cluster ${cluster.name}` : " · no cluster"}
          </p>
          <div className="mt-1.5 flex flex-wrap gap-2 [&>div]:m-0">
            <IpTags
              tags={[
                { label: "Mesh IP", value: leaseOf(fabric, "node", n.id) },
                { label: "Instance block", value: leaseOf(fabric, "block", n.id) },
                { label: "Cluster subnet", value: fabric?.spec?.subnet },
              ]}
            />
          </div>
        </div>
        <div className="flex flex-wrap gap-2">
          <AddAlertButton className="" prefill={{ place: "node", targets: { servers: [n.id] } }} title={`Create an alert rule for ${nodeName(n)}`} />
          <Button variant="primary" disabled={n.status === "revoked"} title={n.status === "revoked" ? "The node's agent is revoked" : undefined} onClick={() => terminals.open(n)}>
            <SquareTerminal className="size-4" /> Open terminal
          </Button>
          <Button variant="danger" disabled={n.status === "revoked"} title={n.status === "revoked" ? "Already revoked" : undefined} onClick={() => setConfirmRevoke(true)}>
            <ShieldX className="size-4" /> Revoke
          </Button>
          {n.role !== "control" && (
            <Button variant="danger" title="Delete this node and its data from Forge" onClick={() => setConfirmRemove(true)}>
              <Trash2 className="size-4" /> Remove
            </Button>
          )}
        </div>
      </div>

      <Tabs.Root value={tab ?? "overview"} onValueChange={setTab}>
        <Tabs.List className="mb-4 flex gap-1 overflow-x-auto border-b border-line" aria-label="Node sections">
          <Tabs.Trigger value="overview" className={trigger}>
            <Gauge className="size-4" /> Overview
          </Tabs.Trigger>
          <Tabs.Trigger value="processes" className={trigger}>
            <Cpu className="size-4" /> Processes
          </Tabs.Trigger>
          <Tabs.Trigger value="services" className={trigger}>
            <Layers className="size-4" /> Services
          </Tabs.Trigger>
          <Tabs.Trigger value="packages" className={trigger}>
            <Package className="size-4" /> Packages
          </Tabs.Trigger>
          <Tabs.Trigger value="containers" className={trigger}>
            <Container className="size-4" /> Docker
          </Tabs.Trigger>
          <Tabs.Trigger value="files" className={trigger}>
            <FolderOpen className="size-4" /> Files
          </Tabs.Trigger>
          <Tabs.Trigger value="terminal" className={trigger}>
            <SquareTerminal className="size-4" /> SSH / Terminal
            {sessions.length > 0 && <Badge tone="accent">{sessions.length}</Badge>}
          </Tabs.Trigger>
          {platformAdmin && (
            <Tabs.Trigger value="credentials" className={trigger}>
              <KeyRound className="size-4" /> Credentials
            </Tabs.Trigger>
          )}
          <Tabs.Trigger value="details" className={trigger}>
            <Info className="size-4" /> Details
          </Tabs.Trigger>
        </Tabs.List>

        <Tabs.Content value="overview" className="space-y-4">
          <SystemPanel node={n} canEdit={canManageNode} />
          <NodeOverview node={n} />
        </Tabs.Content>
        <Tabs.Content value="processes">
          <Panel title="Processes" description="Live, like htop. Click a column to sort; refreshes only while this tab is visible.">
            <div className="p-3">
              <ProcessPanel serverId={n.id} online={n.status === "online"} canManage={canManageNode} />
            </div>
          </Panel>
        </Tabs.Content>
        <Tabs.Content value="services">
          <NodeServices node={n} />
        </Tabs.Content>
        <Tabs.Content value="packages">
          <NodePackages node={n} />
        </Tabs.Content>
        <Tabs.Content value="containers">
          <DockerWorkspace
            node={n}
            tab={dockerTab ?? "containers"}
            onTab={(t) => void navigate({ to: "/nodes/$nodeId", params: { nodeId }, search: { tab: "containers", docker: t === "containers" ? undefined : t }, replace: true })}
          />
        </Tabs.Content>
        <Tabs.Content value="files">
          <FileExplorer serverId={n.id} disabled={n.status === "revoked"} initialRoot={rootParam} />
        </Tabs.Content>
        <Tabs.Content value="terminal" className="grid gap-4 xl:grid-cols-12">
          <div className="xl:col-span-7">
            <SshAccess node={n} />
          </div>
          <div className="xl:col-span-5">
            <Panel
              title="Sessions on this node"
              description="Sessions open in the terminal panel at the bottom and stay connected while you navigate."
              actions={
                <Button variant="primary" disabled={n.status === "revoked"} title={n.status === "revoked" ? "The node's agent is revoked" : undefined} onClick={() => terminals.open(n)}>
                  <SquareTerminal className="size-4" /> New session
                </Button>
              }
            >
              <ul className="divide-y divide-line">
                {sessions.map((s) => (
                  <li key={s.key} className="flex items-center gap-2 px-4 py-2 text-[13px]">
                    <button
                      className="flex-1 text-left font-mono text-xs hover:text-accent"
                      onClick={() => {
                        terminals.activate(s.key);
                        terminals.setDock({ open: true });
                      }}
                    >
                      {sessionLabel(s)}
                    </button>
                    {s.detached ? <Badge tone="accent">separate window</Badge> : <Badge>panel</Badge>}
                    <Button variant="ghost" onClick={() => terminals.close(s.key)}>
                      Close
                    </Button>
                  </li>
                ))}
                {sessions.length === 0 && <li className="px-4 py-6 text-muted">No open sessions on {n.hostname}.</li>}
              </ul>
            </Panel>
          </div>
        </Tabs.Content>
        {platformAdmin && (
          <Tabs.Content value="credentials">
            <Credentials linkFilter={{ kind: "node", id: n.id }} />
          </Tabs.Content>
        )}
        <Tabs.Content value="details">
          <div className="space-y-4">
            <ContainerTelemetryPanel server={n.id} canEdit={canManageNode} />
            <NodeDetails nodeId={n.id} />
          </div>
        </Tabs.Content>
      </Tabs.Root>

      <RevokeNodeDialog node={n} open={confirmRevoke} onOpenChange={setConfirmRevoke} />
      <RevokeNodeDialog node={n} mode="remove" open={confirmRemove} onOpenChange={setConfirmRemove} onRemoved={() => void navigate({ to: "/nodes" })} />
      <RenameNodeDialog node={n} open={renaming} onOpenChange={setRenaming} />
    </>
  );
}

/** Identity, placement, agent commands, facts and recent jobs. */
function NodeDetails({ nodeId }: { nodeId: string }) {
  const n = useServer(nodeId).data;
  const clusters = useClusters().data ?? [];
  const envs = useAllEnvironments().data ?? [];
  const jobs = useJobs().data ?? [];
  const command = useServerCommand();
  const assign = useAssignServer();
  const toast = useToast();
  const [service, setService] = useState("");
  const [lastJob, setLastJob] = useState<string>();
  const [moving, setMoving] = useState(false);
  const names = useJobNames();
  if (!n) return null;
  const cluster = clusters.find((c) => c.id === n.cluster_id);
  const online = n.status === "online";
  const nodeJobs = jobs.filter((j) => j.resource?.kind === "server" && j.resource?.id === n.id);
  const lastFacts = nodeJobs.find((j) => j.status === "succeeded" && (j.payload?.command as { type?: string } | undefined)?.type === "collect_facts");
  const rawFacts = lastFacts?.state?.output;
  const facts = rawFacts && typeof rawFacts === "object" && !Array.isArray(rawFacts) ? (rawFacts as Record<string, unknown>) : undefined;

  const run = (cmd: Record<string, string>, label: string) =>
    command.mutate(
      { server: n.id, command: cmd },
      {
        onSuccess: (j) => {
          setLastJob(j.id);
          toast(`${label} sent to ${n.hostname}`);
        },
        onError: (e) => toast(errorText(e), "error"),
      },
    );

  return (
    <div className="grid gap-4 xl:grid-cols-12">
      <div className="space-y-4 xl:col-span-4">
        <Panel title="Identity">
          <dl className="divide-y divide-line px-4 text-[13px]">
            <Row k="Node id" v={<Mono>{n.id}</Mono>} />
            <Row k="Role" v={n.role === "control" ? "Control plane (hosts Forge)" : "Worker"} />
            <Row k="Last heartbeat" v={ago(n.last_seen_at)} />
            <Row k="Enrolled" v={time(n.created_at)} />
            <Row k="Certificate" v={n.cert_serial ? <Mono>{n.cert_serial.slice(0, 16)}…</Mono> : "—"} />
            <Row k="Certificate renews" v={n.cert_not_after ? `before ${time(n.cert_not_after)}` : "—"} />
          </dl>
        </Panel>
        <Panel title="Placement" description="Cluster and environment decide who can manage this node.">
          <div className="space-y-3 p-4">
            <div className="space-y-1">
              <span className="text-xs font-medium text-muted">Cluster</span>
              <div className="flex min-w-0 items-center gap-2">
                <span className="min-w-0 flex-1 truncate">
                  {n.cluster_id ? (
                    <Link to="/clusters/$clusterId" params={{ clusterId: n.cluster_id }} className="hover:text-accent" title={cluster?.name}>
                      <Badge tone="accent">{cluster?.name ?? n.cluster_id.slice(0, 8)}</Badge>
                    </Link>
                  ) : (
                    <span className="text-faint">No cluster</span>
                  )}
                </span>
                <Button
                  disabled={n.status === "revoked"}
                  title={n.status === "revoked" ? "A revoked node can't change cluster" : undefined}
                  onClick={() => setMoving(true)}
                >
                  <ArrowRightLeft className="size-4" /> Move to cluster…
                </Button>
              </div>
            </div>
            <Field label="Environment">
              <Select
                value={n.environment_id ?? ""}
                onChange={(e) =>
                  assign.mutate(
                    { id: n.id, cluster_id: n.cluster_id, environment_id: e.target.value || null },
                    { onSuccess: () => toast("Environment updated"), onError: (err) => toast(errorText(err), "error") },
                  )
                }
              >
                <option value="">None</option>
                {envs.map((e) => (
                  <option key={e.id} value={e.id}>
                    {e.name}
                  </option>
                ))}
              </Select>
            </Field>
          </div>
        </Panel>
      </div>

      <div className="space-y-4 xl:col-span-8">
        <Panel title="Agent commands" description={online ? "Signed, replay-protected commands. Results arrive in seconds." : "The agent must be online to receive commands."}>
          <div className="flex flex-wrap items-end gap-2 p-4">
            <Button disabled={!online} title={online ? undefined : "The agent must be online"} onClick={() => run({ type: "ping" }, "Ping")}>
              <Radio className="size-4" /> Ping
            </Button>
            <Button disabled={!online} title={online ? undefined : "The agent must be online"} onClick={() => run({ type: "collect_facts" }, "Fact collection")}>
              <Cpu className="size-4" /> Collect facts
            </Button>
            <form
              className="flex items-end gap-2"
              onSubmit={(e) => {
                e.preventDefault();
                run({ type: "service_status", name: service }, `Status of ${service}`);
              }}
            >
              <Field label="Service">
                <Input value={service} onChange={(e) => setService(e.target.value)} placeholder={n.os === "windows" ? "W3SVC" : "sshd"} pattern="[A-Za-z0-9@._:\-]+" required />
              </Field>
              <Button disabled={!online} title={online ? undefined : "The agent must be online"}>
                <Activity className="size-4" /> Check status
              </Button>
            </form>
          </div>
          {lastJob && (
            <div className="border-t border-line p-4">
              <JobDetail id={lastJob} />
            </div>
          )}
        </Panel>

        <Panel title="System facts" description={lastFacts ? `Collected ${ago(lastFacts.updated_at)}` : "Run “Collect facts” to populate."}>
          {facts ? (
            <dl className="grid gap-x-8 px-4 py-2 text-[13px] sm:grid-cols-2">
              {Object.entries(facts).map(([k, v]) => (
                <Row key={k} k={k.replace(/_/g, " ")} v={String(v ?? "—")} />
              ))}
            </dl>
          ) : (
            <p className="px-4 py-6 text-muted">No facts collected yet.</p>
          )}
        </Panel>

        <MoveClusterDialog node={n} open={moving} onOpenChange={setMoving} />
        <Panel title="Recent jobs on this node">
          <ul className="divide-y divide-line">
            {nodeJobs.slice(0, 10).map((j) => (
              <li key={j.id}>
                <button className="flex w-full items-center justify-between gap-3 px-4 py-2 text-left hover:bg-panel-2" onClick={() => setLastJob(j.id)}>
                  <span className="flex min-w-0 items-center gap-2">
                    <Boxes className="size-3.5 text-faint" />
                    <span className="truncate text-[13px]">{jobTitle(j, names)}</span>
                    <span className="font-mono text-[11px] whitespace-nowrap text-faint">{ago(j.created_at)}</span>
                  </span>
                  <Status value={j.status} />
                </button>
              </li>
            ))}
            {nodeJobs.length === 0 && <li className="px-4 py-6 text-muted">No jobs yet.</li>}
          </ul>
        </Panel>
      </div>
    </div>
  );
}
