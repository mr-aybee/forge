import { Link } from "@tanstack/react-router";
import { ArrowDown, ArrowUp, Boxes, Crown, KeyRound, Loader2, LogOut, Pause, Play, Plus, Tag, Trash2, X } from "lucide-react";
import { useState } from "react";

import { useServers } from "../../api/hooks";
import {
  type SwarmAvailability,
  type SwarmMember,
  type SwarmNode,
  type SwarmRole,
  type SwarmView,
  useJoinSwarm,
  useLeaveSwarm,
  useRotateSwarmTokens,
  useSwarm,
  useSwarmAddresses,
  useUpdateSwarmNode,
} from "../../api/swarm";
import { nodeName, type Server } from "../../api/types";
import { errorText, useToast } from "../../app/toast";
import { SwarmNetworks } from "../swarm/SwarmNetworks";
import { SwarmObjects } from "../swarm/SwarmObjects";
import { SwarmStacks } from "../swarm/SwarmStacks";
import { SwarmVolumes } from "../swarm/SwarmVolumes";
import { Badge, Button, Confirm, CopyIcon, Empty, ErrorNote, Field, Input, Modal, Panel, RowMenu, Segmented, Select, Skeleton, Stat, Table, Td, TextArea, ago, time } from "../ui";

type SwarmSection = "overview" | "nodes" | "stacks" | "objects" | "networks" | "volumes";

/** Health at a glance: managers and workers, leader, nodes down or draining, failed joins. */
function Health({ v, servers, onNodes }: { v: SwarmView; servers: Server[]; onNodes: () => void }) {
  const nodes = v.nodes ?? [];
  const managers = nodes.filter((n) => n.role === "manager");
  const workers = nodes.filter((n) => n.role === "worker");
  const down = nodes.filter((n) => n.state !== "ready");
  const unavailable = nodes.filter((n) => n.availability !== "active");
  const leader = nodes.find((n) => n.leader);
  const failed = v.members.filter((m) => m.state === "failed");
  const busy = v.members.filter((m) => m.state === "joining" || m.state === "leaving");
  const byNode = new Map(v.members.filter((m) => m.node_id).map((m) => [m.node_id, m.server_id]));
  const leaderServer = servers.find((s) => s.id === byNode.get(leader?.id));
  const known = v.nodes !== null;
  const healthy = known && down.length === 0 && failed.length === 0 && v.quorum.status === "ok";
  return (
    <div className="space-y-2">
      {v.nodes_error && <ErrorNote error={new Error(`No manager answered: ${v.nodes_error}`)} />}
      <div className="grid grid-cols-2 gap-3 md:grid-cols-4">
        <Stat label="Health" value={!known ? "—" : healthy ? "Healthy" : "Degraded"} tone={!known ? undefined : healthy ? "ok" : "warn"} hint={v.quorum.status === "ok" ? "quorum ok" : `quorum: ${v.quorum.status}`} />
        <Stat label="Managers" value={known ? `${managers.filter((n) => n.state === "ready").length} / ${managers.length}` : "—"} hint={`tolerates ${v.quorum.tolerates} failure${v.quorum.tolerates === 1 ? "" : "s"}`} tone={v.quorum.status === "ok" ? undefined : "warn"} onClick={onNodes} />
        <Stat label="Workers" value={known ? `${workers.filter((n) => n.state === "ready").length} / ${workers.length}` : "—"} hint="ready / total" onClick={onNodes} />
        <Stat label="Nodes down" value={known ? down.length : "—"} tone={down.length ? "bad" : undefined} hint={unavailable.length ? `${unavailable.length} paused or draining` : "none paused or draining"} onClick={onNodes} />
      </div>
      <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-muted">
        <span className="flex min-w-0 items-center gap-1">
          <Crown className="size-3.5 text-warn" /> Leader{" "}
          {leader ? <NodeLink server={leaderServer} fallback={leader.hostname} /> : <span className="text-faint">—</span>}
        </span>
        {down.length > 0 && (
          <span className="min-w-0 truncate text-bad" title={down.map((n) => n.hostname).join(", ")}>
            Down: {down.map((n) => n.hostname).join(", ")}
          </span>
        )}
        {failed.length > 0 && <span className="text-bad">{failed.length} failed join{failed.length === 1 ? "" : "s"}: see Nodes</span>}
        {busy.length > 0 && (
          <span className="flex items-center gap-1">
            <Loader2 className="size-3.5 animate-spin" /> {busy.length} node{busy.length === 1 ? "" : "s"} joining or leaving
          </span>
        )}
      </div>
    </div>
  );
}

