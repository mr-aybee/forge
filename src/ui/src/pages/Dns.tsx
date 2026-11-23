import { Copy, Globe, Network, Pencil, Plus, RefreshCw, Search, Trash2, X } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";

import { post } from "../api/client";
import { useInstallations, useServers } from "../api/hooks";
import { nodeName } from "../api/types";
import { LocalNamesPanel } from "../components/LocalNamesPanel";
import { errorText, useToast } from "../app/toast";
import { Badge, Button, Empty, Field, IconButton, Input, Modal, PageHeader, Panel, Select, Skeleton } from "../components/ui";
import { COPY_BLOCKED, copyText } from "../lib/clipboard";

interface Rec {
  name: string;
  type: string;
  ttl: number;
  contents: string[];
}
interface Zone {
  name: string;
  serial: number;
}
interface NodeResult<T> {
  status: "succeeded" | "failed" | "running";
  result?: T;
  error?: string;
}

/** Per-type help so the form says what a value looks like. */
const TYPES: Record<string, { hint: string; placeholder: string; target?: boolean; priority?: boolean; tone: string }> = {
  A: { hint: "IPv4 address", placeholder: "203.0.113.10", tone: "text-accent" },
  AAAA: { hint: "IPv6 address", placeholder: "2001:db8::10", tone: "text-accent" },
  CNAME: { hint: "Another hostname (alias). Only one value.", placeholder: "app.example.com", target: true, tone: "text-ok" },
  TXT: { hint: "Text; quotes are added for you", placeholder: "v=spf1 include:_spf.example.com ~all", tone: "text-warn" },
  MX: { hint: "Mail server hostname", placeholder: "mail.example.com", target: true, priority: true, tone: "text-ok" },
  NS: { hint: "Name server hostname", placeholder: "ns1.example.com", target: true, tone: "text-muted" },
  SRV: { hint: "weight port target, e.g. 5 5060 sip.example.com", placeholder: "5 5060 sip.example.com", priority: true, tone: "text-muted" },
  CAA: { hint: "flags tag value", placeholder: '0 issue "letsencrypt.org"', tone: "text-muted" },
};
const EDITABLE = Object.keys(TYPES);
const TTLS = [
  [60, "1 min"],
  [300, "5 min"],
  [3600, "1 hour"],
  [86400, "1 day"],
] as const;

const ttlText = (s: number) => (s % 86400 === 0 ? `${s / 86400}d` : s % 3600 === 0 ? `${s / 3600}h` : s % 60 === 0 ? `${s / 60}m` : `${s}s`);
const dot = (h: string) => (h.endsWith(".") ? h : `${h}.`);

