import { useMutation, useQueryClient } from "@tanstack/react-query";
import { AlertTriangle, ChevronRight, Cpu, Loader2, Rocket, Shield } from "lucide-react";
import { type ReactNode, useEffect, useMemo, useState } from "react";

import { api, post } from "../../api/client";
import { useInstallations, useInstallPackage, useServers } from "../../api/hooks";
import { nodeName } from "../../api/types";
import { errorText, useToast } from "../../app/toast";
import { Badge, Button, Input, Select, TextArea, UnsavedBar } from "../ui";
import { type ClusterProfile, defaultNginx, type IngressMode, type IngressSpec, MODES, type Member, type NginxSettings, type ProfileResponse } from "./types";

/** A collapsible section: one line of title + summary, the body underneath. */
function Section({ title, summary, open, children }: { title: string; summary?: ReactNode; open?: boolean; children: ReactNode }) {
  return (
    <details open={open} className="group rounded-lg border border-line bg-panel">
      <summary className="card-head flex cursor-pointer list-none items-center gap-2 rounded-t-lg px-3.5 py-2.5 select-none group-[:not([open])]:rounded-b-lg">
        <ChevronRight className="size-4 text-faint transition group-open:rotate-90" />
        <span className="font-medium">{title}</span>
        {summary && <span className="ml-auto truncate pl-3 text-xs text-muted">{summary}</span>}
      </summary>
      <div className="space-y-3 border-t border-line px-3.5 py-3">{children}</div>
    </details>
  );
}

/** Label left, control right: keeps every setting on one short line. */
function Row({ label, hint, children }: { label: string; hint?: string; children: ReactNode }) {
  return (
    <label className="grid items-center gap-x-4 gap-y-1 sm:grid-cols-[180px_1fr]">
      <span className="text-[13px]">
        {label}
        {hint && <span className="block text-[11px] text-faint">{hint}</span>}
      </span>
      {children}
    </label>
  );
}

function Toggle({ checked, onChange, children }: { checked: boolean; onChange: (v: boolean) => void; children: ReactNode }) {
  return (
    <label className="inline-flex items-center gap-1.5 text-[13px]">
      <input type="checkbox" checked={checked} onChange={(e) => onChange(e.target.checked)} /> {children}
    </label>
  );
}

/** Tiny topology sketch per mode. */
function ModeDiagram({ mode }: { mode: IngressMode }) {
  const edges = mode === "standalone" ? [1] : [0, 1, 2].slice(0, mode === "active_standby" ? 2 : 3);
  return (
    <svg viewBox="0 0 90 34" className="h-8 w-20 shrink-0 text-muted" aria-hidden>
      <circle cx={45} cy={6} r={5} className="fill-accent/20 stroke-accent" />
      {edges.map((_, i) => {
        const x = edges.length === 1 ? 45 : 15 + i * (60 / (edges.length - 1));
        const idle = mode === "active_standby" && i > 0;
        return (
          <g key={i}>
            <line x1={45} y1={11} x2={x} y2={22} className={idle ? "stroke-line" : "stroke-accent"} strokeDasharray={idle ? "2 2" : undefined} />
            <rect x={x - 9} y={22} width={18} height={10} rx={2} className={idle ? "fill-panel-2 stroke-line" : "fill-ok/25 stroke-ok"} />
          </g>
        );
      })}
    </svg>
  );
}

const csv = (s: string) => s.split(/[\s,]+/).map((x) => x.trim()).filter(Boolean);
const blankIngress = (mode: IngressMode = "standalone"): IngressSpec => ({ mode, edges: [], failover: "dns", vrrp: null, nginx: defaultNginx() });

