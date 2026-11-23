import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { AlertTriangle, FileKey2, Lock, Plus, RefreshCw, ShieldCheck, Trash2, Upload } from "lucide-react";
import { type ReactNode, useState } from "react";

import { api, get, post } from "../../api/client";
import { useServers } from "../../api/hooks";
import { errorText, useToast } from "../../app/toast";
import { Button, Confirm, Empty, Field, Input, Modal, Skeleton, TextArea, ago } from "../ui";
import { AddAlertButton } from "../alerts/AddAlert";
import type { Certificate } from "./types";

/** Let's Encrypt state on one edge (checked by Forge every 6 h). */
interface EdgeCert {
  days_left?: number;
  expiry?: string;
  checked_at: number;
  renewed_at?: number;
  renew_error?: string | null;
  error?: string;
  missing?: boolean;
}
interface Site {
  id: string;
  name: string;
  config: { spec: { domains: string[]; tls: string; cert_id?: string | null }; edges: string[]; cluster_id?: string | null };
  edge_status: Record<string, { status: string; cert?: EdgeCert }>;
}

type Tone = "ok" | "warn" | "bad" | "idle";
const toneText: Record<Tone, string> = { ok: "text-ok", warn: "text-warn", bad: "text-bad", idle: "text-faint" };
const toneBar: Record<Tone, string> = { ok: "bg-ok", warn: "bg-warn", bad: "bg-bad", idle: "bg-faint/40" };
const daysTone = (d?: number): Tone => (d === undefined ? "idle" : d < 0 ? "bad" : d <= 14 ? "warn" : "ok");
const daysText = (d?: number) => (d === undefined ? "not checked yet" : d < 0 ? "expired" : `${d} day${d === 1 ? "" : "s"} left`);

/** Reads a dropped / chosen PEM file into a text field. */
function PemField({ label, hint, value, onChange, placeholder }: { label: string; hint?: string; value: string; onChange: (v: string) => void; placeholder: string }) {
  return (
    <Field label={label} hint={hint}>
      <div className="space-y-1">
        <TextArea rows={5} spellCheck={false} className="font-mono text-[11px]" value={value} onChange={(e) => onChange(e.target.value)} placeholder={placeholder} />
        <label className="inline-flex cursor-pointer items-center gap-1 text-xs text-accent hover:underline">
          <Upload className="size-3.5" /> Choose a file
          <input
            type="file"
            accept=".pem,.crt,.cer,.key,.txt"
            className="hidden"
            onChange={(e) => {
              const f = e.target.files?.[0];
              if (f) void f.text().then(onChange);
              e.target.value = "";
            }}
          />
        </label>
      </div>
    </Field>
  );
}

/** Validity used up, as a thin bar (full = just issued). */
function Validity({ from, to, tone }: { from: number; to: number; tone: Tone }) {
  const left = Math.max(0, Math.min(1, (to - Date.now()) / Math.max(1, to - from)));
  return (
    <div className="h-1 w-full overflow-hidden rounded-full bg-panel-2" title={`${Math.round(left * 100)}% of its validity left`}>
      <div className={`h-full rounded-full ${toneBar[tone]}`} style={{ width: `${left * 100}%` }} />
    </div>
  );
}

const Section = ({ title, hint, actions, children }: { title: string; hint: string; actions?: ReactNode; children: ReactNode }) => (
  <section className="overflow-hidden rounded-lg border border-line bg-panel">
    <div className="card-head flex items-center gap-2 border-b border-line px-3 py-2">
      <div className="min-w-0 flex-1">
        <div className="text-[13px] font-semibold">{title}</div>
        <div className="truncate text-[11px] text-muted">{hint}</div>
      </div>
      {actions}
    </div>
    {children}
  </section>
);

/**
 * Every certificate the cluster's endpoints serve: Let's Encrypt (issued and
 * renewed on the edges) and certificates you uploaded (from your CA).
 */