const isLinux = (n: Server) => n.os.toLowerCase().includes("linux");
const stateTone = { joined: "ok", joining: "accent", leaving: "warn", failed: "bad" } as const;
const availTone = { active: "ok", pause: "warn", drain: "warn" } as const;

/** Docker Swarm workspace of a Swarm-mode cluster: quorum, nodes and membership. */
export function ClusterSwarmTab({ clusterId, environmentId }: { clusterId: string; environmentId?: string | null }) {
  const q = useSwarm(clusterId);
  const servers = useServers();
  const [adding, setAdding] = useState<SwarmRole | null>(null);
  const [rotating, setRotating] = useState(false);
  const [section, setSection] = useState<SwarmSection>("overview");

  if (q.error) return <ErrorNote error={q.error} />;
  if (q.isLoading || !q.data) return <Skeleton rows={6} />;
  const v = q.data;
  const clusterNodes = (servers.data ?? []).filter((n) => n.cluster_id === clusterId && n.status !== "revoked");

  return (
    <div className="space-y-4">
      {!v.swarm && v.members.length === 0 ? (
        <Panel>
          <Empty
            icon={<Boxes className="size-5" />}
            title="No swarm yet"
            action={
              <Button variant="primary" onClick={() => setAdding("manager")}>
                <Plus className="size-4" /> Create swarm
              </Button>
            }
          >
            Pick a Linux node of this cluster as the first manager. Forge initialises the swarm there, installs Docker Engine if it is missing, and keeps the
            join tokens on the server.
          </Empty>
        </Panel>
      ) : (
        <>
          <Segmented
            label="Swarm sections"
            value={section}
            onChange={setSection}
            options={[
              { id: "overview", label: "Overview" },
              { id: "nodes", label: "Nodes", count: v.members.length },
              { id: "stacks", label: "Stacks" },
              { id: "objects", label: "Secrets & configs" },
              { id: "networks", label: "Networks" },
              { id: "volumes", label: "Volumes" },
            ]}
          />
          {section === "overview" && (
            <>
              <Overview v={v} onAdd={() => setAdding("worker")} onRotate={() => setRotating(true)} />
              <Health v={v} servers={servers.data ?? []} onNodes={() => setSection("nodes")} />
            </>
          )}
          {section === "nodes" && (
            <>
              <Overview v={v} onAdd={() => setAdding("worker")} onRotate={() => setRotating(true)} />
              <Visualizer v={v} servers={servers.data ?? []} />
              <Members v={v} clusterId={clusterId} servers={servers.data ?? []} />
            </>
          )}
          {section === "stacks" && <SwarmStacks clusterId={clusterId} environmentId={environmentId} />}
          {section === "objects" && <SwarmObjects clusterId={clusterId} />}
          {section === "networks" && <SwarmNetworks clusterId={clusterId} />}
          {section === "volumes" && <SwarmVolumes members={v.members} servers={servers.data ?? []} />}
        </>
      )}
      {adding && <AddNodeDialog v={v} clusterId={clusterId} nodes={clusterNodes} role={adding} onClose={() => setAdding(null)} />}
      {rotating && <RotateDialog clusterId={clusterId} onClose={() => setRotating(false)} />}
    </div>
  );
}