/** Turns what the user typed into what PowerDNS stores. */
function toWire(type: string, value: string, priority: number): string {
  let v = value.trim();
  if (type === "TXT" && !v.startsWith('"')) v = `"${v.replace(/"/g, '\\"')}"`;
  if (type === "SRV") {
    const p = v.split(/\s+/);
    if (p.length === 3) p[2] = dot(p[2] ?? "");
    v = p.join(" ");
  }
  if (TYPES[type]?.target) v = dot(v);
  return TYPES[type]?.priority ? `${priority} ${v}` : v;
}
/** And back, for editing. */
function fromWire(type: string, content: string): { value: string; priority: number } {
  let v = content;
  let priority = 10;
  if (TYPES[type]?.priority) {
    const [p, ...rest] = v.split(" ");
    priority = Number(p) || 0;
    v = rest.join(" ");
  }
  if (type === "TXT" && v.startsWith('"') && v.endsWith('"')) v = v.slice(1, -1).replace(/\\"/g, '"');
  if (TYPES[type]?.target && v.endsWith(".")) v = v.slice(0, -1);
  return { value: v, priority };
}

/** "shop.example.com." in zone "example.com." → "shop"; the apex → "@". */
const relName = (name: string, zone: string) => (name === zone ? "@" : name.endsWith(`.${zone}`) ? name.slice(0, -zone.length - 1) : name);

/** Zones and records on PowerDNS nodes (App Store → PowerDNS). */
export function Dns() {
  const toast = useToast();
  const nodes = useServers().data ?? [];
  const installs = useInstallations().data ?? [];
  const dnsNodes = nodes.filter((n) => installs.some((i) => i.server_id === n.id && i.package === "powerdns" && i.status === "installed"));
  const [server, setServer] = useState("");
  const [zones, setZones] = useState<Zone[]>();
  const [zone, setZone] = useState("");
  const [records, setRecords] = useState<Rec[]>();
  const [zq, setZq] = useState("");
  const [q, setQ] = useState("");
  const [typeFilter, setTypeFilter] = useState("all");
  const [editing, setEditing] = useState<Rec | "new">();
  const [addZone, setAddZone] = useState(false);
  const [dropZone, setDropZone] = useState(false);
  const [dropRec, setDropRec] = useState<Rec>();
  const node = server || dnsNodes[0]?.id || "";

  const call = useCallback(
    async <T,>(op: Record<string, unknown>): Promise<T | undefined> => {
      try {
        const r = await post<NodeResult<T>>(`/api/v1/servers/${node}/dns`, op);
        if (r.status !== "succeeded") {
          toast(r.error ?? "DNS operation did not finish", "error");
          return undefined;
        }
        return (r.result ?? true) as T;
      } catch (e) {
        toast(errorText(e), "error");
        return undefined;
      }
    },
    [node, toast],
  );

  const loadZones = useCallback(async () => {
    if (!node) return;
    const z = await call<Zone[]>({ op: "zones" });
    if (z) {
      setZones(z);
      setZone((cur) => (z.some((x) => x.name === cur) ? cur : (z[0]?.name ?? "")));
    }
  }, [node, call]);
  const loadRecords = useCallback(async () => {
    if (!node || !zone) return setRecords([]);
    setRecords(undefined);
    const r = await call<{ records: Rec[] }>({ op: "records", zone });
    setRecords(r?.records ?? []);
  }, [node, zone, call]);

  useEffect(() => void loadZones(), [loadZones]);
  useEffect(() => void loadRecords(), [loadRecords]);

  const shownZones = (zones ?? []).filter((z) => z.name.includes(zq.toLowerCase()));
  const counts = useMemo(() => {
    const c: Record<string, number> = {};
    for (const r of records ?? []) c[r.type] = (c[r.type] ?? 0) + r.contents.length;
    return c;
  }, [records]);
  const rows = useMemo(() => {
    const needle = q.trim().toLowerCase();
    return (records ?? [])
      .filter((r) => typeFilter === "all" || r.type === typeFilter)
      .filter((r) => !needle || r.name.toLowerCase().includes(needle) || r.contents.some((c) => c.toLowerCase().includes(needle)))
      .sort((a, b) => Number(a.name !== zone) - Number(b.name !== zone) || a.name.localeCompare(b.name) || a.type.localeCompare(b.type));
  }, [records, typeFilter, q, zone]);

  const bind = () => (records ?? []).flatMap((r) => r.contents.map((c) => `${r.name}\t${r.ttl}\tIN\t${r.type}\t${c}`)).join("\n");

  if (dnsNodes.length === 0) {
    return (
      <>
        <PageHeader eyebrow="Infrastructure" title="DNS" description="Local names on every node, and authoritative zones on your PowerDNS nodes." />
        <LocalNamesPanel />
        <Panel title="No DNS server yet" className="mt-4">
          <Empty icon={<Network className="size-5" />} title="Install PowerDNS on a node">
            App Store → PowerDNS. Its API key stays on that node; Forge talks to it through the node's helper.
          </Empty>
        </Panel>
      </>
    );
  }

  return (
    <>
      <PageHeader
        eyebrow="Infrastructure"
        title="DNS"
        description="Authoritative zones on your PowerDNS nodes. Ingress endpoints can publish their records here automatically."
        actions={
          <div className="flex items-center gap-2">
            {dnsNodes.length > 1 && (
              <label className="flex items-center gap-2 text-xs text-muted">
                Server
                <Select
                  className="w-auto py-1"
                  value={node}
                  onChange={(e) => {
                    setServer(e.target.value);
                    setZones(undefined);
                    setZone("");
                  }}
                >
                  {dnsNodes.map((n) => (
                    <option key={n.id} value={n.id}>
                      {nodeName(n)}
                    </option>
                  ))}
                </Select>
              </label>
            )}
            <Button variant="primary" onClick={() => setAddZone(true)}>
              <Plus className="size-4" /> New zone
            </Button>
          </div>
        }
      />
      <LocalNamesPanel />
      <div className="mt-4" />

      <div className="grid gap-4 lg:grid-cols-[280px_minmax(0,1fr)]">
        {/* Zones */}
        <Panel
          title={`Zones${zones ? ` (${zones.length})` : ""}`}
          actions={
            <IconButton label="Refresh zones" onClick={() => void loadZones()}>
              <RefreshCw className="size-4" />
            </IconButton>
          }
        >
          <div className="border-b border-line p-2">
            <div className="relative">
              <Search className="pointer-events-none absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-faint" />
              <Input className="py-1 pl-8" value={zq} onChange={(e) => setZq(e.target.value)} placeholder="Filter zones" />
            </div>
          </div>
          {!zones ? (
            <Skeleton rows={3} />
          ) : (
            <ul className="max-h-[60vh] overflow-y-auto p-1.5">
              {shownZones.map((z) => (
                <li key={z.name}>
                  <button
                    className={`flex w-full items-center gap-2.5 rounded-md px-2.5 py-2 text-left ${zone === z.name ? "bg-accent-soft text-accent" : "hover:bg-panel-2"}`}
                    onClick={() => setZone(z.name)}
                  >
                    <Globe className="size-4 shrink-0 opacity-70" />
                    <span className="min-w-0 flex-1">
                      <span className="block truncate text-[13px] font-medium">{z.name.replace(/\.$/, "")}</span>
                      <span className="block text-[11px] text-faint">serial {z.serial}</span>
                    </span>
                  </button>
                </li>
              ))}
              {shownZones.length === 0 && (
                <li className="px-3 py-6 text-center text-[13px] text-muted">
                  {zones.length === 0 ? (
                    <>
                      No zones yet.{" "}
                      <button className="text-accent hover:underline" onClick={() => setAddZone(true)}>
                        Create one
                      </button>
                    </>
                  ) : (
                    "No match."
                  )}
                </li>
              )}
            </ul>
          )}
        </Panel>

        {/* Records */}
        <Panel
          title={zone ? zone.replace(/\.$/, "") : "Records"}
          description={zone ? `${(records ?? []).reduce((n, r) => n + r.contents.length, 0)} records` : undefined}
          actions={
            zone && (
              <div className="flex items-center gap-1">
                <IconButton
                  label="Copy as zone file"
                  onClick={() => {
                    void copyText(bind()).then((ok) => (ok ? toast("Zone copied in BIND format") : toast(COPY_BLOCKED, "error")));
                  }}
                >
                  <Copy className="size-4" />
                </IconButton>
                <IconButton label="Refresh records" onClick={() => void loadRecords()}>
                  <RefreshCw className="size-4" />
                </IconButton>
                <IconButton label="Delete zone" className="hover:text-bad" onClick={() => setDropZone(true)}>
                  <Trash2 className="size-4" />
                </IconButton>
                <Button variant="primary" onClick={() => setEditing("new")}>
                  <Plus className="size-4" /> Record
                </Button>
              </div>
            )
          }
        >
          {!zone ? (
            <Empty icon={<Globe className="size-5" />} title="Pick or create a zone" />
          ) : (
            <>
              <div className="flex flex-wrap items-center gap-2 border-b border-line px-4 py-2.5">
                <div className="flex flex-wrap gap-1">
                  {["all", ...Object.keys(counts).sort()].map((t) => (
                    <button
                      key={t}
                      onClick={() => setTypeFilter(t)}
                      className={`rounded-full border px-2.5 py-0.5 text-xs ${typeFilter === t ? "border-accent/40 bg-accent-soft text-accent" : "border-line text-muted hover:text-fg"}`}
                    >
                      {t === "all" ? "All" : t}
                      <span className="ml-1 text-faint">{t === "all" ? Object.values(counts).reduce((a, b) => a + b, 0) : counts[t]}</span>
                    </button>
                  ))}
                </div>
                <div className="relative ml-auto w-full sm:w-56">
                  <Search className="pointer-events-none absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-faint" />
                  <Input className="py-1 pl-8" value={q} onChange={(e) => setQ(e.target.value)} placeholder="Search name or value" />
                </div>
              </div>
              {!records ? (
                <Skeleton rows={5} />
              ) : rows.length === 0 ? (
                <Empty icon={<Network className="size-5" />} title={records.length ? "No matching records" : "No records"}>
                  {!records.length && "Add an A record to point a name at a server."}
                </Empty>
              ) : (
                <div className="overflow-x-auto">
                  <table className="w-full text-[13px]">
                    <thead>
                      <tr className="border-b border-line text-left text-[11px] tracking-wide text-faint uppercase">
                        <th className="px-4 py-2 font-medium">Name</th>
                        <th className="px-2 py-2 font-medium">Type</th>
                        <th className="px-2 py-2 font-medium">Value</th>
                        <th className="px-2 py-2 font-medium">TTL</th>
                        <th />
                      </tr>
                    </thead>
                    <tbody className="divide-y divide-line">
                      {rows.map((r) => {
                        const locked = !EDITABLE.includes(r.type) || (r.type === "NS" && r.name === zone);
                        return (
                          <tr key={`${r.name}/${r.type}`} className="group align-top hover:bg-panel-2/60">
                            <td className="px-4 py-2.5">
                              <span className="font-mono text-xs font-medium">{relName(r.name, zone)}</span>
                              {r.name !== zone && <div className="truncate font-mono text-[11px] text-faint">{r.name}</div>}
                            </td>
                            <td className="px-2 py-2.5">
                              <span className={`font-mono text-xs font-semibold ${TYPES[r.type]?.tone ?? "text-faint"}`}>{r.type}</span>
                            </td>
                            <td className="max-w-[420px] px-2 py-2.5">
                              <div className="flex flex-col gap-1">
                                {r.contents.map((c) => (
                                  <span key={c} className="w-fit max-w-full truncate rounded bg-panel-2 px-1.5 py-0.5 font-mono text-xs" title={c}>
                                    {c}
                                  </span>
                                ))}
                              </div>
                            </td>
                            <td className="px-2 py-2.5 text-xs text-muted tabular-nums">{ttlText(r.ttl)}</td>
                            <td className="px-3 py-2 text-right whitespace-nowrap">
                              {locked ? (
                                <Badge>managed</Badge>
                              ) : (
                                <span className="opacity-60 group-hover:opacity-100">
                                  <IconButton label="Edit" onClick={() => setEditing(r)}>
                                    <Pencil className="size-4" />
                                  </IconButton>
                                  <IconButton label="Delete" className="hover:text-bad" onClick={() => setDropRec(r)}>
                                    <Trash2 className="size-4" />
                                  </IconButton>
                                </span>
                              )}
                            </td>
                          </tr>
                        );
                      })}
                    </tbody>
                  </table>
                </div>
              )}
            </>
          )}
        </Panel>
      </div>

      <RecordDialog
        zone={zone}
        rec={editing}
        existing={records ?? []}
        onClose={() => setEditing(undefined)}
        onSave={async (r, replaced) => {
          if (replaced && (replaced.name !== r.name || replaced.type !== r.type)) {
            if (!(await call({ op: "delete_records", zone, name: replaced.name, rtype: replaced.type }))) return false;
          }
          const ok = await call({ op: "set_records", zone, name: r.name, rtype: r.type, ttl: r.ttl, contents: r.contents });
          if (ok) {
            toast(`${r.type} ${relName(r.name, zone)} saved`);
            void loadRecords();
            void loadZones();
          }
          return !!ok;
        }}
      />

      <NewZoneDialog
        open={addZone}
        onOpenChange={setAddZone}
        onCreate={async (name, ns) => {
          const ok = await call({ op: "create_zone", zone: name, nameservers: ns });
          if (ok) {
            toast(`Zone ${name} created`);
            setZone(name);
            void loadZones();
          }
          return !!ok;
        }}
      />

      <TypedConfirm
        open={dropZone}
        onOpenChange={setDropZone}
        title={`Delete zone ${zone.replace(/\.$/, "")}?`}
        word={zone.replace(/\.$/, "")}
        body="Every record in the zone is removed and the domain stops resolving from this server."
        onConfirm={async () => {
          if (await call({ op: "delete_zone", zone })) {
            toast(`Zone ${zone} deleted`);
            setZone("");
            void loadZones();
            return true;
          }
          return false;
        }}
      />

      <TypedConfirm
        open={!!dropRec}
        onOpenChange={(o) => !o && setDropRec(undefined)}
        title={dropRec ? `Delete ${dropRec.type} ${relName(dropRec.name, zone)}?` : ""}
        body={dropRec ? `${dropRec.contents.length} value${dropRec.contents.length === 1 ? "" : "s"}: ${dropRec.contents.join(", ")}` : ""}
        onConfirm={async () => {
          if (dropRec && (await call({ op: "delete_records", zone, name: dropRec.name, rtype: dropRec.type }))) {
            toast("Record deleted");
            void loadRecords();
            return true;
          }
          return false;
        }}
      />
    </>
  );
}

function RecordDialog({
  zone,
  rec,
  existing,
  onClose,
  onSave,
}: {
  zone: string;
  rec: Rec | "new" | undefined;
  existing: Rec[];
  onClose(): void;
  onSave(r: Rec, replaced?: Rec): Promise<boolean>;
}) {
  const editing = rec && rec !== "new" ? rec : undefined;
  const [name, setName] = useState("");
  const [type, setType] = useState("A");
  const [ttl, setTtl] = useState(300);
  const [values, setValues] = useState<{ value: string; priority: number }[]>([{ value: "", priority: 10 }]);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!rec) return;
    if (editing) {
      setName(relName(editing.name, zone));
      setType(editing.type);
      setTtl(editing.ttl);
      setValues(editing.contents.map((c) => fromWire(editing.type, c)));
    } else {
      setName("");
      setType("A");
      setTtl(300);
      setValues([{ value: "", priority: 10 }]);
    }
  }, [rec, editing, zone]);

  const spec = TYPES[type] ?? TYPES.A!;
  const fqdn = name.trim() === "" || name.trim() === "@" ? zone : name.endsWith(".") ? name.trim() : `${name.trim()}.${zone}`;
  const clash = !editing || editing.name !== fqdn || editing.type !== type ? existing.find((r) => r.name === fqdn && r.type === type) : undefined;
  const cnameConflict = type === "CNAME" ? existing.some((r) => r.name === fqdn && r.type !== "CNAME") : existing.some((r) => r.name === fqdn && r.type === "CNAME");
  const filled = values.filter((v) => v.value.trim());

  return (
    <Modal open={!!rec} onOpenChange={(o) => !o && onClose()} title={editing ? "Edit record" : "Add record"} description={zone} wide>
      <form
        className="space-y-4"
        onSubmit={async (e) => {
          e.preventDefault();
          setBusy(true);
          const ok = await onSave({ name: fqdn, type, ttl, contents: filled.map((v) => toWire(type, v.value, v.priority)) }, editing);
          setBusy(false);
          if (ok) onClose();
        }}
      >
        <div className="grid gap-3 sm:grid-cols-[1fr_140px]">
          <Field label="Name" hint={<span className="font-mono">{fqdn}</span>}>
            <div className="flex items-center rounded-md border border-line focus-within:border-accent">
              <input data-bare className="min-w-0 flex-1 bg-transparent px-2.5 py-1.5 text-[13px] outline-none" value={name} onChange={(e) => setName(e.target.value)} placeholder="@ or www" autoFocus />
              <span className="pr-2.5 font-mono text-xs text-faint">.{zone.replace(/\.$/, "")}</span>
            </div>
          </Field>
          <Field label="Type">
            <Select value={type} onChange={(e) => setType(e.target.value)}>
              {EDITABLE.map((t) => (
                <option key={t}>{t}</option>
              ))}
            </Select>
          </Field>
        </div>

        <Field label={type === "CNAME" ? "Target" : "Values"} hint={spec.hint + (type === "CNAME" ? "" : ". Several values are answered round robin.")}>
          <div className="space-y-2">
            {values.map((v, i) => (
              <div key={i} className="flex gap-2">
                {spec.priority && (
                  <Input
                    className="w-20"
                    type="number"
                    min={0}
                    title="Priority"
                    value={v.priority}
                    onChange={(e) => setValues(values.map((x, j) => (j === i ? { ...x, priority: Number(e.target.value) } : x)))}
                  />
                )}
                <Input className="flex-1 font-mono" value={v.value} placeholder={spec.placeholder} onChange={(e) => setValues(values.map((x, j) => (j === i ? { ...x, value: e.target.value } : x)))} />
                {values.length > 1 && (
                  <IconButton label="Remove value" type="button" onClick={() => setValues(values.filter((_, j) => j !== i))}>
                    <Trash2 className="size-4" />
                  </IconButton>
                )}
              </div>
            ))}
            {type !== "CNAME" && (
              <button type="button" className="text-xs text-accent hover:underline" onClick={() => setValues([...values, { value: "", priority: 10 }])}>
                + Add value
              </button>
            )}
          </div>
        </Field>

        <Field label="TTL" hint="How long resolvers may cache the answer.">
          <div className="flex flex-wrap items-center gap-1.5">
            {TTLS.map(([s, label]) => (
              <button
                type="button"
                key={s}
                onClick={() => setTtl(s)}
                className={`rounded-md border px-2.5 py-1 text-xs ${ttl === s ? "border-accent/40 bg-accent-soft text-accent" : "border-line text-muted hover:text-fg"}`}
              >
                {label}
              </button>
            ))}
            <Input className="w-24 py-1" type="number" min={30} value={ttl} onChange={(e) => setTtl(Number(e.target.value))} />
            <span className="text-xs text-faint">seconds</span>
          </div>
        </Field>

        {clash && <p className="rounded-md bg-warn-soft px-3 py-2 text-[13px] text-warn">A {type} record for this name exists; saving replaces its {clash.contents.length} value(s).</p>}
        {cnameConflict && <p className="rounded-md bg-bad-soft px-3 py-2 text-[13px] text-bad">A CNAME cannot share its name with other records.</p>}

        <div className="flex justify-end gap-2">
          <Button type="button" variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" disabled={busy || filled.length === 0 || cnameConflict || (type === "CNAME" && filled.length > 1)}>
            {busy ? "Saving…" : editing ? "Save changes" : "Add record"}
          </Button>
        </div>
      </form>
    </Modal>
  );
}

