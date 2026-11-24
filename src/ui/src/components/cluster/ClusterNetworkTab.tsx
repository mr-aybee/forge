import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Globe, Loader2, Network, Pin, PinOff, RefreshCw, Rocket, Save, Trash2 } from "lucide-react";
import { useEffect, useState } from "react";

import { api, ApiError, get, post } from "../../api/client";
import { useClusters } from "../../api/hooks";
import { errorText, useToast } from "../../app/toast";
import { blankFabric, FabricForm, type FabricResponse, type FabricSpec, type Lease, type NodeSync, validSubnet } from "../fabric/FabricForm";
import { PAGE_SIZES, Pager, usePaged } from "../Pager";
import { Badge, Button, Empty, Field, Input, Modal, SectionBar, Panel, RowMenu, type RowAction, SearchInput, Skeleton, Status, Table, Td, useRowMenu } from "../ui";

/** A cluster's Forge network: subnet, mode, domain and the address plan. */
export function ClusterNetworkTab({ clusterId }: { clusterId: string }) {
  const cluster = useClusters().data?.find((c) => c.id === clusterId);
  const qc = useQueryClient();
  const toast = useToast();
  const q = useQuery({
    queryKey: ["fabric", clusterId],
    queryFn: () => get<FabricResponse>(`/api/v1/clusters/${clusterId}/fabric`),
    // Follow the reconciler while it works.
    refetchInterval: (query) => (query.state.data?.status?.applied_by ? 5_000 : false),
  });
  const [spec, setSpec] = useState<FabricSpec>(blankFabric());
  const [pins, setPins] = useState<Lease[]>([]);
  const [editing, setEditing] = useState(false);
  const [pinning, setPinning] = useState<Lease>();
  const [saving, setSaving] = useState(false);
  const [endpointFor, setEndpointFor] = useState<{ id: string; name: string }>();
  const rm = useRowMenu();
  const [applying, setApplying] = useState(false);
  const [removing, setRemoving] = useState(false);
  const [memberQ, setMemberQ] = useState("");

  const applyNetwork = async () => {
    setApplying(true);
    try {
      const r = await post<{ nodes: number }>(`/api/v1/clusters/${clusterId}/fabric/apply`, {});
      toast(`Applying the network to ${r.nodes} node${r.nodes === 1 ? "" : "s"}`);
      await qc.invalidateQueries({ queryKey: ["fabric", clusterId] });
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setApplying(false);
    }
  };
  const resync = async (node: string) => {
    try {
      await post(`/api/v1/clusters/${clusterId}/fabric/nodes/${node}/resync`, {});
      toast("Re-syncing the node");
      await qc.invalidateQueries({ queryKey: ["fabric", clusterId] });
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  useEffect(() => {
    if (q.data?.spec) {
      setSpec(q.data.spec);
      setPins(q.data.pins ?? []);
    }
  }, [q.data]);

  const save = async (nextPins = pins, nextSpec = spec) => {
    setSaving(true);
    try {
      await api("PUT", `/api/v1/clusters/${clusterId}/fabric`, { spec: nextSpec, pins: nextPins });
      toast("Network saved");
      setEditing(false);
      setPinning(undefined);
      await qc.invalidateQueries({ queryKey: ["fabric", clusterId] });
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setSaving(false);
    }
  };

  const data = q.data;
  const leases = data?.leases ?? [];
  const memberNeedle = memberQ.trim().toLowerCase();
  const memberRows = (data?.members ?? []).filter((m) => `${m.name} ${m.address} ${m.status}`.toLowerCase().includes(memberNeedle));
  const memberPage = usePaged(memberRows, { reset: memberNeedle });
  const memberName = (id: string) => data?.members.find((m) => m.id === id)?.name ?? id.slice(0, 8);
  const svc = (id: string) => data?.services.find((s) => s.id === id);
  const lease = (kind: Lease["kind"], owner: string) => leases.find((l) => l.kind === kind && l.owner === owner);
  const unpin = (l: Lease) => void save(pins.filter((p) => !(p.kind === l.kind && p.owner === l.owner)));
  const pinActions = (l: Lease | undefined): RowAction[] =>
    l
      ? [
          { label: l.pinned ? "Change fixed address" : "Fix this address", icon: Pin, onSelect: () => setPinning(l) },
          { label: "Release fixed address", icon: PinOff, hidden: !l.pinned, onSelect: () => unpin(l) },
        ]
      : [];

  return (
    <>
      <SectionBar
        description="Forge assigns every node, node block and service an address from the cluster subnet. Addresses stay the same as nodes and services come and go; fix any of them by hand."
        actions={
          data?.spec && (
            <>
              <Button onClick={() => setEditing(true)}>
                <Network className="size-4" /> Change network
              </Button>
              {data.spec.mode === "mesh" && (
                <Button variant="primary" disabled={applying || data.members.length === 0} title={data.members.length === 0 ? "Add nodes to the cluster first" : undefined} onClick={() => void applyNetwork()}>
                  <Rocket className="size-4" /> {data.status?.applied_by ? "Re-apply network" : "Apply network"}
                </Button>
              )}
              <Button variant="danger" onClick={() => setRemoving(true)}>
                <Trash2 className="size-4" /> Remove network
              </Button>
            </>
          )
        }
      />
      {q.isLoading || !data ? (
        <Skeleton />
      ) : !data.spec ? (
        <Panel title="Set up the cluster network">
          <div className="p-4">
            <FabricForm value={spec} onChange={setSpec} clusterId={clusterId} clusterName={cluster?.name ?? ""} />
          </div>
          <div className="flex items-center justify-end gap-3 border-t border-line bg-panel-2 px-4 py-3">
            {(!validSubnet(spec.subnet) || !spec.domain) && <span className="text-xs text-warn">{!validSubnet(spec.subnet) ? "Enter a valid subnet." : "Enter an internal domain."}</span>}
            <Button variant="primary" disabled={saving || !validSubnet(spec.subnet) || !spec.domain} onClick={() => void save([], spec)}>
              <Save className="size-4" /> Create network
            </Button>
          </div>
        </Panel>
      ) : (
        <div className="space-y-4">
          <div className="grid gap-3 rounded-lg border border-line bg-panel p-3 shadow-[var(--shadow-card)] sm:grid-cols-4">
            <Info label="Subnet" value={<span className="font-mono">{data.spec.subnet}</span>} />
            <Info label="Mode" value={<Badge tone="ok">encrypted mesh</Badge>} />
            <Info label="Internal domain" value={<span className="font-mono">{data.spec.domain}</span>} />
            <Info label="WireGuard port" value={<span className="font-mono">{data.spec.port}/udp</span>} />
          </div>

          <Panel
            title={`Nodes · ${data.members.length}`}
            actions={data.members.length > PAGE_SIZES[0] ? <SearchInput value={memberQ} onChange={setMemberQ} placeholder="Filter nodes" /> : undefined}
          >
            {data.members.length === 0 ? (
              <Empty icon={<Network className="size-5" />} title="No member nodes">
                Add nodes to the cluster; each gets an address and an instance block.
              </Empty>
            ) : (
              <>
              <Table head={["Node", "Real address", "Mesh address", "Instance block", "Status", ""]}>
                {memberPage.rows.map((m) => {
                  const a = lease("node", m.id);
                  const b = lease("block", m.id);
                  const st = data.status?.nodes?.[m.id];
                  const actions: RowAction[] = [
                    { label: "Re-sync now", icon: RefreshCw, hidden: !data.status?.applied_by, onSelect: () => void resync(m.id) },
                    { label: data.spec?.endpoints?.[m.id] ? "Change public endpoint" : "Set public endpoint", icon: Globe, onSelect: () => setEndpointFor({ id: m.id, name: m.name }) },
                    { label: a?.pinned ? "Change fixed mesh address" : "Fix mesh address", icon: Pin, hidden: !a, onSelect: () => a && setPinning(a) },
                    { label: b?.pinned ? "Change fixed block" : "Fix instance block", icon: Pin, hidden: !b, onSelect: () => b && setPinning(b) },
                    { label: "Release fixed mesh address", icon: PinOff, hidden: !a?.pinned, onSelect: () => a && unpin(a) },
                    { label: "Release fixed block", icon: PinOff, hidden: !b?.pinned, onSelect: () => b && unpin(b) },
                  ];
                  return (
                    <tr key={m.id} {...rm.rowProps(actions, `Actions for ${m.name}`)}>
                      <Td className="font-medium">{m.name}</Td>
                      <Td className="font-mono text-xs text-muted">{m.address}</Td>
                      <Td className="font-mono text-xs">
                        {a?.cidr.replace("/32", "")} {a?.pinned && <Pin className="ml-1 inline size-3 text-accent" />}
                      </Td>
                      <Td className="font-mono text-xs">
                        {b?.cidr} {b?.pinned && <Pin className="ml-1 inline size-3 text-accent" />}
                      </Td>
                      <Td>
                        <SyncState st={st} applied={!!data.status?.applied_by} online={m.status === "online"} names={memberName} />
                      </Td>
                      <Td className="text-right">
                        <RowMenu actions={actions} label={`Actions for ${m.name}`} />
                      </Td>
                    </tr>
                  );
                })}
                {!memberRows.length && (
                  <tr>
                    <Td colSpan={6} className="py-4 text-center text-muted">
                      No node matches “{memberQ}”.
                    </Td>
                  </tr>
                )}
              </Table>
              <Pager paged={memberPage} />
              </>
            )}
          </Panel>

          <Panel title={`Services · ${data.services.length}`}>
            {data.services.length === 0 ? (
              <Empty icon={<Network className="size-5" />} title="No services yet">
                Add a service to one of this cluster's environments; it gets a service IP and an internal name.
              </Empty>
            ) : (
              <Table head={["Service", "Environment", "Service IP", "Internal name", ""]}>
                {data.services.map((s) => {
                  const l = lease("service", s.id);
                  return (
                    <tr key={s.id} {...rm.rowProps(pinActions(l), `Actions for ${s.name}`)}>
                      <Td className="font-medium">{s.name}</Td>
                      <Td>
                        <Badge tone="accent">{s.environment}</Badge>
                      </Td>
                      <Td className="font-mono text-xs">
                        {l?.cidr.replace("/32", "")} {l?.pinned && <Pin className="ml-1 inline size-3 text-accent" />}
                      </Td>
                      <Td className="font-mono text-xs text-muted">
                        {s.name}.{s.environment}.{data.spec!.domain}
                      </Td>
                      <Td className="text-right">
                        <RowMenu actions={pinActions(l)} label={`Actions for ${s.name}`} />
                      </Td>
                    </tr>
                  );
                })}
              </Table>
            )}
          </Panel>
          {rm.menu}
          {data.spec.mode === "mesh" && !data.status?.applied_by && (
            <p className="rounded-md border border-accent/40 bg-accent-soft px-3 py-2 text-[13px]">
              This plan is not on the nodes yet. <b>Apply network</b> installs WireGuard on each member, creates its key on the node (only the public key is sent to Forge) and connects the mesh. Forge then keeps every node in sync and retries failures.
            </p>
          )}
          <p className="text-xs text-faint">Service IPs, the firewall between environments and internal DNS are applied in the next releases.</p>
        </div>
      )}

      <Modal open={editing} onOpenChange={setEditing} title="Change network" description="Changing the subnet re-assigns every address that is not fixed." width={960}>
        <FabricForm value={spec} onChange={setSpec} clusterId={clusterId} clusterName={cluster?.name ?? ""} />
        <div className="mt-4 flex justify-end gap-2">
          <Button variant="ghost" onClick={() => setEditing(false)}>
            Cancel
          </Button>
          <Button variant="primary" disabled={saving || !validSubnet(spec.subnet) || !spec.domain} onClick={() => void save(spec.subnet === data?.spec?.subnet ? pins : [], spec)}>
            Save
          </Button>
        </div>
      </Modal>

      {endpointFor && data?.spec && (
        <EndpointDialog
          name={endpointFor.name}
          value={data.spec.endpoints?.[endpointFor.id] ?? ""}
          saving={saving}
          onClose={() => setEndpointFor(undefined)}
          onSave={(v) => {
            const endpoints = { ...(data.spec!.endpoints ?? {}) };
            if (v) endpoints[endpointFor.id] = v;
            else delete endpoints[endpointFor.id];
            const next = { ...data.spec!, endpoints };
            setSpec(next);
            void save(pins, next).then(() => setEndpointFor(undefined));
          }}
        />
      )}
      {removing && data?.spec && (
        <RemoveNetworkDialog
          clusterId={clusterId}
          clusterName={cluster?.name ?? ""}
          data={data}
          onClose={() => setRemoving(false)}
          onRemoved={() => {
            setRemoving(false);
            setEditing(false);
            setSpec(blankFabric());
            setPins([]);
          }}
        />
      )}
      {pinning && (
        <PinDialog
          lease={pinning}
          title={pinning.kind === "service" ? (svc(pinning.owner)?.name ?? "service") : memberName(pinning.owner)}
          saving={saving}
          onClose={() => setPinning(undefined)}
          onSave={(cidr) => void save([...pins.filter((p) => !(p.kind === pinning.kind && p.owner === pinning.owner)), { kind: pinning.kind, owner: pinning.owner, cidr, pinned: true }])}
        />
      )}
    </>
  );
}

function Info({ label, value }: { label: string; value: React.ReactNode }) {
  return (
    <div>
      <div className="text-[11px] tracking-wide text-faint uppercase">{label}</div>
      <div className="mt-0.5 text-[13px]">{value}</div>
    </div>
  );
}

function PinDialog({ lease, title, saving, onClose, onSave }: { lease: Lease; title: string; saving: boolean; onClose: () => void; onSave: (cidr: string) => void }) {
  const host = lease.kind !== "block";
  const [v, setV] = useState(host ? lease.cidr.replace("/32", "") : lease.cidr);
  const what = lease.kind === "node" ? "mesh address" : lease.kind === "block" ? "instance block" : "service IP";
  return (
    <Modal open onOpenChange={(o) => !o && onClose()} title={`Fix the ${what} of ${title}`} description="A fixed address is never moved by Forge. It must be inside its range of the cluster subnet and not used by anything else.">
      <form
        className="space-y-4"
        onSubmit={(e) => {
          e.preventDefault();
          onSave(host ? `${v.trim()}/32` : v.trim());
        }}
      >
        <Field label={host ? "Address" : "Block (CIDR)"}>
          <Input className="font-mono" autoFocus value={v} onChange={(e) => setV(e.target.value)} />
        </Field>
        <div className="flex justify-end">
          <Button variant="primary" disabled={saving}>
            Fix address
          </Button>
        </div>
      </form>
    </Modal>
  );
}

const STATE: Record<string, { label: string; tone: string }> = {
  ok: { label: "in sync", tone: "online" },
  waiting: { label: "waiting for peers", tone: "running" },
  pending: { label: "pending", tone: "pending" },
  retrying: { label: "retrying", tone: "running" },
  failed: { label: "needs attention", tone: "failed" },
};

function SyncState({ st, applied, online, names }: { st?: NodeSync; applied: boolean; online: boolean; names: (id: string) => string }) {
  if (!applied) return <span className="text-xs text-faint">not applied</span>;
  if (!st) return <span className="text-xs text-muted">{online ? "starting…" : "waiting for the node to come online"}</span>;
  const s = STATE[st.state] ?? { label: st.state, tone: "pending" };
  const wait = st.next_at && st.state !== "ok" ? Math.max(0, Math.round((st.next_at - Date.now()) / 1000)) : 0;
  return (
    <div className="space-y-0.5 text-xs">
      <div className="flex items-center gap-1.5">
        <Status value={s.tone} />
        <span>{s.label}</span>
        {st.step && st.state !== "ok" && <span className="text-faint">· {st.step}</span>}
        {st.relay && <Badge tone="accent">relay</Badge>}
      </div>
      {st.error && (
        <div className="max-w-[320px] truncate text-bad" title={st.error}>
          {st.error}
        </div>
      )}
      {st.state === "retrying" && wait > 0 && (
        <div className="text-faint">
          attempt {st.attempts} · next in {wait < 60 ? `${wait}s` : `${Math.round(wait / 60)} min`}
        </div>
      )}
      {!!st.unreachable?.length && <div className="text-warn">no path yet to {st.unreachable.map(names).join(", ")}</div>}
    </div>
  );
}

function EndpointDialog({ name, value, saving, onClose, onSave }: { name: string; value: string; saving: boolean; onClose: () => void; onSave: (v: string) => void }) {
  const [v, setV] = useState(value);
  const ok = !v || /^[a-z0-9.-]+:\d{1,5}$/i.test(v.trim());
  return (
    <Modal open onOpenChange={(o) => !o && onClose()} title={`Public endpoint of ${name}`} description="Set this when the node is behind NAT but reachable through a port forward (for example on a home router). Leave empty to let Forge decide.">
      <form
        className="space-y-4"
        onSubmit={(e) => {
          e.preventDefault();
          onSave(v.trim());
        }}
      >
        <Field label="host:port" hint="The router forwards this UDP port to the node's WireGuard port.">
          <Input className="font-mono" autoFocus value={v} onChange={(e) => setV(e.target.value)} placeholder="home.example.net:51820" />
        </Field>
        {!ok && <p className="text-xs text-bad">Use host:port, e.g. 203.0.113.7:51820.</p>}
        <div className="flex justify-end gap-2">
          {value && (
            <Button type="button" variant="ghost" onClick={() => onSave("")}>
              Clear
            </Button>
          )}
          <Button variant="primary" disabled={saving || !ok}>
            Save
          </Button>
        </div>
      </form>
    </Modal>
  );
}

/**
 * Removes the cluster network (`DELETE /clusters/{id}/fabric`). The server
 * asks for an MFA-verified session when needed; the API client handles that
 * prompt and retries once.
 */
function RemoveNetworkDialog({ clusterId, clusterName, data, onClose, onRemoved }: { clusterId: string; clusterName: string; data: FabricResponse; onClose: () => void; onRemoved: () => void }) {
  const qc = useQueryClient();
  const toast = useToast();
  const [typed, setTyped] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const word = clusterName || "remove";
  const members = data.members ?? [];
  const services = data.services ?? [];
  const applied = !!data.status?.applied_by;
  const offline = members.filter((m) => m.status !== "online");

  const remove = async () => {
    setBusy(true);
    setError(undefined);
    try {
      const r = await api<{ not_cleaned?: string[] } | undefined>("DELETE", `/api/v1/clusters/${clusterId}/fabric`);
      const left = Array.isArray(r?.not_cleaned) ? r.not_cleaned : [];
      toast(left.length ? `Network removed; remove the interface by hand on ${left.join(", ")}` : "Network removed", left.length ? "error" : undefined);
      await qc.invalidateQueries({ queryKey: ["fabric", clusterId] });
      onRemoved();
    } catch (e) {
      const msg = e instanceof ApiError && e.code === "step_up_required" ? `Not verified: ${e.message}` : errorText(e);
      setError(msg);
      toast(msg, "error");
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal open onOpenChange={(o) => !o && !busy && onClose()} title={`Remove the network of ${clusterName || "this cluster"}?`} description="This deletes the address plan. Creating a network again assigns new addresses.">
      <div className="space-y-3 text-[13px]">
        <ul className="space-y-1 text-muted">
          {applied ? (
            <li>
              • Removes the fabric interface (WireGuard) from {members.length ? <b className="text-fg">{members.length} node{members.length === 1 ? "" : "s"}</b> : "every node"}. Nodes stop reaching each other over the mesh.
            </li>
          ) : (
            <li>• The network was never applied, so no node has a fabric interface to remove.</li>
          )}
          {applied && offline.length > 0 && (
            <li>
              • <span className="text-warn">Offline now:</span> {offline.map((m) => m.name).join(", ")}. These keep a dead interface until you remove it by hand.
            </li>
          )}
          <li>
            • <b className="text-fg">{services.length}</b> service{services.length === 1 ? "" : "s"} lose{services.length === 1 ? "s" : ""} the mesh address and internal name under <span className="font-mono">{data.spec?.domain ?? "—"}</span>.
          </li>
          <li>• Fixed addresses and public endpoints are deleted with the plan.</li>
        </ul>
        <Field label={`Type ${word} to confirm`}>
          <Input value={typed} onChange={(e) => setTyped(e.target.value)} autoComplete="off" spellCheck={false} aria-label="Confirmation" autoFocus />
        </Field>
        {error && <p className="text-xs break-words text-bad">{error}</p>}
        <div className="flex justify-end gap-2">
          <Button variant="ghost" onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button variant="danger" disabled={typed.trim() !== word || busy} onClick={() => void remove()}>
            {busy ? <Loader2 className="size-4 animate-spin" /> : <Trash2 className="size-4" />}
            Remove network
          </Button>
        </div>
      </div>
    </Modal>
  );
}