function Overview({ v, onAdd, onRotate }: { v: SwarmView; onAdd: () => void; onRotate: () => void }) {
  const { quorum: qr, swarm } = v;
  const explain =
    qr.status === "single"
      ? "single manager: losing it stops scheduling"
      : qr.status === "even"
        ? `even count adds no fault tolerance; tolerates ${qr.tolerates} manager failure${qr.tolerates === 1 ? "" : "s"}`
        : `tolerates ${qr.tolerates} manager failure${qr.tolerates === 1 ? "" : "s"}`;
  return (
    <div className="flex flex-wrap items-center gap-x-5 gap-y-2 rounded-lg border border-line bg-panel px-3 py-2 text-xs">
      <span className="flex min-w-0 items-center gap-1.5">
        <span className="text-muted">Swarm</span>
        {swarm ? (
          <>
            <span className="max-w-40 truncate font-mono text-[11px] select-all" title={swarm.swarm_id}>
              {swarm.swarm_id}
            </span>
            <CopyIcon text={swarm.swarm_id} label="Copy swarm id" />
          </>
        ) : (
          <span className="text-faint">—</span>
        )}
      </span>
      <span className="flex items-center gap-1.5">
        <span className="text-muted">Managers</span>
        <Badge tone={qr.status === "ok" ? "ok" : "warn"}>{qr.managers}</Badge>
        <span className="text-muted">{qr.managers ? explain : "—"}</span>
      </span>
      <span className="flex items-center gap-1.5">
        <span className="text-muted">Autolock</span>
        {swarm ? <Badge tone={swarm.autolock ? "accent" : "muted"}>{swarm.autolock ? "on" : "off"}</Badge> : "—"}
      </span>
      <span className="flex min-w-0 items-center gap-1.5">
        <span className="text-muted">Address pools</span>
        <span className="truncate font-mono text-[11px]" title={swarm?.default_addr_pool.join(", ")}>
          {swarm?.default_addr_pool.length ? swarm.default_addr_pool.join(", ") : "Docker default"}
        </span>
      </span>
      {swarm?.port !== undefined && (
        <span className="flex items-center gap-1.5">
          <span className="text-muted">Port</span>
          <span className="font-mono text-[11px] tabular-nums">{swarm.port}</span>
        </span>
      )}
      <span className="ml-auto flex gap-2">
        <Button onClick={onRotate} disabled={!swarm} title={swarm ? undefined : "No swarm yet"}>
          <KeyRound className="size-4" /> Rotate join tokens
        </Button>
        <Button variant="primary" onClick={onAdd}>
          <Plus className="size-4" /> Add node
        </Button>
      </span>
    </div>
  );
}

/** Live nodes when a manager answered, otherwise Forge's member records. */
function Visualizer({ v, servers }: { v: SwarmView; servers: Server[] }) {
  const byId = new Map(servers.map((s) => [s.id, s]));
  const memberByNode = new Map(v.members.filter((m) => m.node_id).map((m) => [m.node_id, m]));
  return (
    <section className="space-y-2">
      <h3 className="text-[11px] font-medium tracking-wide text-muted uppercase">Nodes</h3>
      {v.nodes_error && <ErrorNote error={new Error(`No manager answered: ${v.nodes_error}. Showing Forge's records.`)} />}
      <div className="grid grid-cols-[repeat(auto-fill,minmax(14rem,1fr))] gap-2">
        {v.nodes
          ? v.nodes.map((n) => <NodeCard key={n.id} node={n} member={memberByNode.get(n.id)} server={byId.get(memberByNode.get(n.id)?.server_id ?? "")} />)
          : v.members.map((m) => <MemberCard key={m.server_id} m={m} server={byId.get(m.server_id)} />)}
      </div>
      {v.nodes?.length === 0 && <div className="text-xs text-muted">The manager reports no nodes.</div>}
    </section>
  );
}

function NodeLink({ server, fallback }: { server?: Server; fallback: string }) {
  return server ? (
    <Link to="/nodes/$nodeId" params={{ nodeId: server.id }} className="truncate font-medium hover:text-accent" title={nodeName(server)}>
      {nodeName(server)}
    </Link>
  ) : (
    <span className="truncate font-medium" title={fallback}>
      {fallback}
    </span>
  );
}