function NewZoneDialog({ open, onOpenChange, onCreate }: { open: boolean; onOpenChange(o: boolean): void; onCreate(zone: string, ns: string[]): Promise<boolean> }) {
  const [name, setName] = useState("");
  const [ns, setNs] = useState<string[]>([""]);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    if (open) {
      setName("");
      setNs([""]);
    }
  }, [open]);
  const zone = dot(name.trim().toLowerCase());
  const filled = ns.map((x) => x.trim()).filter(Boolean);
  const nameservers = filled.length ? filled.map(dot) : [`ns1.${zone}`];
  const host = /^[A-Za-z0-9-]+(\.[A-Za-z0-9-]+)+\.?$/;
  return (
    <Modal open={open} onOpenChange={onOpenChange} title="New zone" description="Creates an authoritative zone with an SOA and NS records.">
      <form
        className="space-y-4"
        onSubmit={async (e) => {
          e.preventDefault();
          setBusy(true);
          const ok = await onCreate(zone, nameservers);
          setBusy(false);
          if (ok) onOpenChange(false);
        }}
      >
        <Field label="Domain">
          <Input value={name} onChange={(e) => setName(e.target.value)} placeholder="example.com" required autoFocus pattern="[A-Za-z0-9.\-]+" />
        </Field>
        <Field label="Name servers" hint={`One per row; at least two are recommended. Empty uses ns1.${name.trim() || "example.com"}.`}>
          <div className="space-y-1.5">
            {ns.map((v, i) => (
              <div key={i} className="flex items-center gap-1.5">
                <span className="w-5 text-right text-xs text-faint tabular-nums">{i + 1}</span>
                <Input
                  className={`flex-1 font-mono ${v.trim() && !host.test(v.trim()) ? "border-bad" : ""}`}
                  value={v}
                  onChange={(e) => setNs(ns.map((x, n) => (n === i ? e.target.value : x)))}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" && v.trim()) {
                      e.preventDefault();
                      setNs([...ns, ""]);
                    }
                  }}
                  placeholder={`ns${i + 1}.${name.trim() || "example.com"}`}
                  aria-label={`Name server ${i + 1}`}
                />
                <button type="button" aria-label="Remove name server" title={ns.length === 1 ? "A zone needs at least one name server" : "Remove name server"} disabled={ns.length === 1} className="rounded p-1 text-faint hover:text-bad disabled:opacity-30" onClick={() => setNs(ns.filter((_, n) => n !== i))}>
                  <X className="size-4" />
                </button>
              </div>
            ))}
            <button type="button" className="inline-flex items-center gap-1 pl-6 text-xs text-accent hover:underline" onClick={() => setNs([...ns, ""])}>
              <Plus className="size-3.5" /> Add name server
            </button>
          </div>
        </Field>
        <div className="flex justify-end gap-2">
          <Button type="button" variant="ghost" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button variant="primary" disabled={busy || !name.trim() || filled.some((x) => !host.test(x))}>
            {busy ? "Creating…" : "Create zone"}
          </Button>
        </div>
      </form>
    </Modal>
  );
}

