import { Lock, RefreshCw } from "lucide-react";
import { type ReactNode, useEffect, useState } from "react";

import { post } from "../../api/client";
import { errorText } from "../../app/toast";
import { Input } from "../ui";

/** Forge connects cluster nodes with an encrypted WireGuard mesh only. */
export type FabricMode = "mesh";
export interface FabricSpec {
  subnet: string;
  mode: FabricMode;
  domain: string;
  port: number;
  endpoints?: Record<string, string>;
}
export interface Lease {
  kind: "node" | "block" | "service";
  owner: string;
  cidr: string;
  pinned?: boolean;
}
export interface Layout {
  subnet: string;
  reserved: [number, number];
  nodes: [number, number];
  services: [number, number];
  block_prefix: number;
  first_block: number;
  blocks: number;
}
export interface FabricResponse {
  spec: FabricSpec | null;
  layout?: Layout;
  pins?: Lease[];
  leases?: Lease[];
  status?: { applied_by?: { at: number }; nodes?: Record<string, NodeSync> };
  members: { id: string; name: string; address: string; status: string; os: string }[];
  services: { id: string; name: string; environment: string }[];
}
export interface NodeSync {
  state: "pending" | "ok" | "waiting" | "retrying" | "failed";
  step?: string | null;
  error?: string | null;
  attempts?: number;
  next_at?: number;
  checked_at?: number;
  public_key?: string | null;
  unreachable?: string[];
  relay?: boolean;
  /** Container network on the node: "ok", "unsupported" or "waiting for a block". Older servers omit it. */
  network?: string;
  network_error?: string;
  docker_version?: string;
}

/** Docker name of the cluster container network: `forge-` + the interface name (`forge` + first 6 alphanumerics of the cluster id). */
export function meshNetworkName(clusterId: string): string {
  return `forge-forge${clusterId.replace(/[^a-z0-9]/gi, "").slice(0, 6).toLowerCase()}`;
}
export interface Suggestion {
  subnet: string;
  pool: string;
  domain: string;
  taken: { subnet: string; by: string }[];
}

const CIDR = /^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})\/(\d{1,2})$/;
export const validSubnet = (s: string) => {
  const m = CIDR.exec(s.trim());
  return !!m && m.slice(1, 5).every((o) => Number(o) <= 255) && Number(m[5]) >= 16 && Number(m[5]) <= 24;
};

const ip = (s: string) => s.split("/")[0]!.split(".").map(Number);
const add = (base: number[], off: number) => {
  const n = ((base[0]! << 24) >>> 0) + (base[1]! << 16) + (base[2]! << 8) + base[3]! + off;
  return [n >>> 24, (n >>> 16) & 255, (n >>> 8) & 255, n & 255].join(".");
};

/** What each region of the subnet is for (mirrors forge_catalog::fabric::Layout). */
export function layoutOf(subnet: string) {
  const prefix = Number(subnet.split("/")[1]);
  const base = ip(subnet);
  const size = 2 ** (32 - prefix);
  const large = prefix <= 20;
  const blockSize = large ? 256 : 16;
  const firstBlock = large ? 10 : 6;
  const blocks = Math.max(0, size / blockSize - firstBlock);
  const nodes: [number, number] = large ? [16, 254] : [16, 31];
  const services: [number, number] = large ? [256, 511] : [32, 95];
  return {
    rows: [
      { label: "Gateway & ingress VIPs", range: `${add(base, 1)} – ${add(base, 15)}`, tone: "bg-faint" },
      { label: "Node addresses", range: `${add(base, nodes[0])} – ${add(base, nodes[1])}`, tone: "bg-accent" },
      { label: "Service IPs", range: `${add(base, services[0])} – ${add(base, services[1])}`, tone: "bg-ok" },
      { label: `Instance blocks (/${large ? 24 : 28} per node)`, range: `${add(base, firstBlock * blockSize)} onward · ${blocks} blocks`, tone: "bg-warn" },
    ],
    maxNodes: Math.min(blocks, nodes[1] - nodes[0] + 1),
    services: services[1] - services[0] + 1,
  };
}