function NodeCard({ node: n, member, server }: { node: SwarmNode; member?: SwarmMember; server?: Server }) {
  const ready = n.state === "ready";
  const labels = Object.entries(n.labels);
  return (
    <div className="min-w-0 rounded-md border border-line bg-panel px-2.5 py-2 text-xs">
      <div className="flex min-w-0 items-center gap-1.5">
        <span aria-label={n.state} title={n.state} className={`size-2 shrink-0 rounded-full ${ready ? "bg-ok" : "bg-bad"}`} />
        <NodeLink server={server} fallback={n.hostname} />
        {n.leader && <Crown className="size-3.5 shrink-0 text-warn" aria-label="Leader" />}
        <span className="ml-auto shrink-0">
          <Badge tone={n.role === "manager" ? "accent" : "muted"}>{n.role}</Badge>
        </span>
      </div>
      <div className="mt-1 flex flex-wrap items-center gap-1.5 text-muted">
        <Badge tone={availTone[n.availability]}>{n.availability}</Badge>
        <span className="font-mono text-[11px] tabular-nums">{n.engine_version ? `v${n.engine_version}` : "—"}</span>
        {n.addr && <span className="truncate font-mono text-[11px]">{n.addr}</span>}
        {member && member.state !== "joined" && <Badge tone={stateTone[member.state]}>{member.state}</Badge>}
      </div>
      {labels.length > 0 && (
        <div className="mt-1 flex flex-wrap gap-1">
          {labels.map(([k, val]) => (
            <span key={k} className="max-w-full truncate rounded bg-panel-2 px-1 font-mono text-[10px] text-muted" title={`${k}=${val}`}>
              {k}={val}
            </span>
          ))}
        </div>
      )}
    </div>
  );
}

function MemberCard({ m, server }: { m: SwarmMember; server?: Server }) {
  return (
    <div className="min-w-0 rounded-md border border-line bg-panel px-2.5 py-2 text-xs">
      <div className="flex min-w-0 items-center gap-1.5">
        <span className={`size-2 shrink-0 rounded-full ${m.state === "joined" ? "bg-faint" : m.state === "failed" ? "bg-bad" : "bg-accent"}`} />
        <NodeLink server={server} fallback={m.hostname ?? m.server_id} />
        <span className="ml-auto shrink-0">
          <Badge tone={m.role === "manager" ? "accent" : "muted"}>{m.role}</Badge>
        </span>
      </div>
      <div className="mt-1 flex items-center gap-1.5 text-muted">
        <Badge tone={stateTone[m.state]}>{m.state}</Badge>
        <span className="truncate font-mono text-[11px]">{m.advertise_addr ?? "—"}</span>
      </div>
    </div>
  );
}

type Pending = { kind: "leave"; m: SwarmMember } | { kind: "labels"; m: SwarmMember; labels: Record<string, string> };

