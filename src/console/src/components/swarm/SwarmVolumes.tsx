import { Link } from "@tanstack/react-router";
import { Database, Info, Loader2, Plus } from "lucide-react";
import { useState } from "react";

import { type VolumeRemote, useDockerOp, useDockerVolumes } from "../../api/dockerResources";
import type { SwarmMember } from "../../api/swarm";
import { nodeName, type Server } from "../../api/types";
import { errorText, useToast } from "../../app/toast";
import { Badge, Button, Empty, ErrorNote, Field, Input, Modal, Panel, Segmented, Select, Skeleton, Table, Td, ago, time } from "../ui";

const VOL_RE = /^[A-Za-z0-9][A-Za-z0-9_.-]*$/;

/** Docker volumes on one member node at a time, with guidance for stateful services. */
export function SwarmVolumes({ members, servers }: { members: SwarmMember[]; servers: Server[] }) {
  const joined = members.filter((m) => m.state === "joined");
  const [picked, setPicked] = useState("");
  const server = joined.some((m) => m.server_id === picked) ? picked : (joined[0]?.server_id ?? "");
  const q = useDockerVolumes(server);
  const [creating, setCreating] = useState(false);
  const byId = new Map(servers.map((s) => [s.id, s]));
  const name = (id: string) => (byId.get(id) ? nodeName(byId.get(id) as Server) : id.slice(0, 8));

  return (
    <Panel
      title="Volumes"
      description="Local volumes live on one node; NFS or CIFS volumes follow a task to any node."
      actions={
        <div className="flex flex-wrap items-center gap-2">
          <Select value={server} onChange={(e) => setPicked(e.target.value)} aria-label="Node" className="w-auto" disabled={!joined.length}>
            {joined.length === 0 && <option value="">No joined nodes</option>}
            {joined.map((m) => (
              <option key={m.server_id} value={m.server_id}>
                {name(m.server_id)} · {m.role}
              </option>
            ))}
          </Select>
          <Button variant="primary" disabled={!server} onClick={() => setCreating(true)}>
            <Plus className="size-4" /> Create volume
          </Button>
        </div>
      }
    >
      <div className="flex gap-2 border-b border-line px-3 py-2 text-xs text-muted">
        <Info className="mt-0.5 size-3.5 shrink-0 text-accent" />
        <p>
          Stateful services (databases, queues) with a local volume must stay on the node that holds the data: pin them with a placement constraint such as{" "}
          <code className="font-mono text-[11px] select-all">node.hostname=={server ? name(server) : "<node-name>"}</code> or a node label such as{" "}
          <code className="font-mono text-[11px] select-all">node.labels.db==true</code> (set labels in Nodes). Use one replica, or an NFS/CIFS volume when the service may move.
        </p>
      </div>
      {!server ? (
        <Empty icon={<Database className="size-5" />} title="No joined nodes">
          Add a node to the swarm first.
        </Empty>
      ) : q.error ? (
        <div className="p-3">
          <ErrorNote error={q.error} />
        </div>
      ) : q.isLoading ? (
        <div className="p-3">
          <Skeleton rows={3} />
        </div>
      ) : !q.data?.length ? (
        <Empty
          icon={<Database className="size-5" />}
          title={`No volumes on ${name(server)}`}
          action={
            <Button variant="primary" onClick={() => setCreating(true)}>
              <Plus className="size-4" /> Create volume
            </Button>
          }
        >
          Stacks create their named volumes on deploy; create one here to pre-provision it or mount NFS/CIFS storage.
        </Empty>
      ) : (
        <Table head={["Name", "Driver", "Used by", "Created"]}>
          {q.data.map((v) => (
            <tr key={v.name} className="text-[13px]">
              <Td className="max-w-72">
                <div className="flex min-w-0 items-center gap-1.5">
                  <span className="truncate font-mono text-xs select-all" title={v.mountpoint ?? v.name}>
                    {v.name}
                  </span>
                  {v.managed && <Badge tone="muted">Forge</Badge>}
                  {v.labels["com.docker.stack.namespace"] && <Badge tone="accent">{v.labels["com.docker.stack.namespace"]}</Badge>}
                </div>
              </Td>
              <Td className="text-xs text-muted">
                {v.driver}
                {v.scope ? ` · ${v.scope}` : ""}
              </Td>
              <Td className="max-w-64 truncate text-xs text-muted" title={v.used_by.join(", ")}>
                {v.used_by.length ? v.used_by.join(", ") : "—"}
              </Td>
              <Td className="text-xs whitespace-nowrap text-muted">
                <span title={time(v.created)}>{v.created ? ago(v.created) : "—"}</span>
              </Td>
            </tr>
          ))}
        </Table>
      )}
      <div className="px-3 py-2 text-[11px] text-faint">
        Full Docker view of this node:{" "}
        {server && (
          <Link to="/nodes/$nodeId" params={{ nodeId: server }} className="text-accent hover:underline">
            {name(server)}
          </Link>
        )}
      </div>
      {creating && server && <CreateVolume server={server} serverName={name(server)} onClose={() => setCreating(false)} />}
    </Panel>
  );
}

