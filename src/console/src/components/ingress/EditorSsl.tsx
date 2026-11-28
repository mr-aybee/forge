import { Loader2, RefreshCw, ShieldOff, Upload } from "lucide-react";
import { useState } from "react";

import {
  type Site,
  type SiteSpec,
  covers,
  useCertificates,
  useRenewSite,
  useUploadCertificate,
} from "../../api/ingress";
import { useServers } from "../../api/hooks";
import { errorText, useToast } from "../../app/toast";
import {
  Badge,
  Button,
  ErrorNote,
  Field,
  Input,
  Mono,
  Select,
  tabTrigger,
  time,
} from "../ui";
import { PemField } from "./Certificates";
import type { EndpointForm } from "./endpointForm";
import { Check, SectionHead } from "./EndpointParts";

type Tab = "acme" | "upload" | "store";

/** SSL: the current certificate, Force HTTPS / HSTS, and where the certificate comes from. */
export function SslSection({
  f,
  setSpec,
  site,
  dirty,
}: {
  f: EndpointForm;
  setSpec: (p: Partial<SiteSpec>) => void;
  site: Site;
  dirty: boolean;
}) {
  const toast = useToast();
  const s = f.spec;
  const certs = useCertificates();
  const nodes = useServers().data ?? [];
  const renew = useRenewSite();
  const upload = useUploadCertificate();
  const [tab, setTab] = useState<Tab>(s.tls === "uploaded" ? "store" : "acme");
  const [pem, setPem] = useState({ name: "", chain_pem: "", key_pem: "" });
  const host = (id: string) =>
    nodes.find((n) => n.id === id)?.hostname ?? id.slice(0, 8);
  const wildcard = s.domains.some((d) => d.includes("*"));
  const current = certs.data?.find((c) => c.id === s.cert_id);

  return (
    <div className="space-y-5">
      <section>
        <SectionHead
          title="Current state"
          actions={
            s.tls !== "off" && (
              <Button
                type="button"
                variant="danger"
                onClick={() => setSpec({ tls: "off" })}
                title="Serve HTTP only after the next Save and Apply"
              >
                <ShieldOff className="size-4" /> Disable SSL
              </Button>
            )
          }
        />
        {s.tls === "off" && (
          <p className="text-xs text-muted">
            SSL is off: the endpoint answers on HTTP only. Choose a certificate
            source below.
          </p>
        )}
        {s.tls === "uploaded" &&
          (certs.error ? (
            <ErrorNote error={certs.error} />
          ) : current ? (
            <dl className="grid grid-cols-[110px_1fr] gap-y-1 text-xs">
              <dt className="text-muted">Certificate</dt>
              <dd className="min-w-0 truncate">{current.name}</dd>
              <dt className="text-muted">Issuer</dt>
              <dd className="min-w-0 truncate" title={current.issuer}>
                {current.issuer || "—"}
              </dd>
              <dt className="text-muted">Domains</dt>
              <dd className="min-w-0 font-mono text-[11px] break-all">
                {current.domains.join(", ") || "—"}
              </dd>
              <dt className="text-muted">Expires</dt>
              <dd>
                <span title={time(current.not_after)}>
                  {current.days_left < 0 ? (
                    <Badge tone="bad">expired</Badge>
                  ) : (
                    <Badge tone={current.days_left <= 14 ? "warn" : "ok"}>
                      {current.days_left} days left
                    </Badge>
                  )}
                </span>
              </dd>
            </dl>
          ) : (
            <p className="text-xs text-warn">
              {s.cert_id
                ? "The chosen certificate no longer exists. Pick or upload another."
                : "No certificate chosen yet."}
            </p>
          ))}
        {s.tls === "acme" && (
          <div className="space-y-1 text-xs">
            {site.config.edges.length === 0 && (
              <p className="text-muted">
                Not applied to any edge yet; Let's Encrypt issues the
                certificate on the first Apply.
              </p>
            )}
            {site.config.edges.map((e) => {
              const c = site.edge_status[e]?.cert;
              return (
                <div key={e} className="flex min-w-0 items-center gap-2">
                  <span className="w-32 truncate" title={host(e)}>
                    {host(e)}
                  </span>
                  {c?.renew_error ? (
                    <span title={c.renew_error}>
                      <Badge tone="bad">renewal failing</Badge>
                    </span>
                  ) : typeof c?.days_left === "number" ? (
                    <Badge tone={c.days_left <= 5 ? "warn" : "ok"}>
                      {c.days_left} days left
                    </Badge>
                  ) : (
                    <Badge>not issued yet</Badge>
                  )}
                  {c?.expiry && (
                    <span className="truncate text-faint">
                      expires {c.expiry}
                    </span>
                  )}
                </div>
              );
            })}
          </div>
        )}
        {s.tls !== "off" && (
          <div className="mt-3 grid gap-1.5 sm:grid-cols-2">
            <Check
              checked={s.force_https}
              onChange={(force_https) => setSpec({ force_https })}
              hint="HTTP requests get a 301 to HTTPS."
            >
              Force HTTPS
            </Check>
            <Check
              checked={s.hsts ?? true}
              onChange={(hsts) => setSpec({ hsts })}
              hint="Browsers remember to use HTTPS only (Strict-Transport-Security)."
            >
              HSTS
            </Check>
          </div>
        )}
      </section>

      <section>
        <div
          role="tablist"
          aria-label="Certificate source"
          className="mb-3 flex gap-1 border-b border-line"
        >
          {(
            [
              ["acme", "Let's Encrypt"],
              ["upload", "Upload"],
              ["store", "From certificate store"],
            ] as const
          ).map(([id, label]) => (
            <button
              key={id}
              type="button"
              role="tab"
              aria-selected={tab === id}
              data-state={tab === id ? "active" : "inactive"}
              className={tabTrigger}
              onClick={() => setTab(id)}
            >
              {label}
              {((id === "acme" && s.tls === "acme") ||
                (id === "store" && s.tls === "uploaded")) && (
                <Badge tone="ok">in use</Badge>
              )}
            </button>
          ))}
        </div>

        {tab === "acme" && (
          <div className="space-y-2">
            <p className="text-xs text-muted">
              Free certificates, issued and renewed on the edges (Forge renews 5
              days before expiry). Let's Encrypt reaches the edges on HTTP port
              80.
            </p>
            {wildcard && (
              <p className="text-xs text-warn">
                Wildcard domains can't be issued this way; use an uploaded
                certificate.
              </p>
            )}
            <Field
              label="Contact email"
              hint="Let's Encrypt sends expiry warnings here."
            >
              <Input
                type="email"
                value={s.acme_email}
                onChange={(e) => setSpec({ acme_email: e.target.value })}
                placeholder="ops@example.com"
              />
            </Field>
            <div className="flex flex-wrap gap-2">
              {s.tls !== "acme" && (
                <Button
                  type="button"
                  variant="primary"
                  disabled={wildcard}
                  title={
                    wildcard ? "Remove the wildcard domains first" : undefined
                  }
                  onClick={() => setSpec({ tls: "acme" })}
                >
                  Use Let's Encrypt
                </Button>
              )}
              {s.tls === "acme" && (
                <Button
                  type="button"
                  disabled={
                    renew.isPending || dirty || site.config.spec.tls !== "acme"
                  }
                  title={
                    dirty || site.config.spec.tls !== "acme"
                      ? "Save first: renewal uses the saved endpoint"
                      : "Ask Let's Encrypt for a new certificate on every edge now"
                  }
                  onClick={() =>
                    renew.mutate(site.id, {
                      onSuccess: (r) => {
                        const bad = Object.entries(r.certificates ?? {}).find(
                          ([, c]) => c?.renew_error,
                        );
                        if (bad)
                          toast(
                            `Renewal failed on ${host(bad[0])}: ${bad[1]?.renew_error}`,
                            "error",
                          );
                        else toast("Certificate renewed on every edge");
                      },
                      onError: (e) => toast(errorText(e), "error"),
                    })
                  }
                >
                  {renew.isPending ? (
                    <Loader2 className="size-4 animate-spin" />
                  ) : (
                    <RefreshCw className="size-4" />
                  )}{" "}
                  Renew now
                </Button>
              )}
            </div>
          </div>
        )}

        {tab === "upload" && (
          <form
            className="space-y-2"
            onSubmit={(e) => {
              e.preventDefault();
              upload.mutate(pem, {
                onSuccess: (c) => {
                  toast(
                    `Certificate ${c.name} uploaded and selected; Save to use it`,
                  );
                  setSpec({ tls: "uploaded", cert_id: c.id });
                  setPem({ name: "", chain_pem: "", key_pem: "" });
                  setTab("store");
                },
                onError: (err) => toast(errorText(err), "error"),
              });
            }}
          >
            <Field label="Name">
              <Input
                value={pem.name}
                onChange={(e) => setPem({ ...pem, name: e.target.value })}
                placeholder={s.domains[0] ?? "shop.example.com"}
                required
              />
            </Field>
            <PemField
              label="Certificate chain"
              hint="Your certificate first, then the intermediates."
              value={pem.chain_pem}
              onChange={(chain_pem) => setPem({ ...pem, chain_pem })}
              placeholder="-----BEGIN CERTIFICATE-----"
            />
            <PemField
              label="Private key"
              hint="Stored encrypted on the server; it never comes back to the browser."
              value={pem.key_pem}
              onChange={(key_pem) => setPem({ ...pem, key_pem })}
              placeholder="-----BEGIN PRIVATE KEY-----"
            />
            <Button
              type="submit"
              variant="primary"
              disabled={
                upload.isPending || !pem.name || !pem.chain_pem || !pem.key_pem
              }
            >
              {upload.isPending ? (
                <Loader2 className="size-4 animate-spin" />
              ) : (
                <Upload className="size-4" />
              )}{" "}
              Upload and use
            </Button>
          </form>
        )}

        {tab === "store" &&
          (certs.error ? (
            <ErrorNote error={certs.error} />
          ) : (
            <Field
              label="Certificate"
              hint={
                certs.data?.length
                  ? "Certificates that don't cover every domain, or have expired, can't be chosen."
                  : "No uploaded certificates yet; use the Upload tab."
              }
            >
              <Select
                value={s.tls === "uploaded" ? (s.cert_id ?? "") : ""}
                onChange={(e) =>
                  setSpec(
                    e.target.value
                      ? { tls: "uploaded", cert_id: e.target.value }
                      : { cert_id: undefined },
                  )
                }
              >
                <option value="">Choose…</option>
                {(certs.data ?? []).map((c) => {
                  const ok = s.domains.every((d) => covers(c.domains, d));
                  return (
                    <option
                      key={c.id}
                      value={c.id}
                      disabled={!ok || c.days_left < 0}
                    >
                      {c.name} · {c.domains.slice(0, 2).join(", ")}
                      {c.domains.length > 2 ? "…" : ""} ·{" "}
                      {c.days_left < 0 ? "expired" : `${c.days_left} days left`}
                      {ok ? "" : " · doesn't cover these domains"}
                    </option>
                  );
                })}
              </Select>
            </Field>
          ))}
      </section>
      <p className="text-[11px] text-faint">
        Changes here take effect after <Mono>Save</Mono> and <Mono>Apply</Mono>.
      </p>
    </div>
  );
}