function Members({ v, clusterId, servers }: { v: SwarmView; clusterId: string; servers: Server[] }) {
  const toast = useToast();
  const update = useUpdateSwarmNode(clusterId);
  const [pending, setPending] = useState<Pending | null>(null);
  const byId = new Map(servers.map((s) => [s.id, s]));
  const nodeOf = (m: SwarmMember) => (m.node_id ? v.nodes?.find((n) => n.id === m.node_id) : undefined);
  const managers = v.members.filter((m) => m.role === "manager" && m.state === "joined").length;
  const name = (m: SwarmMember) => (byId.get(m.server_id) ? nodeName(byId.get(m.server_id) as Server) : (m.hostname ?? m.server_id));

  const patch = (m: SwarmMember, b: { role?: SwarmRole; availability?: SwarmAvailability }, msg: string) =>
    update.mutate(
      { server: m.server_id, ...b },
      { onSuccess: () => toast(`${name(m)}: ${msg}`), onError: (e) => toast(errorText(e), "error") },
    );

  return (
    <Panel title={`Members · ${v.members.length}`}>
      {v.members.length === 0 ? (
        <Empty title="No members">Add a node to join it to the swarm.</Empty>
      ) : (
        <Table head={["Node", "Role", "State", "Availability", "Advertise address", "Updated", ""]}>
          {v.members.map((m) => {
            const n = nodeOf(m);
            const joined = m.state === "joined";
            const lastManager = m.role === "manager" && managers <= 1;
            const why = joined ? undefined : `Node is ${m.state}`;
            return (
              <tr key={m.server_id} className="text-[13px]">
                <Td className="max-w-56">
                  <div className="flex min-w-0 items-center gap-1.5">
                    <NodeLink server={byId.get(m.server_id)} fallback={m.hostname ?? m.server_id} />
                    {n?.leader && <Crown className="size-3.5 shrink-0 text-warn" aria-label="Leader" />}
                  </div>
                </Td>
                <Td>
                  <Badge tone={m.role === "manager" ? "accent" : "muted"}>{m.role}</Badge>
                </Td>
                <Td className="max-w-72">
                  <div className="flex items-center gap-1.5">
                    <Badge tone={stateTone[m.state]}>{m.state}</Badge>
                    {(m.state === "joining" || m.state === "leaving") && <Loader2 className="size-3.5 animate-spin text-muted" />}
                  </div>
                  {m.error && (
                    <div className="mt-0.5 truncate text-xs text-bad select-all" title={m.error}>
                      {m.error}
                    </div>
                  )}
                </Td>
                <Td>{n ? <Badge tone={availTone[n.availability]}>{n.availability}</Badge> : <span className="text-faint">—</span>}</Td>
                <Td className="font-mono text-[11px] select-all">{m.advertise_addr ?? "—"}</Td>
                <Td className="text-xs whitespace-nowrap text-muted">
                  <span title={m.updated_at ? time(m.updated_at) : undefined}>{m.updated_at ? ago(m.updated_at) : "—"}</span>
                </Td>
                <Td className="text-right">
                  <RowMenu
                    label={`Actions for ${name(m)}`}
                    actions={[
                      {
                        label: "Promote to manager",
                        icon: ArrowUp,
                        hidden: m.role === "manager",
                        disabled: !joined,
                        hint: why,
                        onSelect: () => patch(m, { role: "manager" }, "promoted to manager"),
                      },
                      {
                        label: "Demote to worker",
                        icon: ArrowDown,
                        hidden: m.role !== "manager",
                        disabled: !joined || lastManager,
                        hint: why ?? (lastManager ? "The last manager can't be demoted" : undefined),
                        onSelect: () => patch(m, { role: "worker" }, "demoted to worker"),
                      },
                      {
                        label: "Activate",
                        icon: Play,
                        tone: "ok",
                        hidden: n?.availability === "active",
                        disabled: !joined,
                        hint: why,
                        onSelect: () => patch(m, { availability: "active" }, "active"),
                      },
                      {
                        label: "Pause (no new tasks)",
                        icon: Pause,
                        tone: "warn",
                        hidden: n?.availability === "pause",
                        disabled: !joined,
                        hint: why,
                        onSelect: () => patch(m, { availability: "pause" }, "paused"),
                      },
                      {
                        label: "Drain (move tasks away)",
                        icon: LogOut,
                        tone: "warn",
                        hidden: n?.availability === "drain",
                        disabled: !joined,
                        hint: why,
                        onSelect: () => patch(m, { availability: "drain" }, "draining"),
                      },
                      {
                        label: "Edit labels",
                        icon: Tag,
                        disabled: !joined || !n,
                        hint: why ?? (!n ? "No manager answered" : undefined),
                        onSelect: () => setPending({ kind: "labels", m, labels: n?.labels ?? {} }),
                      },
                      {
                        label: "Leave swarm",
                        icon: Trash2,
                        danger: true,
                        disabled: m.state === "leaving",
                        hint: m.state === "leaving" ? "Already leaving" : undefined,
                        onSelect: () => setPending({ kind: "leave", m }),
                      },
                    ]}
                  />
                </Td>
              </tr>
            );
          })}
        </Table>
      )}
      {pending?.kind === "leave" && (
        <LeaveDialog clusterId={clusterId} m={pending.m} name={name(pending.m)} v={v} onClose={() => setPending(null)} />
      )}
      {pending?.kind === "labels" && (
        <LabelsDialog clusterId={clusterId} m={pending.m} name={name(pending.m)} initial={pending.labels} onClose={() => setPending(null)} />
      )}
    </Panel>
  );
}