type Kind = "local" | "nfs" | "cifs";

function CreateVolume({ server, serverName, onClose }: { server: string; serverName: string; onClose: () => void }) {
  const op = useDockerOp(server);
  const toast = useToast();
  const [f, setF] = useState({ kind: "local" as Kind, name: "", host: "", path: "", version: "4", share: "", username: "", password: "", read_only: false });
  const nameOk = VOL_RE.test(f.name);
  const remoteOk = f.kind === "local" || (f.host.trim() && (f.kind === "nfs" ? f.path.trim().startsWith("/") : f.share.trim()));
  const ok = nameOk && !!remoteOk;
  const remote = (): VolumeRemote | undefined =>
    f.kind === "nfs"
      ? { type: "nfs", server: f.host.trim(), path: f.path.trim(), version: f.version || undefined, read_only: f.read_only }
      : f.kind === "cifs"
        ? { type: "cifs", server: f.host.trim(), share: f.share.trim(), username: f.username || undefined, password: f.password || undefined, read_only: f.read_only }
        : undefined;

  return (
    <Modal open onOpenChange={(o) => !o && onClose()} title={`Create volume on ${serverName}`} description="Create it on every node a service may run on; a remote volume points each node at the same share.">
      <form
        className="space-y-3"
        onSubmit={(e) => {
          e.preventDefault();
          if (!ok) return;
          op.mutate(
            { op: "volume_create", name: f.name, labels: {}, remote: remote() },
            {
              onSuccess: () => {
                toast(`Created volume ${f.name} on ${serverName}`);
                onClose();
              },
              onError: (e) => toast(errorText(e), "error"),
            },
          );
        }}
      >
        <Field label="Name" hint={f.name && !nameOk ? <span className="text-bad">Letters, digits, . _ -</span> : undefined}>
          <Input value={f.name} onChange={(e) => setF({ ...f, name: e.target.value.trim() })} className="font-mono" placeholder="pgdata" autoFocus />
        </Field>
        <Segmented
          label="Storage"
          value={f.kind}
          onChange={(kind) => setF({ ...f, kind })}
          options={[
            { id: "local", label: "Local disk" },
            { id: "nfs", label: "NFS" },
            { id: "cifs", label: "CIFS / SMB" },
          ]}
        />
        {f.kind === "local" && <p className="text-xs text-muted">Data stays on {serverName}. Pin the service to this node.</p>}
        {f.kind !== "local" && (
          <>
            <Field label="Server" hint="Host name or IP of the file server, reachable from every node.">
              <Input value={f.host} onChange={(e) => setF({ ...f, host: e.target.value })} className="font-mono" placeholder="192.0.2.20" />
            </Field>
            {f.kind === "nfs" ? (
              <div className="grid grid-cols-[1fr_6rem] gap-2">
                <Field label="Export path">
                  <Input value={f.path} onChange={(e) => setF({ ...f, path: e.target.value })} className="font-mono" placeholder="/exports/pgdata" />
                </Field>
                <Field label="Version">
                  <Select value={f.version} onChange={(e) => setF({ ...f, version: e.target.value })} aria-label="NFS version">
                    <option value="4">4</option>
                    <option value="4.1">4.1</option>
                    <option value="3">3</option>
                  </Select>
                </Field>
              </div>
            ) : (
              <>
                <Field label="Share">
                  <Input value={f.share} onChange={(e) => setF({ ...f, share: e.target.value })} className="font-mono" placeholder="data" />
                </Field>
                <div className="grid grid-cols-2 gap-2">
                  <Field label="Username">
                    <Input value={f.username} onChange={(e) => setF({ ...f, username: e.target.value })} autoComplete="off" />
                  </Field>
                  <Field label="Password">
                    <Input type="password" value={f.password} onChange={(e) => setF({ ...f, password: e.target.value })} autoComplete="new-password" />
                  </Field>
                </div>
              </>
            )}
            <label className="flex items-center gap-2 text-[13px]">
              <input type="checkbox" checked={f.read_only} onChange={(e) => setF({ ...f, read_only: e.target.checked })} /> Read-only
            </label>
          </>
        )}
        <div className="flex justify-end gap-2 pt-2">
          <Button type="button" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" disabled={!ok || op.isPending}>
            {op.isPending && <Loader2 className="size-4 animate-spin" />} Create
          </Button>
        </div>
      </form>
    </Modal>
  );
}