/** Confirmation; with `word`, the user has to type it first. */
function TypedConfirm({
  open,
  onOpenChange,
  title,
  body,
  word,
  onConfirm,
}: {
  open: boolean;
  onOpenChange(o: boolean): void;
  title: string;
  body: string;
  word?: string;
  onConfirm(): Promise<boolean>;
}) {
  const [typed, setTyped] = useState("");
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    if (open) setTyped("");
  }, [open]);
  const ready = !word || typed.trim() === word;
  return (
    <Modal open={open} onOpenChange={onOpenChange} title={title}>
      <form
        className="space-y-4"
        onSubmit={async (e) => {
          e.preventDefault();
          if (!ready) return;
          setBusy(true);
          const ok = await onConfirm();
          setBusy(false);
          if (ok) onOpenChange(false);
        }}
      >
        <p className="text-[13px] break-all text-muted">{body}</p>
        {word && (
          <Field label="Type the zone name to confirm" hint={<span className="font-mono">{word}</span>}>
            <Input value={typed} onChange={(e) => setTyped(e.target.value)} autoComplete="off" spellCheck={false} autoFocus />
          </Field>
        )}
        <div className="flex justify-end gap-2">
          <Button type="button" variant="ghost" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button variant="danger" disabled={!ready || busy}>
            {busy ? "Deleting…" : "Delete"}
          </Button>
        </div>
      </form>
    </Modal>
  );
}