function LeaveDialog({ clusterId, m, name, v, onClose }: { clusterId: string; m: SwarmMember; name: string; v: SwarmView; onClose: () => void }) {
  const toast = useToast();
  const leave = useLeaveSwarm(clusterId);
  const [force, setForce] = useState(m.state === "failed");
  const others = v.members.filter((x) => x.server_id !== m.server_id);
  const managersLeft = others.filter((x) => x.role === "manager").length;
  return (
    <Confirm
      open
      onOpenChange={(o) => !o && onClose()}
      title={`Remove ${name} from the swarm?`}
      confirmLabel={force ? "Force leave" : "Leave swarm"}
      busy={leave.isPending}
      onConfirm={() =>
        leave.mutate(
          { server: m.server_id, force },
          {
            onSuccess: () => {
              toast(`${name} is leaving the swarm`);
              onClose();
            },
            onError: (e) => toast(errorText(e), "error"),
          },
        )
      }
      body={
        <div className="space-y-2 text-[13px]">
          <p>Swarm tasks on {name} stop and are rescheduled on the remaining nodes. The node stays in the cluster.</p>
          {m.role === "manager" && (
            <p className="text-warn">
              {managersLeft === 0
                ? others.length
                  ? "This is the last manager while workers remain; the server will refuse."
                  : "This is the last node: the swarm is dissolved."
                : `${managersLeft} manager${managersLeft === 1 ? "" : "s"} remain; check quorum before removing a manager.`}
            </p>
          )}
          <label className="flex items-center gap-2">
            <input type="checkbox" checked={force} onChange={(e) => setForce(e.target.checked)} />
            Force
            <span className="text-xs text-faint">for a failed or unreachable node; forgets it without a clean leave</span>
          </label>
        </div>
      }
    />
  );
}

let rowSeq = 0;
type LabelRow = { id: number; k: string; v: string };

function LabelsDialog({ clusterId, m, name, initial, onClose }: { clusterId: string; m: SwarmMember; name: string; initial: Record<string, string>; onClose: () => void }) {
  const toast = useToast();
  const update = useUpdateSwarmNode(clusterId);
  const [rows, setRows] = useState<LabelRow[]>(() => Object.entries(initial).map(([k, v]) => ({ id: ++rowSeq, k, v })));
  const keys = rows.map((r) => r.k.trim()).filter(Boolean);
  const dup = keys.length !== new Set(keys).size;
  const set = (id: number, p: Partial<LabelRow>) => setRows((rs) => rs.map((r) => (r.id === id ? { ...r, ...p } : r)));
  return (
    <Modal open onOpenChange={(o) => !o && onClose()} title={`Labels · ${name}`} description="Swarm node labels; services place tasks with node.labels constraints. Saving replaces all labels.">
      <form
        className="space-y-2"
        onSubmit={(e) => {
          e.preventDefault();
          const labels = Object.fromEntries(rows.filter((r) => r.k.trim()).map((r) => [r.k.trim(), r.v]));
          update.mutate(
            { server: m.server_id, labels },
            {
              onSuccess: () => {
                toast(`Labels of ${name} saved`);
                onClose();
              },
              onError: (e) => toast(errorText(e), "error"),
            },
          );
        }}
      >
        {rows.length === 0 && <div className="text-xs text-muted">No labels.</div>}
        {rows.map((r) => (
          <div key={r.id} className="flex items-center gap-2">
            <Input aria-label="Label key" placeholder="key" value={r.k} onChange={(e) => set(r.id, { k: e.target.value })} className="font-mono" />
            <span className="text-faint">=</span>
            <Input aria-label="Label value" placeholder="value" value={r.v} onChange={(e) => set(r.id, { v: e.target.value })} className="font-mono" />
            <button type="button" aria-label="Remove label" className="rounded p-1 text-muted hover:text-bad focus-visible:outline-2 focus-visible:outline-accent" onClick={() => setRows((rs) => rs.filter((x) => x.id !== r.id))}>
              <X className="size-4" />
            </button>
          </div>
        ))}
        <Button type="button" onClick={() => setRows((rs) => [...rs, { id: ++rowSeq, k: "", v: "" }])}>
          <Plus className="size-4" /> Add label
        </Button>
        {dup && <div className="text-xs text-bad">Each key can appear once.</div>}
        <div className="flex justify-end gap-2 pt-3">
          <Button type="button" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" disabled={dup || update.isPending}>
            {update.isPending && <Loader2 className="size-4 animate-spin" />} Save labels
          </Button>
        </div>
      </form>
    </Modal>
  );
}