export function Certificates({ clusterId }: { clusterId?: string }) {
  const qc = useQueryClient();
  const toast = useToast();
  const nodes = useServers().data ?? [];
  const list = useQuery({ queryKey: ["certificates"], queryFn: () => get<Certificate[]>("/api/v1/certificates") });
  const sites = useQuery({ queryKey: ["sites"], queryFn: () => get<Site[]>("/api/v1/sites") });
  const [open, setOpen] = useState(false);
  const [form, setForm] = useState({ name: "", chain_pem: "", key_pem: "" });
  const [dropping, setDropping] = useState<Certificate>();
  const [renewing, setRenewing] = useState<string>();
  const host = (id: string) => nodes.find((n) => n.id === id)?.hostname ?? id.slice(0, 8);

  const upload = useMutation({
    mutationFn: () => post<Certificate>("/api/v1/certificates", form),
    onSuccess: (c) => {
      toast(`Certificate “${c.name}” uploaded`);
      setOpen(false);
      setForm({ name: "", chain_pem: "", key_pem: "" });
      void qc.invalidateQueries({ queryKey: ["certificates"] });
    },
    onError: (e) => toast(errorText(e), "error"),
  });
  const drop = useMutation({
    mutationFn: (id: string) => api("DELETE", `/api/v1/certificates/${id}`),
    onSuccess: () => {
      toast("Certificate deleted");
      setDropping(undefined);
      void qc.invalidateQueries({ queryKey: ["certificates"] });
    },
    onError: (e) => toast(errorText(e), "error"),
  });
  const renew = useMutation({
    mutationFn: (id: string) => post<{ ok: boolean; certificates: Record<string, EdgeCert | null> }>(`/api/v1/sites/${id}/renew`),
    onMutate: (id) => setRenewing(id),
    onSettled: () => {
      setRenewing(undefined);
      void qc.invalidateQueries({ queryKey: ["sites"] });
    },
    onSuccess: (r) => {
      const bad = Object.entries(r.certificates).find(([, c]) => c?.renew_error);
      if (bad) toast(`Renewal failed on ${host(bad[0])}: ${bad[1]?.renew_error}`, "error");
      else toast("Certificate renewed on every edge");
    },
    onError: (e) => toast(errorText(e), "error"),
  });

  if (list.isLoading || sites.isLoading) return <Skeleton />;
  const certs = list.data ?? [];
  const acme = (sites.data ?? []).filter((s) => s.config.spec.tls === "acme" && (!clusterId || s.config.cluster_id === clusterId));
  const acmeDays = (s: Site) => {
    const d = s.config.edges.map((e) => s.edge_status[e]?.cert?.days_left).filter((x): x is number => typeof x === "number");
    return d.length ? Math.min(...d) : undefined;
  };
  const attention = acme.filter((s) => daysTone(acmeDays(s)) !== "ok" || s.config.edges.some((e) => s.edge_status[e]?.cert?.renew_error)).length + certs.filter((c) => c.days_left <= 14).length;

  return (
    <div className="max-w-5xl space-y-3">
      {/* Summary */}
      <div className="grid gap-2 sm:grid-cols-3">
        <Stat icon={<Lock className="size-4" />} label="Let's Encrypt endpoints" value={acme.length} hint="renewed automatically at 5 days left" />
        <Stat icon={<FileKey2 className="size-4" />} label="Uploaded certificates" value={certs.length} hint="renewed by you, from your CA" />
        <Stat icon={attention ? <AlertTriangle className="size-4" /> : <ShieldCheck className="size-4" />} label="Need attention" value={attention} hint={attention ? "expiring within 14 days, expired or failing" : "all certificates are healthy"} tone={attention ? "warn" : "ok"} />
      </div>

      {/* Let's Encrypt */}
      <Section title="Let's Encrypt" hint="Issued on each edge. Forge checks every 6 hours and renews at 5 days or fewer left.">
        {acme.length === 0 ? (
          <p className="px-3 py-3 text-xs text-faint">No endpoint uses Let's Encrypt. Choose it under TLS when you edit an endpoint.</p>
        ) : (
          <div className="divide-y divide-line">
            {acme.map((s) => {
              const min = acmeDays(s);
              return (
                <div key={s.id} className="flex flex-wrap items-center gap-x-3 gap-y-1.5 px-3 py-2 text-[13px]">
                  <Lock className={`size-4 shrink-0 ${toneText[daysTone(min)]}`} />
                  <div className="min-w-0 flex-1">
                    <div className="truncate font-medium">{s.config.spec.domains.join(", ")}</div>
                    <div className="flex flex-wrap gap-1 pt-0.5">
                      {s.config.edges.map((e) => {
                        const c = s.edge_status[e]?.cert;
                        const t: Tone = c?.renew_error || c?.error ? "bad" : daysTone(c?.days_left);
                        return (
                          <span key={e} title={c?.renew_error ?? c?.error ?? (c?.renewed_at ? `renewed ${ago(c.renewed_at)}` : c?.checked_at ? `checked ${ago(c.checked_at)}` : undefined)} className="inline-flex items-center gap-1 rounded border border-line px-1.5 py-px text-[11px]">
                            <span className={`size-1.5 rounded-full ${toneBar[t]}`} />
                            {host(e)}
                            <span className="text-faint">{c?.renew_error ? "renewal failing" : c?.missing ? "not issued" : daysText(c?.days_left)}</span>
                          </span>
                        );
                      })}
                    </div>
                  </div>
                  <Button className="h-7 px-2 text-xs" disabled={renewing === s.id} onClick={() => renew.mutate(s.id)} title="Renew now on every edge">
                    <RefreshCw className={`size-3.5 ${renewing === s.id ? "animate-spin" : ""}`} /> Renew now
                  </Button>
                </div>
              );
            })}
          </div>
        )}
      </Section>

      {/* Uploaded */}
      <Section
        title="Uploaded"
        hint="Certificates from your CA, picked per endpoint. The key is encrypted at rest and sent only to edge nodes."
        actions={
          <Button variant="primary" className="h-7 px-2.5 text-xs" onClick={() => setOpen(true)}>
            <Plus className="size-3.5" /> Upload
          </Button>
        }
      >
        {certs.length === 0 ? (
          <div className="p-3">
            <Empty icon={<FileKey2 className="size-5" />} title="No uploaded certificates">
              Upload your certificate chain (PEM) and its private key, then choose it under TLS on an endpoint.
            </Empty>
          </div>
        ) : (
          <div className="divide-y divide-line">
            {certs.map((c) => {
              const t = daysTone(c.days_left);
              return (
                <div key={c.id} className="grid gap-x-4 gap-y-1 px-3 py-2.5 text-[13px] md:grid-cols-[minmax(0,1.4fr)_minmax(0,1fr)_auto] md:items-center">
                  <div className="min-w-0">
                    <div className="flex items-center gap-1.5">
                      <ShieldCheck className={`size-4 shrink-0 ${toneText[t]}`} />
                      <span className="truncate font-medium">{c.name}</span>
                    </div>
                    <div className="truncate pl-5.5 text-xs text-muted" title={c.domains.join(", ")}>
                      {c.domains.join(", ")}
                    </div>
                    <div className="truncate pl-5.5 text-[11px] text-faint">{c.issuer}</div>
                  </div>
                  <div className="min-w-0 space-y-1">
                    <div className="flex items-baseline justify-between gap-2 text-xs">
                      <span className={toneText[t]}>{daysText(c.days_left)}</span>
                      <span className="text-faint">until {new Date(c.not_after).toLocaleDateString()}</span>
                    </div>
                    <Validity from={c.not_before} to={c.not_after} tone={t} />
                    <div className="truncate text-[11px] text-faint">{c.used_by.length ? `Used by ${c.used_by.join(", ")}` : "Not used by any endpoint"}</div>
                  </div>
                  <div className="flex items-center justify-end gap-1">
                    <AddAlertButton label="Alert" prefill={{ place: "certificate", templateId: "certificate-expires-14d", targets: { certificates: [c.id] } }} title={`Create an expiry alert for ${c.name}`} />
                    <button
                      type="button"
                      aria-label={`Delete ${c.name}`}
                      title={c.used_by.length ? "In use by endpoints: switch them first" : "Delete"}
                      disabled={c.used_by.length > 0}
                      onClick={() => setDropping(c)}
                      className="rounded p-1.5 text-faint hover:bg-panel-2 hover:text-bad disabled:opacity-40"
                    >
                      <Trash2 className="size-4" />
                    </button>
                  </div>
                </div>
              );
            })}
          </div>
        )}
      </Section>

      <Modal open={open} onOpenChange={setOpen} title="Upload a certificate" description="Checked before it is saved: the key must match, and the certificate must not be expired.">
        <form
          className="space-y-3"
          onSubmit={(e) => {
            e.preventDefault();
            upload.mutate();
          }}
        >
          <Field label="Name">
            <Input required maxLength={100} value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })} placeholder="shop.example.com 2026" />
          </Field>
          <PemField label="Certificate chain" hint="Your certificate first, then the intermediates." value={form.chain_pem} onChange={(v) => setForm({ ...form, chain_pem: v })} placeholder="-----BEGIN CERTIFICATE-----" />
          <PemField label="Private key" hint="Unencrypted PEM. Stored encrypted; never shown again." value={form.key_pem} onChange={(v) => setForm({ ...form, key_pem: v })} placeholder="-----BEGIN PRIVATE KEY-----" />
          <div className="flex justify-end">
            <Button variant="primary" disabled={upload.isPending || !form.chain_pem || !form.key_pem}>
              {upload.isPending ? "Checking…" : "Upload"}
            </Button>
          </div>
        </form>
      </Modal>
      <Confirm
        open={!!dropping}
        onOpenChange={(o) => !o && setDropping(undefined)}
        title={`Delete “${dropping?.name}”?`}
        confirmLabel="Delete"
        busy={drop.isPending}
        onConfirm={() => dropping && drop.mutate(dropping.id)}
        body="The certificate and its key are removed from Forge. Edges keep serving their current copy until an endpoint changes."
      />
    </div>
  );
}

function Stat({ icon, label, value, hint, tone }: { icon: ReactNode; label: string; value: ReactNode; hint: string; tone?: "ok" | "warn" }) {
  return (
    <div className="flex items-center gap-3 rounded-lg border border-line bg-panel px-3 py-2">
      <span className={`grid size-8 shrink-0 place-items-center rounded-md ${tone === "warn" ? "bg-warn-soft text-warn" : tone === "ok" ? "bg-ok-soft text-ok" : "bg-accent-soft text-accent"}`}>{icon}</span>
      <div className="min-w-0">
        <div className="text-[11px] text-faint">{label}</div>
        <div className="text-sm font-semibold tabular-nums">{value}</div>
        <div className="truncate text-[10.5px] text-faint">{hint}</div>
      </div>
    </div>
  );
}
