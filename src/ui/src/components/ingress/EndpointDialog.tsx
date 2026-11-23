import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { CheckCircle2, Code2, ShieldCheck, Trash2 } from "lucide-react";
import { useEffect, useMemo, useState } from "react";

import { api, get, post } from "../../api/client";
import { useServices, useInstallations, useServers } from "../../api/hooks";
import { errorText, useToast } from "../../app/toast";
import { Button, Field, Input, Modal, Mono, Select, TextArea } from "../ui";
import { TargetEditor } from "./TargetEditor";
import { BALANCE_LABELS, type Balance, type Certificate, covers, type DnsCfg, type Member, type ProfileResponse, type Site, type SiteSpec, type Target } from "./types";

const blank = (): SiteSpec => ({
  name: "",
  domains: [],
  upstreams: [],
  standby: [],
  balance: "round_robin",
  tls: "off",
  acme_email: "",
  force_https: true,
  websockets: true,
  max_body_mb: 25,
  rate_limit: 0,
  custom: "",
});
/** `POST /api/v1/sites/preview`: `config` is the variant the edges run. */
interface Preview {
  variant?: "http" | "https";
  config?: string;
  http: string;
  https: string | null;
  custom: string;
}
const lines = (s: string) => s.split(/[\n,]/).map((x) => x.trim()).filter(Boolean);

/** Pre-filled values for a new endpoint (e.g. **Expose through ingress** on a service page). */
export interface EndpointInitial {
  name?: string;
  targets?: Target[];
  balance?: Balance;
  /** Hide the Balancing select (one upstream: the rule changes nothing). */
  hideBalance?: boolean;
  /** Focus the first target's port (no port could be guessed). */
  focusPort?: boolean;
}

/**
 * The endpoint editor (New endpoint / Endpoint · name) as a modal. Used by the
 * Ingress Endpoints tab and by a service page's **Expose through ingress**.
 * Saving never applies; the caller tells the user where **Apply to edges** is.
 */