const CIDR = /^\d{1,3}(\.\d{1,3}){3}\/\d{1,2}$/;

function AddNodeDialog({ v, clusterId, nodes, role, onClose }: { v: SwarmView; clusterId: string; nodes: Server[]; role: SwarmRole; onClose: () => void }) {
  const toast = useToast();
  const join = useJoinSwarm(clusterId);
  const first = !v.swarm && v.members.length === 0;
  // `advertise_addr: null` means "use the best suggested address" (mesh first).
  const [f, setF] = useState({ server_id: "", role: first ? ("manager" as SwarmRole) : role, advertise_addr: null as string | null, pools: "", autolock: false });
  const addrs = useSwarmAddresses(clusterId, f.server_id);
  const suggested = addrs.data ?? [];
  const advertise = f.advertise_addr ?? suggested[0]?.address ?? "";
  const members = new Set(v.members.map((m) => m.server_id));
  const candidates = nodes.filter((n) => !members.has(n.id));
  const pools = f.pools.split(/[\s,]+/).filter(Boolean);
  const badPool = pools.find((p) => !CIDR.test(p));
  const picked = candidates.find((n) => n.id === f.server_id);
  const ok = !!picked && isLinux(picked) && !badPool && pools.length <= 8;

  return (
    <Modal open onOpenChange={(o) => !o && onClose()} title={first ? "Create swarm" : "Add node to swarm"} description={first ? "The first node initialises the swarm and becomes its manager." : "The node joins with a token that never leaves the server."}>
      <form
        className="space-y-3"
        onSubmit={(e) => {
          e.preventDefault();
          if (!ok) return;
          join.mutate(
            {
              server_id: f.server_id,
              role: f.role,
              advertise_addr: advertise.trim() || undefined,
              ...(first ? { default_addr_pool: pools.length ? pools : undefined, autolock: f.autolock } : {}),
            },
            {
              onSuccess: (r) => {
                const n = r.installs.length;
                toast(n ? `Installing Docker Engine on ${n} node${n === 1 ? "" : "s"}, then joining ${nodeName(picked)}` : `${nodeName(picked)} is joining the swarm`);
                onClose();
              },
              onError: (e) => toast(errorText(e), "error"),
            },
          );
        }}
      >
        <Field label="Node" hint={candidates.length ? "Linux nodes of this cluster that are not members. Docker Engine is installed first when it is missing." : undefined}>
          {candidates.length ? (
            <Select value={f.server_id} onChange={(e) => setF({ ...f, server_id: e.target.value, advertise_addr: null })} aria-label="Node">
              <option value="">Choose a node…</option>
              {candidates.map((n) => (
                <option key={n.id} value={n.id} disabled={!isLinux(n)}>
                  {nodeName(n)}
                  {isLinux(n) ? "" : ` (${n.os || "unknown OS"}: Linux only)`}
                </option>
              ))}
            </Select>
          ) : (
            <div className="text-xs text-muted">Every node of this cluster is already a member. Add a node to the cluster first.</div>
          )}
        </Field>
        <Field label="Role" hint={first ? "The first node must be a manager." : "Use 3 or 5 managers for fault tolerance; even counts add none."}>
          <Select value={f.role} disabled={first} onChange={(e) => setF({ ...f, role: e.target.value as SwarmRole })} aria-label="Role">
            <option value="manager">manager</option>
            <option value="worker">worker</option>
          </Select>
        </Field>
        <Field
          label="Advertise address"
          hint={
            addrs.error
              ? "No suggestions from this server. IP or interface other nodes reach this node on; Docker picks one when empty."
              : "IP or interface other nodes reach this node on. The encrypted mesh address is preferred; Docker picks one when empty."
          }
        >
          <div className="flex min-w-0 items-center gap-2">
            <Input value={advertise} onChange={(e) => setF({ ...f, advertise_addr: e.target.value })} placeholder="192.0.2.5 or eth1" className="font-mono" />
            {addrs.isFetching && <Loader2 className="size-3.5 shrink-0 animate-spin text-muted" />}
          </div>
          {suggested.length > 0 && (
            <div className="mt-1 flex flex-wrap gap-1">
              {suggested.map((a, i) => (
                <button
                  key={a.address}
                  type="button"
                  onClick={() => setF({ ...f, advertise_addr: a.address })}
                  title={a.interface ? `${a.source} · ${a.interface}` : a.source}
                  className={`inline-flex items-center gap-1 rounded border px-1.5 py-0.5 font-mono text-[11px] focus-visible:outline-2 focus-visible:outline-accent ${advertise === a.address ? "border-accent text-accent" : "border-line text-muted hover:text-fg"}`}
                >
                  {a.address}
                  <span className={`rounded px-1 font-sans text-[10px] ${a.source === "mesh" ? "bg-accent-soft text-accent" : "bg-panel-2 text-faint"}`}>{a.source}</span>
                  {i === 0 && <span className="font-sans text-[10px] text-faint">best</span>}
                </button>
              ))}
            </div>
          )}
        </Field>
        {first && (
          <>
            <Field label="Address pools" hint={badPool ? <span className="text-bad">Not a CIDR: {badPool}</span> : "Optional. Overlay CIDRs, comma or newline separated, at most 8."}>
              <TextArea rows={2} value={f.pools} onChange={(e) => setF({ ...f, pools: e.target.value })} placeholder="10.20.0.0/16" className="font-mono" />
            </Field>
            <label className="flex items-start gap-2 text-[13px]">
              <input type="checkbox" className="mt-0.5" checked={f.autolock} onChange={(e) => setF({ ...f, autolock: e.target.checked })} />
              <span>
                Autolock
                <span className="block text-xs text-faint">Encrypts manager keys at rest; a restarted manager needs the unlock key before it rejoins.</span>
              </span>
            </label>
          </>
        )}
        <div className="flex justify-end gap-2 pt-2">
          <Button type="button" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" disabled={!ok || join.isPending}>
            {join.isPending && <Loader2 className="size-4 animate-spin" />}
            {first ? "Create swarm" : "Join"}
          </Button>
        </div>
      </form>
    </Modal>
  );
}