export function ProfileEditor({ clusterId, data }: { clusterId: string; data: ProfileResponse; name?: string }) {
  const qc = useQueryClient();
  const toast = useToast();
  const installs = useInstallations().data ?? [];
  const install = useInstallPackage();
  const servers = useServers().data ?? [];
  const dnsNodes = servers.filter((n) => installs.some((i) => i.server_id === n.id && i.package === "powerdns" && i.status === "installed"));
  const saved = useMemo<ClusterProfile>(() => ({ ingress: data.profile?.ingress ?? null }), [data.profile]);
  const [p, setP] = useState<ClusterProfile>(saved);
  useEffect(() => setP(saved), [saved]);
  const dirty = JSON.stringify(p) !== JSON.stringify(saved);

  const members = data.members;
  const ingress = p.ingress;
  const setIngress = (i: Partial<IngressSpec>) => setP((x) => ({ ingress: { ...(x.ingress ?? blankIngress()), ...i } }));
  const setNginx = (n: Partial<NginxSettings>) => ingress && setIngress({ nginx: { ...ingress.nginx, ...n } });
  const has = (server: string, pkg: string) => installs.some((i) => i.server_id === server && i.package === pkg && i.status === "installed");
  const nameOf = (id: string) => members.find((m) => m.id === id)?.name ?? id.slice(0, 8);

  const ha = !!ingress && ingress.mode !== "standalone";
  const needsVrrp = ha && ingress.failover === "vrrp";
  const vipCount = ingress?.mode === "active_active" ? ingress.edges.length : 1;
  const mode = MODES.find((m) => m.id === ingress?.mode);

  const setMode = (m: IngressMode) =>
    setIngress({
      mode: m,
      edges: m === "standalone" ? (ingress?.edges ?? []).slice(0, 1).map((e) => ({ ...e, role: "primary" as const })) : (ingress?.edges ?? []),
      ...(m === "standalone" ? { failover: "dns" as const, vrrp: null } : {}),
    });

  const toggleEdge = (m: Member) => {
    const cur = ingress ?? blankIngress();
    if (cur.edges.some((e) => e.server_id === m.id)) return setIngress({ edges: cur.edges.filter((e) => e.server_id !== m.id) });
    const edges = cur.mode === "standalone" ? [] : cur.edges;
    setIngress({ edges: [...edges, { server_id: m.id, role: edges.some((e) => e.role === "primary") ? "backup" : "primary" }] });
  };

  const save = useMutation({
    mutationFn: () => api("PUT", `/api/v1/clusters/${clusterId}/profile`, p),
    onSuccess: () => {
      toast("Profile saved. Apply pushes it to the edge nodes.");
      void qc.invalidateQueries({ queryKey: ["cluster-profile", clusterId] });
    },
    onError: (e) => toast(errorText(e), "error"),
  });
  const apply = useMutation({
    mutationFn: async () => {
      await api("PUT", `/api/v1/clusters/${clusterId}/profile`, p);
      return post<{ edges: Record<string, { status: string; error: string | null }>; dns: { error?: string; records: { status: string; name: string; error: string | null }[] } | null }>(
        `/api/v1/clusters/${clusterId}/profile/apply`,
      );
    },
    onSuccess: (r) => {
      const failed = Object.entries(r.edges).filter(([, v]) => v.status !== "applied");
      const n = Object.keys(r.edges).length;
      if (failed.length) toast(`Failed on ${failed.map(([id]) => nameOf(id)).join(", ")}: ${failed[0]![1].error ?? ""}`, "error");
      else if (r.dns && (r.dns.error || r.dns.records.some((x) => x.status !== "succeeded")))
        toast(`Edges applied; DNS: ${r.dns.error ?? r.dns.records.find((x) => x.status !== "succeeded")?.error ?? "not finished"}`, "error");
      else toast(`Applied to ${n} edge node${n === 1 ? "" : "s"}${r.dns ? ` · ${r.dns.records.length} DNS record${r.dns.records.length === 1 ? "" : "s"} published` : ""}`);
      void qc.invalidateQueries({ queryKey: ["cluster-profile", clusterId] });
      void qc.invalidateQueries({ queryKey: ["cluster-live", clusterId] });
    },
    onError: (e) => toast(errorText(e), "error"),
  });

  const missing = (ingress?.edges ?? []).flatMap((e) => [
    ...(has(e.server_id, "nginx") ? [] : [{ server: e.server_id, pkg: "nginx" }]),
    ...(needsVrrp && !has(e.server_id, "keepalived") ? [{ server: e.server_id, pkg: "keepalived" }] : []),
  ]);
  const installMissing = () => {
    const byPkg = new Map<string, string[]>();
    for (const m of missing) byPkg.set(m.pkg, [...(byPkg.get(m.pkg) ?? []), m.server]);
    for (const [pkg, ids] of byPkg) {
      const params: Record<string, string> = pkg === "keepalived" && ingress?.vrrp ? { vip: ingress.vrrp.vips[0] ?? "", interface: ingress.vrrp.interface } : {};
      install.mutate(
        { package: pkg, version: pkg === "nginx" ? "stable" : "distro", server_ids: ids, params },
        { onSuccess: () => toast(`Installing ${pkg} on ${ids.length} node${ids.length === 1 ? "" : "s"}`), onError: (e) => toast(errorText(e), "error") },
      );
    }
  };

  const edgeHint = ingress?.mode === "standalone" ? "Pick one node, ideally with a public IP." : ingress?.mode === "active_standby" ? "Pick two or more; one is the primary." : "Pick two or more.";

  return (
    <div className="max-w-3xl space-y-3">
      <Section title="Layout" summary={mode?.title ?? "Not set"} open>
        <div className="inline-flex rounded-lg border border-line bg-panel-2/40 p-0.5" role="radiogroup" aria-label="Ingress layout">
          {MODES.map((m) => (
            <button
              key={m.id}
              type="button"
              role="radio"
              aria-checked={ingress?.mode === m.id}
              onClick={() => setMode(m.id)}
              className={`rounded-md px-3 py-1.5 text-[13px] transition ${ingress?.mode === m.id ? "bg-panel font-medium text-accent shadow-sm" : "text-muted hover:text-fg"}`}
            >
              {m.title}
            </button>
          ))}
        </div>
        {mode ? (
          <div className="flex items-center gap-3">
            <ModeDiagram mode={mode.id} />
            <p className="text-xs text-muted">{mode.body}</p>
          </div>
        ) : (
          <p className="text-xs text-muted">Pick a layout to run Nginx on this cluster's edge nodes.</p>
        )}
      </Section>

      {ingress && (
        <Section title="Edge nodes" summary={ingress.edges.length ? ingress.edges.map((e) => nameOf(e.server_id)).join(", ") : edgeHint} open>
          <p className="text-xs text-muted">{edgeHint}</p>
          <div className="divide-y divide-line rounded-md border border-line">
            {members.map((m) => {
              const edge = ingress.edges.find((e) => e.server_id === m.id);
              return (
                <div key={m.id} className={`flex items-center gap-2.5 px-2.5 py-1.5 text-[13px] ${edge ? "bg-accent-soft/40" : ""}`}>
                  <input type="checkbox" checked={!!edge} onChange={() => toggleEdge(m)} aria-label={`Use ${m.name} as an edge`} />
                  <span className="min-w-0 flex-1 truncate font-medium">{m.name}</span>
                  {edge ? (
                    <Input
                      className="h-7 w-36 py-0 font-mono text-xs"
                      value={edge.public_ip ?? ""}
                      placeholder={m.address || "public IP"}
                      title="Public IP: the address clients and DNS use for this edge"
                      aria-label={`Public IP of ${m.name}`}
                      onChange={(e) => setIngress({ edges: ingress.edges.map((x) => (x.server_id === m.id ? { ...x, public_ip: e.target.value.trim() || null } : x)) })}
                    />
                  ) : (
                    <span className="font-mono text-xs text-faint">{m.address}</span>
                  )}
                  {has(m.id, "nginx") ? <Badge tone="ok">nginx</Badge> : <Badge>no nginx</Badge>}
                  {edge && ingress.mode === "active_standby" && (
                    <Select
                      className="h-7 w-24 py-0 text-xs"
                      value={edge.role}
                      onChange={(e) =>
                        setIngress({
                          edges: ingress.edges.map((x) => (x.server_id === m.id ? { ...x, role: e.target.value as "primary" | "backup" } : e.target.value === "primary" ? { ...x, role: "backup" } : x)),
                        })
                      }
                    >
                      <option value="primary">Primary</option>
                      <option value="backup">Backup</option>
                    </Select>
                  )}
                </div>
              );
            })}
            {!members.length && <p className="p-3 text-xs text-muted">This cluster has no nodes yet.</p>}
          </div>
        </Section>
      )}

      {ingress && (
        <Section
          title="Traffic & DNS"
          summary={
            ingress.dns
              ? `Forge DNS · TTL ${ingress.dns.ttl}s${ha && ingress.dns.auto_failover ? " · automatic failover" : ""}`
              : ha && ingress.failover === "vrrp"
                ? "Virtual IP (same LAN)"
                : "You manage DNS"
          }
          open={ha}
        >
          {!ha && (
            <p className="text-xs text-muted">
              Standalone needs no DNS from Forge: point each endpoint's domain at{" "}
              <span className="font-mono text-fg">{ingress.edges[0]?.public_ip || members.find((m) => m.id === ingress.edges[0]?.server_id)?.address || "the edge's public IP"}</span>. Or let Forge keep the records:
            </p>
          )}
          {ha && (
            <Row label="Failover" hint="DNS works anywhere, including public edges.">
              <Select
                value={ingress.failover}
                onChange={(e) => {
                  const f = e.target.value as "vrrp" | "dns";
                  setIngress({ failover: f, vrrp: f === "vrrp" ? (ingress.vrrp ?? { vips: [], interface: "eth0", router_id: 51 }) : null });
                }}
              >
                <option value="dns">DNS ({ingress.mode === "active_active" ? "round-robin over healthy edges" : "record moves to a backup"})</option>
                <option value="vrrp">Virtual IP (keepalived, edges on one LAN)</option>
              </Select>
            </Row>
          )}
          <Row label="Forge-managed records" hint={dnsNodes.length ? "A/AAAA records for every endpoint domain." : "Install PowerDNS from the App Store first."}>
            <Select
              value={ingress.dns?.server_id ?? ""}
              disabled={!dnsNodes.length}
              onChange={(e) =>
                setIngress({ dns: e.target.value ? { ttl: 60, auto_failover: true, ...ingress.dns, server_id: e.target.value } : null })
              }
            >
              <option value="">Off (I manage DNS myself)</option>
              {dnsNodes.map((n) => (
                <option key={n.id} value={n.id}>
                  PowerDNS on {nodeName(n)}
                </option>
              ))}
            </Select>
          </Row>
          {ingress.dns && (
            <>
              <Row label="Record TTL (s)" hint="Lower = faster failover.">
                <Input className="w-28" type="number" min={30} max={86400} value={ingress.dns.ttl} onChange={(e) => setIngress({ dns: { ...ingress.dns!, ttl: Number(e.target.value) } })} />
              </Row>
              {ha && (
                <Toggle checked={ingress.dns.auto_failover} onChange={(v) => setIngress({ dns: { ...ingress.dns!, auto_failover: v } })}>
                  Check the edges every 15 s and move records off an edge after 3 failed checks
                </Toggle>
              )}
              {ingress.edges.some((e) => !e.public_ip) && <p className="text-xs text-warn">Set the public IP of every edge (Edge nodes above).</p>}
              <p className="text-[11px] text-faint">Zones, nameservers and other records are yours to manage on the DNS page; Forge only writes the endpoint records.</p>
            </>
          )}
          {needsVrrp && (
            <>
              <Row label={vipCount === 1 ? "Virtual IP" : `Virtual IPs (${vipCount})`} hint="Free addresses on the edges' LAN.">
                <Input
                  className="font-mono"
                  value={(ingress.vrrp?.vips ?? []).join(", ")}
                  onChange={(e) => setIngress({ vrrp: { interface: "eth0", router_id: 51, ...ingress.vrrp, vips: csv(e.target.value) } })}
                  placeholder={vipCount === 1 ? "10.0.0.100" : "10.0.0.100, 10.0.0.101"}
                />
              </Row>
              <Row label="Interface · router id">
                <div className="flex gap-2">
                  <Input className="font-mono" value={ingress.vrrp?.interface ?? "eth0"} onChange={(e) => setIngress({ vrrp: { vips: [], router_id: 51, ...ingress.vrrp, interface: e.target.value } })} />
                  <Input className="w-24" type="number" min={1} max={255} value={ingress.vrrp?.router_id ?? 51} onChange={(e) => setIngress({ vrrp: { vips: [], interface: "eth0", ...ingress.vrrp, router_id: Number(e.target.value) } })} />
                </div>
              </Row>
            </>
          )}
        </Section>
      )}

      {ingress && (
        <Section title="Nginx tuning" summary={`${ingress.nginx.worker_connections} connections · ${ingress.nginx.client_max_body_mb} MB body${ingress.nginx.gzip ? " · gzip" : ""}`}>
          <div className="grid gap-x-4 gap-y-2.5 sm:grid-cols-2">
            {(
              [
                ["Worker connections", "worker_connections", 64],
                ["Keep-alive (s)", "keepalive_timeout_s", 0],
                ["Upstream timeout (s)", "proxy_read_timeout_s", 1],
                ["Max body (MB)", "client_max_body_mb", 1],
                ["Rate limit (req/s per IP, 0 = off)", "rate_limit", 0],
              ] as const
            ).map(([label, key, min]) => (
              <label key={key} className="flex items-center justify-between gap-3 text-[13px]">
                <span className="text-muted">{label}</span>
                <Input className="h-8 w-28" type="number" min={min} value={ingress.nginx[key]} onChange={(e) => setNginx({ [key]: Number(e.target.value) })} />
              </label>
            ))}
            <label className="flex items-center justify-between gap-3 text-[13px]">
              <span className="text-muted">Access log</span>
              <Select className="h-8 w-28" value={ingress.nginx.access_log} onChange={(e) => setNginx({ access_log: e.target.value as NginxSettings["access_log"] })}>
                <option value="combined">Combined</option>
                <option value="json">JSON</option>
                <option value="off">Off</option>
              </Select>
            </label>
          </div>
          <div className="flex flex-wrap gap-x-5 gap-y-1">
            <Toggle checked={ingress.nginx.gzip} onChange={(v) => setNginx({ gzip: v })}>gzip</Toggle>
            <Toggle checked={ingress.nginx.hsts} onChange={(v) => setNginx({ hsts: v })}>HSTS on HTTPS</Toggle>
            <Toggle checked={ingress.nginx.server_tokens} onChange={(v) => setNginx({ server_tokens: v })}>Show Nginx version</Toggle>
          </div>
          <details className="rounded-md border border-line">
            <summary className="cursor-pointer px-2.5 py-1.5 text-xs text-muted">Advanced</summary>
            <div className="space-y-3 border-t border-line p-2.5">
              <Row label="Trusted proxies" hint="A load balancer in front of the edges.">
                <Input className="font-mono" value={ingress.nginx.real_ip_from.join(", ")} onChange={(e) => setNginx({ real_ip_from: csv(e.target.value) })} placeholder="10.0.0.0/8" />
              </Row>
              <div className="space-y-1">
                <span className="text-[13px]">Custom http {"{}"} directives</span>
                <TextArea rows={4} spellCheck={false} className="font-mono text-xs" value={ingress.nginx.custom} onChange={(e) => setNginx({ custom: e.target.value })} placeholder="proxy_buffers 16 16k;" />
                <p className="text-[11px] text-faint">Checked with nginx -t on every edge; the previous config comes back if the check fails.</p>
              </div>
            </div>
          </details>
        </Section>
      )}

      {missing.length > 0 && (
        <div className="flex items-center gap-2 rounded-lg border border-warn/40 bg-warn/10 px-3 py-2 text-[13px]">
          <AlertTriangle className="size-4 shrink-0 text-warn" />
          <span className="flex-1">Missing: {missing.map((m) => `${m.pkg} on ${nameOf(m.server)}`).join(", ")}</span>
          <Button disabled={install.isPending} onClick={installMissing}>
            <Cpu className="size-4" /> Install
          </Button>
        </div>
      )}

      {!dirty && (
        <div className="flex items-center gap-2 border-t border-line pt-3">
          <span className="mr-auto flex min-w-0 items-center gap-1.5 text-xs text-muted">
            <Shield className="size-3.5 shrink-0" /> Applying needs an MFA session; changes are audited.
          </span>
          <Button variant="primary" disabled={apply.isPending || !ingress?.edges.length} title={ingress?.edges.length ? undefined : "Choose at least one edge node"} onClick={() => apply.mutate()}>
          {apply.isPending ? <Loader2 className="size-4 animate-spin" /> : <Rocket className="size-4" />} {apply.isPending ? "Applying…" : "Apply"}
        </Button>
        </div>
      )}
      <UnsavedBar
        dirty={dirty}
        summary={<span className="text-muted">Applying needs an MFA session.</span>}
        onDiscard={() => setP(saved)}
        saving={save.isPending}
        onSave={() => save.mutate()}
        extra={<Button  disabled={apply.isPending || !ingress?.edges.length} title={ingress?.edges.length ? undefined : "Choose at least one edge node"} onClick={() => apply.mutate()}>
          {apply.isPending ? <Loader2 className="size-4 animate-spin" /> : <Rocket className="size-4" />} {apply.isPending ? "Applying…" : "Save & apply"}
        </Button>}
      />
    </div>
  );
}
