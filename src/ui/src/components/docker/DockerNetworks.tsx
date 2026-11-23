import { Eraser, Link2, Lock, Network, Plus, Search, Trash2, Unlink } from "lucide-react";
import { useMemo, useState } from "react";

import { type ContainerSummary, useContainerQuery } from "../../api/hooks";
import { type DockerNetwork, type NetworkDriver, useDockerNetworks } from "../../api/dockerResources";
import { Badge, Button, Confirm, CopyIcon, Empty, ErrorNote, Field, Input, Modal, RowMenu, type RowAction, Select, Skeleton, Switch, Table, Td, TextArea, useRowMenu } from "../ui";
import { parseLabels } from "./DockerVolumes";
import { Check, DockerProblem, type DockerRunner, InspectDrawer, Spin, Toolbar } from "./shared";

const LOCKED = "Docker's or Forge's own network: it cannot be removed or rewired here";

export function DockerNetworks({ server, runner }: { server: string; runner: DockerRunner }) {
  const q = useDockerNetworks(server);
  const [search, setSearch] = useState("");
  const [picked, setPicked] = useState<Set<string>>(() => new Set());
  const [dialog, setDialog] = useState<
    | { kind: "create" }
    | { kind: "prune" }
    | { kind: "bulk" }
    | { kind: "remove"; n: DockerNetwork }
    | { kind: "inspect"; n: DockerNetwork }
    | { kind: "connect" | "disconnect"; n: DockerNetwork }
    | null
  >(null);
  const rm = useRowMenu();
  const all = useMemo(() => q.data ?? [], [q.data]);
  const rows = useMemo(() => {
    const s = search.trim().toLowerCase();
    return all.filter((n) => !s || `${n.name} ${n.id} ${n.driver} ${n.subnets.join(" ")}`.toLowerCase().includes(s));
  }, [all, search]);
  const removable = (n: DockerNetwork) => !n.protected && n.containers === 0;
  const selectable = rows.filter(removable);
  const sel = rows.filter((n) => picked.has(n.id));
  const close = () => setDialog(null);

  return (
    <div>
      <Toolbar>
        <label className="flex min-w-0 flex-1 items-center gap-2">
          <Search className="size-3.5 shrink-0 text-faint" />
          <input data-bare className="min-w-0 flex-1 bg-transparent text-[13px] outline-none" placeholder="Filter by name, id, driver or subnet" aria-label="Filter networks" value={search} onChange={(e) => setSearch(e.target.value)} />
        </label>
        {sel.length > 0 && (
          <Button variant="danger" disabled={runner.bulk} onClick={() => setDialog({ kind: "bulk" })}>
            <Spin on={runner.bulk}>
              <Trash2 className="size-4" />
            </Spin>{" "}
            Remove {sel.length}
          </Button>
        )}
        <Button onClick={() => setDialog({ kind: "prune" })}>
          <Eraser className="size-4" /> Prune
        </Button>
        <Button variant="primary" onClick={() => setDialog({ kind: "create" })}>
          <Plus className="size-4" /> Create network
        </Button>
      </Toolbar>
      {q.error ? (
        <DockerProblem error={q.error} server={server} />
      ) : q.isLoading ? (
        <Skeleton rows={4} />
      ) : rows.length === 0 ? (
        <Empty icon={<Network className="size-5" />} title={all.length ? "No network matches" : "No networks"}>
          {all.length ? "Change the search." : "Create a bridge network to let containers reach each other by name."}
        </Empty>
      ) : (
        <Table
          head={[
            <Check
              key="all"
              label="Select every removable network shown"
              checked={selectable.length > 0 && selectable.every((n) => picked.has(n.id))}
              disabled={selectable.length === 0}
              onChange={(on) => setPicked(on ? new Set(selectable.map((n) => n.id)) : new Set())}
            />,
            "Name",
            "Driver",
            "Subnets",
            "Flags",
            "Containers",
            "",
          ]}
        >
          {rows.map((n) => {
            const hint = n.protected ? LOCKED : "Another Docker action is running";
            const actions: RowAction[] = [
              { label: "Inspect", icon: Search, tone: "accent", onSelect: () => setDialog({ kind: "inspect", n }) },
              { label: "Connect container…", icon: Link2, disabled: n.protected, hint: LOCKED, onSelect: () => setDialog({ kind: "connect", n }) },
              { label: "Disconnect container…", icon: Unlink, disabled: n.protected || n.containers === 0, hint: n.protected ? LOCKED : "No container is attached", onSelect: () => setDialog({ kind: "disconnect", n }) },
              { label: "Remove…", icon: Trash2, danger: true, disabled: n.protected || runner.pending, hint, onSelect: () => setDialog({ kind: "remove", n }) },
            ];
            return (
              <tr key={n.id || n.name} className="hover:bg-panel-2/60" {...rm.rowProps(actions, `Actions for ${n.name}`)}>
                <Td className="w-8">
                  <Check
                    label={`Select ${n.name}`}
                    checked={picked.has(n.id)}
                    disabled={!removable(n)}
                    title={n.protected ? LOCKED : n.containers > 0 ? "Containers are attached" : undefined}
                    onChange={(on) => setPicked((p) => { const s = new Set(p); if (on) s.add(n.id); else s.delete(n.id); return s; })}
                  />
                </Td>
                <Td className="max-w-[280px]">
                  <div className="flex min-w-0 items-center gap-1.5">
                    <span className="truncate font-mono text-[12px]" title={n.name}>
                      {n.name}
                    </span>
                    {n.protected && (
                      <span title={LOCKED} className="inline-flex">
                        <Badge>
                          <Lock className="inline size-3" aria-label="Protected" /> protected
                        </Badge>
                      </span>
                    )}
                  </div>
                  <div className="flex items-center font-mono text-[10px] text-faint tabular-nums">
                    <span className="select-all">{n.id || "—"}</span>
                    {n.id && <CopyIcon text={n.id} label="Copy network id" />}
                  </div>
                </Td>
                <Td className="text-xs text-muted">
                  {n.driver || "—"}
                  {n.scope && <span className="text-faint"> · {n.scope}</span>}
                </Td>
                <Td className="max-w-[200px] truncate font-mono text-[11px] text-muted" title={n.subnets.join("\n")}>
                  {n.subnets.length ? n.subnets.join(", ") : "—"}
                </Td>
                <Td>
                  <div className="flex gap-1">
                    {n.internal && <Badge tone="warn">internal</Badge>}
                    {n.attachable && <Badge>attachable</Badge>}
                    {!n.internal && !n.attachable && <span className="text-faint">—</span>}
                  </div>
                </Td>
                <Td className="font-mono text-[11px] text-muted tabular-nums">{n.containers}</Td>
                <Td className="text-right">
                  <RowMenu actions={actions} label={`Actions for ${n.name}`} />
                </Td>
              </tr>
            );
          })}
        </Table>
      )}
      {rm.menu}

      {dialog?.kind === "inspect" && <InspectDrawer server={server} op={{ op: "network_inspect", id: dialog.n.id || dialog.n.name }} title={`Inspect · ${dialog.n.name}`} onClose={close} />}
      {dialog?.kind === "create" && <CreateNetwork runner={runner} onClose={close} />}
      {(dialog?.kind === "connect" || dialog?.kind === "disconnect") && <WireDialog server={server} n={dialog.n} mode={dialog.kind} runner={runner} onClose={close} />}
      {dialog?.kind === "remove" && (
        <Confirm
          open
          onOpenChange={(o) => !o && close()}
          title={`Remove network ${dialog.n.name}?`}
          busy={runner.pending}
          body={
            dialog.n.containers > 0
              ? `${dialog.n.containers} container${dialog.n.containers === 1 ? " is" : "s are"} attached; Docker refuses until they are disconnected.`
              : "No container is attached. Containers created later with this network name fail to start until it exists again."
          }
          confirmLabel="Remove"
          onConfirm={() => void runner.run({ op: "network_remove", id: dialog.n.id || dialog.n.name }, `Removed network ${dialog.n.name}`).then((ok) => ok && close())}
        />
      )}
      {dialog?.kind === "prune" && (
        <Confirm
          open
          onOpenChange={(o) => !o && close()}
          title="Prune networks?"
          busy={runner.pending}
          body={
            <div className="space-y-2">
              <p>Docker deletes every custom network no container is attached to.</p>
              <p>Docker's own networks (bridge, host, none, ingress) and Forge's networks stay. The prune runs as a job.</p>
            </div>
          }
          confirmLabel="Prune networks"
          onConfirm={() => void runner.run({ op: "network_prune" }, "Network prune").then((ok) => ok && close())}
        />
      )}
      {dialog?.kind === "bulk" && (
        <Confirm
          open
          onOpenChange={(o) => !o && close()}
          title={`Remove ${sel.length} networks?`}
          body={
            <>
              <p>None of these has containers attached.</p>
              <ul className="mt-2 max-h-40 overflow-auto font-mono text-[11px]">
                {sel.map((n) => (
                  <li key={n.id} className="truncate" title={n.name}>
                    {n.name}
                  </li>
                ))}
              </ul>
            </>
          }
          confirmLabel={`Remove ${sel.length}`}
          onConfirm={() => {
            const ops = sel.map((n) => ({ op: "network_remove" as const, id: n.id }));
            close();
            void runner.runMany(ops, "networks", () => setPicked(new Set()));
          }}
        />
      )}
    </div>
  );
}