function RotateDialog({ clusterId, onClose }: { clusterId: string; onClose: () => void }) {
  const toast = useToast();
  const rotate = useRotateSwarmTokens(clusterId);
  const [f, setF] = useState({ worker: true, manager: false });
  return (
    <Confirm
      open
      onOpenChange={(o) => !o && onClose()}
      title="Rotate join tokens?"
      confirmLabel="Rotate"
      busy={rotate.isPending}
      disabled={!f.worker && !f.manager}
      onConfirm={() =>
        rotate.mutate(f, {
          onSuccess: () => {
            toast("Join tokens rotated");
            onClose();
          },
          onError: (e) => toast(errorText(e), "error"),
        })
      }
      body={
        <div className="space-y-2 text-[13px]">
          <p>Old tokens stop working at once. Nodes already in the swarm are not affected; joins in progress may fail and need a retry.</p>
          <label className="flex items-center gap-2">
            <input type="checkbox" checked={f.worker} onChange={(e) => setF({ ...f, worker: e.target.checked })} /> Worker token
          </label>
          <label className="flex items-center gap-2">
            <input type="checkbox" checked={f.manager} onChange={(e) => setF({ ...f, manager: e.target.checked })} /> Manager token
          </label>
          {!f.worker && !f.manager && <p className="text-xs text-warn">Pick at least one token.</p>}
        </div>
      }
    />
  );
}