export function EndpointDialog({
  site,
  clusterId,
  members,
  initial,
  onClose,
  onDelete,
  onSaved,
}: {
  site?: Site;
  clusterId?: string;
  members: Member[];
  initial?: EndpointInitial;
  onClose: () => void;
  onDelete?: (id: string) => void;
  onSaved?: (s: Site) => void;
}) {
  const qc = useQueryClient();
  const toast = useToast();
  const nodes = (useServers().data ?? []).filter((n) => n.status !== "revoked");
  const installs = useInstallations().data ?? [];
  const apps = useServices(undefined, clusterId ?? null).data ?? [];
  const certs = useQuery({ queryKey: ["certificates"], queryFn: () => get<Certificate[]>("/api/v1/certificates") }).data ?? [];
  const [spec, setSpec] = useState<SiteSpec>(() => (site ? { ...site.config.spec, name: site.name } : { ...blank(), ...(initial?.name ? { name: initial.name } : {}), ...(initial?.balance ? { balance: initial.balance } : {}) }));
  const [edges, setEdges] = useState<string[]>(site?.config.edges ?? []);
  const [targets, setTargets] = useState<Target[]>(site?.config.targets ?? initial?.targets ?? []);
  const [standbyTargets, setStandbyTargets] = useState<Target[]>(site?.config.standby_targets ?? []);
  const typed = !!clusterId || targets.length > 0;
  const [dnsOn, setDnsOn] = useState(!!site?.config.dns);
  const [dns, setDns] = useState<DnsCfg>(site?.config.dns ?? { server_id: "", zone: "", ttl: 300, mode: "round_robin", addresses: [] });
  const [tab, setTab] = useState<"routing" | "tls" | "custom" | "dns" | "preview">("routing");
  const [preview, setPreview] = useState<Preview>();
  const [appId, setAppId] = useState("");
  const [appPort, setAppPort] = useState("8000");
  const set = (p: Partial<SiteSpec>) => setSpec((s) => ({ ...s, ...p }));
  const has = (server: string, pkg: string) => installs.some((i) => i.server_id === server && i.package === pkg && i.status === "installed");
  const nginxNodes = nodes.filter((n) => has(n.id, "nginx"));
  const dnsNodes = nodes.filter((n) => has(n.id, "powerdns"));
  // The cluster profile's DNS (with health failover) owns the records when set.
  const profileDns = useQuery({
    queryKey: ["cluster-profile", clusterId],
    queryFn: () => get<ProfileResponse>(`/api/v1/clusters/${clusterId}/profile`),
    enabled: !!clusterId,
  }).data?.profile?.ingress?.dns;

  useEffect(() => {
    if (tab !== "preview") return;
    post<Preview>("/api/v1/sites/preview", spec).then(setPreview, (e) => toast(errorText(e), "error"));
  }, [tab, spec, toast]);

  const fromApp = () => {
    const a = apps.find((x) => x.id === appId);
    if (!a) return;
    const hostOf = (id: string) => nodes.find((n) => n.id === id)?.hostname ?? id;
    set({
      upstreams: a.instances.filter((i) => i.role === "active").map((i) => `${hostOf(i.server_id)}:${appPort}`),
      standby: a.instances.filter((i) => i.role === "standby").map((i) => `${hostOf(i.server_id)}:${appPort}`),
    });
  };

  const save = useMutation({
    mutationFn: () => {
      const body = {
        ...spec,
        edges: clusterId ? [] : edges,
        cluster_id: clusterId ?? null,
        targets: typed ? targets : [],
        standby_targets: typed ? standbyTargets : [],
        dns: dnsOn ? dns : null,
      };
      return site ? api<Site>("PUT", `/api/v1/sites/${site.id}`, body) : post<Site>("/api/v1/sites", body);
    },
    onSuccess: (s) => {
      if (onSaved) onSaved(s);
      else toast("Saved — press Apply to push it to the edge nodes");
      void qc.invalidateQueries({ queryKey: ["sites"] });
      if (clusterId) void qc.invalidateQueries({ queryKey: ["cluster-routing", clusterId] });
      onClose();
    },
    onError: (e) => toast(errorText(e), "error"),
  });
  const tabs = useMemo(() => ["routing", "tls", "custom", "dns", "preview"] as const, []);

  return (
    <Modal open onOpenChange={(o) => !o && onClose()} title={site ? `Endpoint · ${site.name}` : "New endpoint"} description="Saved here, applied to the edge nodes with Apply." wide>
      <div className="mb-4 flex gap-1 border-b border-line">
        {tabs.map((t) => (
          <button key={t} onClick={() => setTab(t)} className={`-mb-px border-b-2 px-3 py-1.5 text-[13px] capitalize ${tab === t ? "border-accent text-accent" : "border-transparent text-muted hover:text-fg"}`}>
            {t === "tls" ? "TLS & limits" : t === "custom" ? "Custom block" : t === "dns" ? "DNS" : t}
          </button>
        ))}
      </div>

      {tab === "routing" && (
        <div className="view-enter space-y-3">
          <div className="grid gap-3 sm:grid-cols-2">
            <Field label="Name" hint="Used for file names.">
              <Input value={spec.name} disabled={!!site} onChange={(e) => set({ name: e.target.value.toLowerCase() })} pattern="[a-z][a-z0-9\-]{0,59}" placeholder="shop" required />
            </Field>
            <Field label="Domains" hint="Comma or one per line; the first is primary.">
              <Input value={spec.domains.join(", ")} onChange={(e) => set({ domains: lines(e.target.value) })} placeholder="shop.example.com, www.shop.example.com" />
            </Field>
          </div>
          {typed ? (
            <div className="space-y-4 rounded-lg border border-line bg-panel-2/30 p-3">
              <TargetEditor label="Active targets" hint="Where requests go. A service is reached on every replica; its standbys are Nginx backups." value={targets} onChange={setTargets} members={members} apps={apps} autoFocusPort={initial?.focusPort} />
              <TargetEditor label="Standby targets" hint="Only used when every active target is down." value={standbyTargets} onChange={setStandbyTargets} members={members} apps={apps} />
            </div>
          ) : (
            <>
          <div className="flex flex-wrap items-end gap-2 rounded-lg border border-line bg-panel-2/40 p-3">
            <Field label="Fill upstreams from a service">
              <Select value={appId} onChange={(e) => setAppId(e.target.value)}>
                <option value="">Choose…</option>
                {apps.map((a) => (
                  <option key={a.id} value={a.id}>
                    {a.name} ({a.instances.length} nodes)
                  </option>
                ))}
              </Select>
            </Field>
            <Field label="Port">
              <Input className="w-24" value={appPort} onChange={(e) => setAppPort(e.target.value)} />
            </Field>
            <Button type="button" disabled={!appId} title={appId ? undefined : "Choose a service first"} onClick={fromApp}>
              Use
            </Button>
          </div>
          <div className="grid gap-3 sm:grid-cols-2">
            <Field label="Active upstreams" hint="host:port, one per line.">
              <TextArea rows={3} className="font-mono text-xs" value={spec.upstreams.join("\n")} onChange={(e) => set({ upstreams: lines(e.target.value) })} placeholder="10.0.0.11:8000" />
            </Field>
            <Field label="Standby upstreams" hint="Used only when every active one is down.">
              <TextArea rows={3} className="font-mono text-xs" value={spec.standby.join("\n")} onChange={(e) => set({ standby: lines(e.target.value) })} placeholder="10.0.0.12:8000" />
            </Field>
          </div>
              <button type="button" className="text-xs text-accent hover:underline" onClick={() => setTargets([{ kind: "external", address: "" }])}>
                Use typed targets instead
              </button>
            </>
          )}
          {!initial?.hideBalance && (
            <Field label="Balancing" hint="How Nginx spreads requests over the replicas. IP hash keeps a client on one replica.">
              <Select value={spec.balance} onChange={(e) => set({ balance: e.target.value as Balance })}>
                {(Object.keys(BALANCE_LABELS) as Balance[]).map((b) => (
                  <option key={b} value={b}>
                    {BALANCE_LABELS[b]}
                  </option>
                ))}
              </Select>
            </Field>
          )}
          {clusterId ? (
            <p className="rounded-lg border border-line bg-panel-2/40 p-3 text-xs text-muted">Edge nodes come from the cluster's ingress profile; changing the profile moves every endpoint with it.</p>
          ) : (
          <Field label="Edge nodes (Nginx installed)" hint={nginxNodes.length ? "The same config goes to every edge; use Keepalived or DNS for failover between them." : "No node has Nginx yet — install it from the App Store."}>
            <div className="flex flex-wrap gap-1.5">
              {nginxNodes.map((n) => {
                const on = edges.includes(n.id);
                return (
                  <button key={n.id} type="button" onClick={() => setEdges(on ? edges.filter((x) => x !== n.id) : [...edges, n.id])} className={`rounded-full border px-2.5 py-0.5 text-xs ${on ? "border-accent bg-accent-soft text-accent" : "border-line text-muted"}`}>
                    {on && <CheckCircle2 className="mr-1 inline size-3" />}
                    {n.hostname}
                  </button>
                );
              })}
            </div>
          </Field>
          )}
        </div>
      )}

      {tab === "tls" && (
        <div className="view-enter space-y-3">
          <Field label="TLS">
            <Select value={spec.tls} onChange={(e) => set({ tls: e.target.value as SiteSpec["tls"] })}>
              <option value="off">Off (HTTP only)</option>
              <option value="acme">Let's Encrypt (free; Forge renews it automatically before it expires)</option>
              <option value="uploaded">Uploaded certificate (paid or company CA)</option>
            </Select>
          </Field>
          {spec.tls === "uploaded" && (
            <>
              <Field label="Certificate" hint={certs.length ? "Must cover every domain of this endpoint." : "Upload one in Ingress → Certificates first."}>
                <Select value={spec.cert_id ?? ""} onChange={(e) => set({ cert_id: e.target.value })}>
                  <option value="">Choose…</option>
                  {certs.map((c) => {
                    const ok = spec.domains.every((d) => covers(c.domains, d));
                    return (
                      <option key={c.id} value={c.id} disabled={!ok || c.days_left < 0}>
                        {c.name} · {c.domains.slice(0, 2).join(", ")}
                        {c.domains.length > 2 ? "…" : ""} · {c.days_left < 0 ? "expired" : `${c.days_left} days left`}
                        {ok ? "" : " · does not cover these domains"}
                      </option>
                    );
                  })}
                </Select>
              </Field>
              <label className="flex items-center gap-2 text-[13px]">
                <input type="checkbox" checked={spec.force_https} onChange={(e) => set({ force_https: e.target.checked })} /> Redirect HTTP to HTTPS (+ HSTS)
              </label>
            </>
          )}
          {spec.tls === "acme" && (
            <>
              <Field label="Contact email">
                <Input type="email" value={spec.acme_email} onChange={(e) => set({ acme_email: e.target.value })} required />
              </Field>
              <label className="flex items-center gap-2 text-[13px]">
                <input type="checkbox" checked={spec.force_https} onChange={(e) => set({ force_https: e.target.checked })} /> Redirect HTTP to HTTPS (+ HSTS)
              </label>
            </>
          )}
          <label className="flex items-center gap-2 text-[13px]">
            <input type="checkbox" checked={spec.websockets} onChange={(e) => set({ websockets: e.target.checked })} /> WebSockets
          </label>
          <div className="grid gap-3 sm:grid-cols-2">
            <Field label="Max request body (MB)">
              <Input type="number" min={1} max={10240} value={spec.max_body_mb} onChange={(e) => set({ max_body_mb: Number(e.target.value) })} />
            </Field>
            <Field label="Rate limit (requests/s per IP, 0 = off)">
              <Input type="number" min={0} max={10000} value={spec.rate_limit} onChange={(e) => set({ rate_limit: Number(e.target.value) })} />
            </Field>
          </div>
        </div>
      )}

      {tab === "custom" && (
        <div className="view-enter space-y-2">
          <p className="text-xs text-muted">
            Nginx directives included in this site's <Mono>server</Mono> block(s) from <Mono>/etc/nginx/forge/custom/{spec.name || "<site>"}.conf</Mono> — for anything the
            form doesn't cover (extra locations, headers, caching, basic auth…). Forge never overwrites it, checks it with <Mono>nginx -t</Mono> and restores the previous
            version if the test fails. Every change is recorded in the audit log.
          </p>
          <TextArea
            rows={14}
            spellCheck={false}
            className="font-mono text-xs leading-relaxed"
            value={spec.custom}
            onChange={(e) => set({ custom: e.target.value })}
            placeholder={"location /static/ {\n    alias /srv/forge/apps/staging/web/static/;\n    expires 7d;\n}\nadd_header X-Frame-Options SAMEORIGIN;"}
          />
        </div>
      )}

      {tab === "dns" && profileDns && (
        <div className="view-enter rounded-lg border border-line bg-panel-2/40 p-3 text-xs text-muted">
          DNS for this endpoint is published by the cluster's ingress profile ({nodes.find((n) => n.id === profileDns.server_id)?.hostname ?? "PowerDNS node"},
          TTL {profileDns.ttl}s{profileDns.auto_failover ? ", records move off failed edges automatically" : ""}). Change it on the Profile tab.
        </div>
      )}
      {tab === "dns" && !profileDns && (
        <div className="view-enter space-y-3">
          <label className="flex items-center gap-2 text-[13px]">
            <input type="checkbox" checked={dnsOn} onChange={(e) => setDnsOn(e.target.checked)} /> Publish DNS records with PowerDNS when applying
          </label>
          {dnsOn && (
            <>
              <div className="grid gap-3 sm:grid-cols-2">
                <Field label="DNS node (PowerDNS installed)">
                  <Select value={dns.server_id} onChange={(e) => setDns({ ...dns, server_id: e.target.value })}>
                    <option value="">Choose…</option>
                    {dnsNodes.map((n) => (
                      <option key={n.id} value={n.id}>
                        {n.hostname}
                      </option>
                    ))}
                  </Select>
                </Field>
                <Field label="Zone" hint="With trailing dot.">
                  <Input value={dns.zone} onChange={(e) => setDns({ ...dns, zone: e.target.value })} placeholder="example.com." />
                </Field>
              </div>
              <Field label="Edge addresses" hint="Public IPs of the edge nodes, active first.">
                <Input value={dns.addresses.join(", ")} onChange={(e) => setDns({ ...dns, addresses: lines(e.target.value) })} placeholder="203.0.113.10, 203.0.113.11" />
              </Field>
              <div className="grid gap-3 sm:grid-cols-2">
                <Field label="Mode">
                  <Select value={dns.mode} onChange={(e) => setDns({ ...dns, mode: e.target.value as DnsCfg["mode"] })}>
                    <option value="round_robin">Round robin — all addresses</option>
                    <option value="active_standby">Active/standby — first address only</option>
                  </Select>
                </Field>
                <Field label="TTL (s)">
                  <Input type="number" min={30} value={dns.ttl} onChange={(e) => setDns({ ...dns, ttl: Number(e.target.value) })} />
                </Field>
              </div>
            </>
          )}
        </div>
      )}

      {tab === "preview" && (
        <div className="view-enter space-y-2">
          <p className="flex items-center gap-1.5 text-xs text-muted">
            <ShieldCheck className="size-3.5 text-ok" /> Rendered by the same code the edge nodes run. <Code2 className="ml-2 size-3.5" /> {spec.tls !== "off" ? "HTTPS config (in use with TLS)." : "HTTP config (TLS is off)."}
          </p>
          <pre className="log-console h-[50vh] overflow-auto rounded-lg bg-term p-3 font-mono text-[11.5px] leading-relaxed text-term-fg">
            {preview ? (preview.config ?? (spec.tls !== "off" ? preview.https : preview.http) ?? "") : "Complete the routing tab to preview."}
          </pre>
        </div>
      )}

      <div className="mt-5 flex justify-between">
        {site && onDelete ? (
          <Button variant="danger" onClick={() => { onDelete(site.id); onClose(); }}>
            <Trash2 className="size-4" /> Delete
          </Button>
        ) : (
          <span />
        )}
        <div className="flex gap-2">
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" disabled={save.isPending} onClick={() => save.mutate()}>
            Save
          </Button>
        </div>
      </div>
    </Modal>
  );
}