const CIDR = "\\d{1,3}(\\.\\d{1,3}){3}/\\d{1,2}|[0-9A-Fa-f:]+/\\d{1,3}";
const IP = "\\d{1,3}(\\.\\d{1,3}){3}|[0-9A-Fa-f:]+";

function CreateNetwork({ runner, onClose }: { runner: DockerRunner; onClose: () => void }) {
  const [f, setF] = useState({ name: "", driver: "bridge" as NetworkDriver, subnet: "", gateway: "", internal: false, attachable: false, labels: "" });
  const parsed = parseLabels(f.labels);
  const gatewayWithoutSubnet = !!f.gateway.trim() && !f.subnet.trim();
  return (
    <Modal open onOpenChange={(o) => !o && onClose()} title="Create network" description="A user-defined network on this node. Overlay networks need the node to be a Swarm manager.">
      <form
        className="space-y-4"
        onSubmit={(e) => {
          e.preventDefault();
          if (parsed.error || gatewayWithoutSubnet) return;
          const name = f.name.trim();
          void runner
            .run(
              {
                op: "network_create",
                name,
                driver: f.driver,
                subnet: f.subnet.trim() || undefined,
                gateway: f.gateway.trim() || undefined,
                internal: f.internal,
                attachable: f.driver === "overlay" && f.attachable,
                labels: parsed.labels,
              },
              `Created network ${name}`,
            )
            .then((ok) => ok && onClose());
        }}
      >
        <div className="grid grid-cols-[1fr_140px] gap-2">
          <Field label="Name" hint="Not bridge, host, none, ingress or forge-*">
            <Input autoFocus required pattern="[A-Za-z0-9][A-Za-z0-9_.\-]*" className="font-mono" value={f.name} onChange={(e) => setF({ ...f, name: e.target.value })} placeholder="app-net" />
          </Field>
          <Field label="Driver">
            <Select value={f.driver} onChange={(e) => setF({ ...f, driver: e.target.value as NetworkDriver })} aria-label="Driver">
              <option value="bridge">bridge</option>
              <option value="overlay">overlay</option>
            </Select>
          </Field>
        </div>
        <div className="grid grid-cols-2 gap-2">
          <Field label="Subnet" hint="CIDR; optional, Docker picks one">
            <Input pattern={CIDR} className="font-mono" value={f.subnet} onChange={(e) => setF({ ...f, subnet: e.target.value })} placeholder="10.20.0.0/24" />
          </Field>
          <Field label="Gateway" hint={gatewayWithoutSubnet ? <span className="text-bad">Set a subnet with a gateway</span> : "Optional, inside the subnet"}>
            <Input pattern={IP} className="font-mono" value={f.gateway} onChange={(e) => setF({ ...f, gateway: e.target.value })} placeholder="10.20.0.1" />
          </Field>
        </div>
        <div className="flex flex-col gap-2">
          <Switch checked={f.internal} onChange={(v) => setF({ ...f, internal: v })} label="Internal: no route to outside networks" />
          <Switch
            checked={f.driver === "overlay" && f.attachable}
            disabled={f.driver !== "overlay"}
            title={f.driver !== "overlay" ? "Only overlay networks are attachable" : undefined}
            onChange={(v) => setF({ ...f, attachable: v })}
            label="Attachable: standalone containers can join (overlay only)"
          />
        </div>
        <Field label="Labels" hint={parsed.error ? <span className="text-bad">{parsed.error}</span> : "One key=value per line; optional."}>
          <TextArea rows={2} value={f.labels} onChange={(e) => setF({ ...f, labels: e.target.value })} placeholder="team=payments" />
        </Field>
        <div className="flex justify-end gap-2">
          <Button type="button" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" disabled={runner.pending || !f.name.trim() || !!parsed.error || gatewayWithoutSubnet}>
            <Spin on={runner.pending}>
              <Plus className="size-4" />
            </Spin>{" "}
            Create
          </Button>
        </div>
      </form>
    </Modal>
  );
}