/** Subnet size, subnet (with a suggestion), mesh mode and internal domain. */
export function FabricForm({ value, onChange, clusterId, clusterName }: { value: FabricSpec; onChange: (v: FabricSpec) => void; clusterId?: string; clusterName: string }) {
  const [hint, setHint] = useState<Suggestion>();
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const prefix = Number(value.subnet.split("/")[1]) || 16;

  const suggest = async (p = prefix, fill = true) => {
    setBusy(true);
    setError("");
    try {
      const s = await post<Suggestion>(clusterId ? `/api/v1/clusters/${clusterId}/fabric/suggest` : "/api/v1/fabric/suggest", { prefix: p, name: clusterName || "cluster" });
      setHint(s);
      if (fill) onChange({ ...value, subnet: s.subnet, domain: value.domain || s.domain });
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  };
  useEffect(() => {
    void suggest(prefix, !value.subnet);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const ok = validSubnet(value.subnet);
  const clash = ok ? hint?.taken.find((t) => overlaps(t.subnet, value.subnet)) : undefined;
  const plan = ok ? layoutOf(value.subnet) : null;

  return (
    <div className="grid gap-5 lg:grid-cols-[minmax(0,1fr)_22rem]">
      <div className="min-w-0 space-y-5">
        <Step n={1} title="Size">
          <div className="grid gap-2 sm:grid-cols-2" role="radiogroup" aria-label="Subnet size">
            {[
              { p: 16, title: "Class B · /16", body: "65,534 addresses · up to 239 nodes, 256 services" },
              { p: 24, title: "Class C · /24", body: "254 addresses · up to 10 nodes, 64 services" },
            ].map((o) => {
              const on = prefix === o.p;
              return (
                <button
                  type="button"
                  role="radio"
                  aria-checked={on}
                  key={o.p}
                  onClick={() => void suggest(o.p)}
                  className={`flex items-start gap-2.5 rounded-lg border p-3 text-left transition ${on ? "border-accent bg-accent-soft ring-1 ring-accent/30" : "border-line bg-panel hover:border-accent/50"}`}
                >
                  <span className={`mt-0.5 grid size-4 shrink-0 place-items-center rounded-full border ${on ? "border-accent" : "border-line"}`}>{on && <span className="size-2 rounded-full bg-accent" />}</span>
                  <span className="min-w-0">
                    <span className="block text-[13px] font-medium">{o.title}</span>
                    <span className="block text-xs text-muted">{o.body}</span>
                  </span>
                </button>
              );
            })}
          </div>
        </Step>

        <Step n={2} title="Subnet" note={hint ? `From the pool ${hint.pool}` : undefined}>
          <div className="flex gap-2">
            <Input aria-label="Subnet" className="min-w-0 flex-1 font-mono" value={value.subnet} onChange={(e) => onChange({ ...value, subnet: e.target.value.trim() })} placeholder="10.64.0.0/16" />
            <button type="button" onClick={() => void suggest()} className="inline-flex shrink-0 items-center gap-1.5 rounded-md border border-line bg-panel px-3 text-[13px] hover:border-accent" disabled={busy} title="Pick the next free range from the pool">
              <RefreshCw className={`size-3.5 ${busy ? "animate-spin" : ""}`} /> Next free
            </button>
          </div>
          {!ok && value.subnet && <p className="text-xs text-bad">Use a private IPv4 range between /16 and /24, e.g. 10.64.0.0/16.</p>}
          {clash && <p className="text-xs text-bad">Overlaps {clash.subnet} ({clash.by}).</p>}
          {error && <p className="text-xs text-bad">{error}</p>}
          {hint && hint.taken.length > 0 && (
            <div className="flex flex-wrap items-center gap-1.5 text-[11px]">
              <span className="text-faint">Already in use:</span>
              {hint.taken.map((t) => (
                <span key={`${t.subnet}-${t.by}`} className="rounded-full border border-line bg-panel-2 px-2 py-0.5 text-muted">
                  <span className="font-mono">{t.subnet}</span> · {t.by}
                </span>
              ))}
            </div>
          )}
        </Step>

        <Step n={3} title="Internal domain">
          <Input aria-label="Internal domain" className="font-mono" value={value.domain} onChange={(e) => onChange({ ...value, domain: e.target.value.trim().toLowerCase() })} placeholder={hint?.domain ?? "lab.forge.internal"} />
          <p className="text-xs text-muted">
            Services resolve as <code className="font-mono">api</code> (same environment), <code className="font-mono">api.payments</code> (same cluster) and{" "}
            <code className="font-mono">api.payments.{value.domain || "<domain>"}</code>.
          </p>
        </Step>

        <div className="flex items-start gap-3 rounded-lg border border-ok/30 bg-ok-soft p-3">
          <Lock className="mt-0.5 size-4 shrink-0 text-ok" />
          <div>
            <div className="text-[13px] font-medium">Encrypted mesh (WireGuard)</div>
            <div className="text-xs text-muted">Works across clouds, data centres and home labs behind NAT. Traffic between nodes is always encrypted.</div>
          </div>
        </div>
      </div>

      <aside className="min-w-0 self-start overflow-hidden rounded-lg border border-line bg-panel lg:sticky lg:top-4" aria-label="Address plan">
        <div className="border-b border-line bg-panel-2 px-4 py-2">
          <div className="text-[11px] font-semibold tracking-wider text-faint uppercase">Address plan</div>
          <div className="truncate font-mono text-[13px]">{ok ? value.subnet : "—"}</div>
        </div>
        {plan ? (
          <div className="space-y-3 p-4">
            <ul className="space-y-2.5">
              {plan.rows.map((r) => (
                <li key={r.label} className="flex gap-2">
                  <span className={`mt-1 size-2.5 shrink-0 rounded-sm ${r.tone}`} />
                  <div className="min-w-0">
                    <div className="text-xs text-muted">{r.label}</div>
                    <div className="truncate font-mono text-xs" title={r.range}>
                      {r.range}
                    </div>
                  </div>
                </li>
              ))}
            </ul>
            <div className="grid grid-cols-2 gap-2 border-t border-line pt-3 text-center">
              <div className="rounded-md bg-panel-2 py-1.5">
                <div className="font-mono text-sm font-semibold tabular-nums">{plan.maxNodes}</div>
                <div className="text-[11px] text-faint">max nodes</div>
              </div>
              <div className="rounded-md bg-panel-2 py-1.5">
                <div className="font-mono text-sm font-semibold tabular-nums">{plan.services}</div>
                <div className="text-[11px] text-faint">service IPs</div>
              </div>
            </div>
          </div>
        ) : (
          <p className="p-4 text-xs text-faint">Enter a valid subnet to see how Forge splits it.</p>
        )}
      </aside>
    </div>
  );
}

/** A numbered step of the form. */
function Step({ n, title, note, children }: { n: number; title: string; note?: string; children: ReactNode }) {
  return (
    <section className="space-y-2">
      <div className="flex min-w-0 items-center gap-2">
        <span className="grid size-5 shrink-0 place-items-center rounded-full bg-accent-soft text-[11px] font-semibold text-accent">{n}</span>
        <h3 className="text-[13px] font-semibold">{title}</h3>
        {note && <span className="truncate text-[11px] text-faint">{note}</span>}
      </div>
      <div className="space-y-2 pl-7">{children}</div>
    </section>
  );
}

function overlaps(a: string, b: string) {
  const range = (s: string) => {
    const p = Number(s.split("/")[1]);
    const [o1, o2, o3, o4] = ip(s);
    const n = (((o1! << 24) >>> 0) + (o2! << 16) + (o3! << 8) + o4!) >>> 0;
    const size = 2 ** (32 - p);
    const start = n - (n % size);
    return [start, start + size - 1] as const;
  };
  const [a0, a1] = range(a);
  const [b0, b1] = range(b);
  return a0 <= b1 && b0 <= a1;
}

export const blankFabric = (): FabricSpec => ({ subnet: "", mode: "mesh", domain: "", port: 51820, endpoints: {} });
