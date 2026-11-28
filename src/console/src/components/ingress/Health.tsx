import { Activity, Globe } from "lucide-react";

import { IpTag } from "../IpTag";
import { bytesRate, useTraffic } from "../Traffic";
import { Badge, Empty, Status, ago } from "../ui";
import type { LiveResponse, ProfileResponse } from "./types";

/**
 * Edge health as the control plane sees it (agent online + port 80 on the
 * public IP, three checks in a row to change), and the DNS records Forge
 * currently publishes for the cluster's endpoints.
 */
export function Health({ data, live, clusterId }: { data: ProfileResponse; live?: LiveResponse; clusterId?: string }) {
  const traffic = useTraffic(clusterId).data;
  const ingress = data.profile?.ingress;
  if (!ingress) {
    return (
      <Empty icon={<Activity className="size-5" />} title="No ingress yet">
        Configure the profile first.
      </Empty>
    );
  }
  const health = data.status.health ?? {};
  const dns = data.status.dns;
  const nameOf = (id: string) => data.members.find((m) => m.id === id)?.name ?? id.slice(0, 8);
  const watching = !!ingress.dns?.auto_failover && ingress.mode !== "standalone";
  const nodeFlows = Object.entries(traffic?.nodes ?? {}).sort(([, a], [, b]) => b.to_bps + b.from_bps - (a.to_bps + a.from_bps));
  const peak = Math.max(1, ...nodeFlows.map(([, f]) => f.to_bps + f.from_bps));
  return (
    <div className="max-w-3xl space-y-3">
      <div className="rounded-lg border border-line bg-panel">
        <div className="card-head flex items-center gap-2 rounded-t-lg border-b border-line px-3.5 py-2.5">
          <span className="font-medium">Load</span>
          <span className="ml-auto text-xs text-muted">Through the edges, last minute · refreshed every 15 s</span>
        </div>
        <div className="grid gap-4 p-3.5 text-[13px] md:grid-cols-2">
          <div>
            <div className="mb-1.5 text-[10px] font-semibold tracking-widest text-faint uppercase">Edges (clients in / out)</div>
            {ingress.edges.map((e) => {
              const f = traffic?.edges?.[e.server_id];
              return (
                <div key={e.server_id} className="flex items-center justify-between gap-2 py-0.5">
                  <span className="truncate">{nameOf(e.server_id)}</span>
                  <span className="font-mono text-xs tabular-nums text-muted">
                    {f ? (
                      <>
                        <span className="text-accent">↓ {bytesRate(f.in_bps)}</span> · ↑ {bytesRate(f.out_bps)} · {(f.requests / (traffic?.window_secs || 60)).toFixed(1)} req/s
                      </>
                    ) : (
                      "no traffic"
                    )}
                  </span>
                </div>
              );
            })}
          </div>
          <div>
            <div className="mb-1.5 text-[10px] font-semibold tracking-widest text-faint uppercase">Nodes behind the edges</div>
            {nodeFlows.length === 0 && <p className="text-xs text-faint">No requests forwarded in the last minute.</p>}
            {nodeFlows.map(([id, f]) => (
              <div key={id} className="py-0.5">
                <div className="flex items-center justify-between gap-2">
                  <span className="truncate">{data.members.find((m) => m.id === id)?.name ?? id.slice(0, 8)}</span>
                  <span className="font-mono text-xs tabular-nums text-muted">
                    <span className="text-accent">→ {bytesRate(f.to_bps)}</span> · ← {bytesRate(f.from_bps)}
                  </span>
                </div>
                <div className="mt-0.5 h-1 overflow-hidden rounded-full bg-panel-2">
                  <div className="h-full rounded-full bg-accent" style={{ width: `${((f.to_bps + f.from_bps) / peak) * 100}%` }} />
                </div>
              </div>
            ))}
          </div>
        </div>
      </div>
      <div className="rounded-lg border border-line bg-panel">
        <div className="card-head flex items-center gap-2 rounded-t-lg border-b border-line px-3.5 py-2.5">
          <span className="font-medium">Edges</span>
          <span className="ml-auto text-xs text-muted">{watching ? "Checked every 15 s" : "Automatic failover is off"}</span>
        </div>
        <div className="divide-y divide-line">
          {ingress.edges.map((e) => {
            const h = health[e.server_id];
            const nginx = live?.edges[e.server_id]?.state?.nginx;
            const inDns = !!e.public_ip && (dns?.addresses ?? []).includes(e.public_ip);
            return (
              <div key={e.server_id} className="flex flex-wrap items-center gap-x-3 gap-y-1 px-3.5 py-2 text-[13px]">
                <Status value={!h ? "pending" : h.healthy ? (h.streak ? "running" : "online") : "failed"} />
                <span className="font-medium">{nameOf(e.server_id)}</span>
                {ingress.mode === "active_standby" && <Badge tone={e.role === "primary" ? "accent" : "muted"}>{e.role}</Badge>}
                {e.public_ip && <IpTag label="public" value={e.public_ip} />}
                {nginx && <span className="text-xs text-faint">nginx {nginx}</span>}
                <span className="ml-auto flex items-center gap-2 text-xs">
                  {h && !h.healthy && <span className="text-bad">{h.last_error ?? "unhealthy"}</span>}
                  {h && h.healthy && h.streak > 0 && <span className="text-warn">{h.streak} failed check{h.streak === 1 ? "" : "s"}</span>}
                  {h?.since ? <span className="text-faint">since {ago(h.since)}</span> : null}
                  {ingress.dns && (inDns ? <Badge tone="ok">in DNS</Badge> : <Badge>not in DNS</Badge>)}
                </span>
              </div>
            );
          })}
        </div>
      </div>

      {ingress.dns ? (
        <div className="rounded-lg border border-line bg-panel">
          <div className="card-head flex items-center gap-2 rounded-t-lg border-b border-line px-3.5 py-2.5">
            <Globe className="size-4 text-accent" />
            <span className="font-medium">DNS records</span>
            <span className="ml-auto text-xs text-muted">{dns ? `published ${ago(dns.published_at)} · TTL ${ingress.dns.ttl}s` : "not published yet: press Save & apply"}</span>
          </div>
          {dns?.error && <p className="px-3.5 py-2 text-xs text-bad">{dns.error}</p>}
          <div className="divide-y divide-line">
            {(dns?.records ?? []).map((r, i) => (
              <div key={i} className="flex flex-wrap items-center gap-x-3 gap-y-1 px-3.5 py-2 text-[13px]">
                <Status value={r.status === "succeeded" ? "online" : r.status === "failed" ? "failed" : "pending"} />
                <span className="font-mono">{r.name}</span>
                {r.type && <Badge>{r.type}</Badge>}
                <span className="font-mono text-xs text-muted">→ {dns!.addresses.join(", ")}</span>
                {r.zone && <span className="text-xs text-faint">zone {r.zone}</span>}
                {r.error && <span className="w-full text-xs text-bad">{r.error}</span>}
              </div>
            ))}
            {dns && !dns.records.length && !dns.error && <p className="px-3.5 py-2 text-xs text-muted">No endpoints on this cluster yet.</p>}
          </div>
        </div>
      ) : (
        <p className="text-xs text-muted">Forge-managed DNS is off for this cluster. Turn it on in Profile → Traffic &amp; DNS to publish and fail over records automatically.</p>
      )}
    </div>
  );
}