function WireDialog({ server, n, mode, runner, onClose }: { server: string; n: DockerNetwork; mode: "connect" | "disconnect"; runner: DockerRunner; onClose: () => void }) {
  const list = useContainerQuery<ContainerSummary[]>(server, "list");
  const [f, setF] = useState({ container: "", force: false });
  const containers = Array.isArray(list.data?.result) ? list.data.result : [];
  const connect = mode === "connect";
  return (
    <Modal
      open
      onOpenChange={(o) => !o && onClose()}
      title={`${connect ? "Connect a container to" : "Disconnect a container from"} ${n.name}`}
      description={connect ? "The container joins the network now; other containers on it reach it by name." : "The container loses its address on this network now; connections through it drop."}
    >
      <form
        className="space-y-4"
        onSubmit={(e) => {
          e.preventDefault();
          const op = connect
            ? ({ op: "network_connect", network: n.id || n.name, container: f.container } as const)
            : ({ op: "network_disconnect", network: n.id || n.name, container: f.container, force: f.force } as const);
          void runner.run(op, `${connect ? "Connected" : "Disconnected"} ${f.container}`).then((ok) => ok && onClose());
        }}
      >
        <ErrorNote error={list.error ?? (list.data?.status === "failed" ? list.data.error ?? "Listing the containers failed" : undefined)} />
        <Field label="Container">
          {list.isLoading ? (
            <Skeleton rows={1} />
          ) : (
            <Select value={f.container} onChange={(e) => setF({ ...f, container: e.target.value })} aria-label="Container" required>
              <option value="">Choose a container</option>
              {containers.map((c) => (
                <option key={c.id} value={c.name}>
                  {c.name} · {c.state}
                </option>
              ))}
            </Select>
          )}
        </Field>
        {!connect && <Switch checked={f.force} onChange={(v) => setF({ ...f, force: v })} label="Force: disconnect even if the container is not running" />}
        <div className="flex justify-end gap-2">
          <Button type="button" onClick={onClose}>
            Cancel
          </Button>
          <Button variant={connect ? "primary" : "danger"} disabled={runner.pending || !f.container}>
            <Spin on={runner.pending}>{connect ? <Link2 className="size-4" /> : <Unlink className="size-4" />}</Spin> {connect ? "Connect" : "Disconnect"}
          </Button>
        </div>
      </form>
    </Modal>
  );
}
