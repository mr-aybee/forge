import { Link } from "@tanstack/react-router";
import { CheckCircle2 } from "lucide-react";
import type { ReactNode } from "react";

import type { Certificate, EdgeCert, Placement, Site } from "../../api/ingress";
import { Badge, Select } from "../ui";
import type { usePlacementOptions } from "./endpointForm";

/** Certificate chip: green while valid (Forge renews Let's Encrypt at 5 days left). */
export function CertChip({
  site,
  certs,
}: {
  site: Site;
  certs: Certificate[];
}) {
  const tls = site.config.spec.tls;
  if (tls === "off") return <Badge>HTTP</Badge>;
  if (tls === "uploaded") {
    const c = certs.find((x) => x.id === site.config.spec.cert_id);
    if (!c) return <Badge tone="bad">certificate missing</Badge>;
    const tone = c.days_left < 0 ? "bad" : c.days_left <= 14 ? "warn" : "ok";
    return (
      <span
        title={
          c.days_left <= 14
            ? "Uploaded certificates cannot renew themselves: upload the new one from your CA and switch this endpoint to it."
            : `${c.name} · issued by ${c.issuer}`
        }
      >
        <Badge tone={tone}>
          {c.days_left < 0
            ? "certificate expired"
            : `uploaded · ${c.days_left}d`}
        </Badge>
      </span>
    );
  }
  const states = site.config.edges
    .map((e) => site.edge_status[e]?.cert)
    .filter((c): c is EdgeCert => !!c);
  const failing = states.find((c) => c.renew_error);
  const days = states
    .map((c) => c.days_left)
    .filter((d): d is number => typeof d === "number");
  const min = days.length ? Math.min(...days) : undefined;
  if (failing) {
    return (
      <span title={failing.renew_error ?? undefined}>
        <Badge tone="bad">
          renewal failing{min !== undefined ? ` · ${min}d` : ""}
        </Badge>
      </span>
    );
  }
  return (
    <span title="Let's Encrypt: Forge checks every 6 hours and renews automatically when 5 days or fewer are left.">
      <Badge tone={min === undefined ? "muted" : "ok"}>
        Let's Encrypt · {min !== undefined ? `${min}d` : "—"}
      </Badge>
    </span>
  );
}

/** A labelled checkbox on one line. */
export function Check({
  checked,
  onChange,
  children,
  hint,
  disabled,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  children: ReactNode;
  hint?: string;
  disabled?: boolean;
}) {
  return (
    <label
      className={`flex items-start gap-2 text-[13px] ${disabled ? "opacity-50" : ""}`}
      title={hint}
    >
      <input
        type="checkbox"
        className="mt-0.5"
        checked={checked}
        disabled={disabled}
        onChange={(e) => onChange(e.target.checked)}
      />
      <span className="min-w-0">
        {children}
        {hint && <span className="block text-[11px] text-faint">{hint}</span>}
      </span>
    </label>
  );
}

/** A section heading inside the editor. */
export function SectionHead({
  title,
  hint,
  actions,
}: {
  title: string;
  hint?: ReactNode;
  actions?: ReactNode;
}) {
  return (
    <div className="mb-2 flex items-end justify-between gap-3 border-b border-line pb-1.5">
      <div className="min-w-0">
        <div className="text-[13px] font-semibold">{title}</div>
        {hint && <div className="text-[11px] text-muted">{hint}</div>}
      </div>
      {actions && <div className="flex shrink-0 gap-1.5">{actions}</div>}
    </div>
  );
}

/** Where an endpoint is served: nodes with Nginx, or a cluster's edge group. */
export function PlacementPicker({
  value,
  onChange,
  opts,
}: {
  value: Placement;
  onChange: (p: Placement) => void;
  opts: ReturnType<typeof usePlacementOptions>;
}) {
  const nodesOn = value.kind === "nodes";
  const ids = value.kind === "nodes" ? value.server_ids : [];
  const cid = value.kind === "cluster" ? value.cluster_id : "";
  return (
    <div className="space-y-2" role="radiogroup" aria-label="Placement">
      <div className="rounded-md border border-line p-2">
        <label className="flex items-center gap-2 text-[13px] font-medium">
          <input
            type="radio"
            name="placement"
            checked={nodesOn}
            onChange={() =>
              onChange({
                kind: "nodes",
                server_ids: ids.length
                  ? ids
                  : opts.nginx.length === 1
                    ? [opts.nginx[0]!.id]
                    : [],
              })
            }
          />
          A node with Nginx
        </label>
        {nodesOn && (
          <div className="mt-1.5 pl-5">
            {opts.nginx.length ? (
              <div className="flex flex-wrap gap-1.5">
                {opts.nginx.map((n) => {
                  const on = ids.includes(n.id);
                  return (
                    <button
                      key={n.id}
                      type="button"
                      aria-pressed={on}
                      onClick={() =>
                        onChange({
                          kind: "nodes",
                          server_ids: on
                            ? ids.filter((x) => x !== n.id)
                            : [...ids, n.id],
                        })
                      }
                      className={`inline-flex max-w-[220px] items-center gap-1 rounded-full border px-2.5 py-0.5 text-xs ${on ? "border-accent bg-accent-soft text-accent" : "border-line text-muted hover:text-fg"}`}
                      title={n.hostname}
                    >
                      {on && <CheckCircle2 className="size-3 shrink-0" />}
                      <span className="truncate">
                        {n.display_name || n.hostname}
                      </span>
                    </button>
                  );
                })}
              </div>
            ) : (
              <p className="text-xs text-muted">
                {opts.loading
                  ? "Looking for nodes with Nginx…"
                  : "No node has Nginx installed. "}
                {!opts.loading && (
                  <Link
                    to="/app-store/$packageId"
                    params={{ packageId: "nginx" }}
                    className="text-accent hover:underline"
                  >
                    Install Nginx from the App Store →
                  </Link>
                )}
              </p>
            )}
            {ids.length > 1 && (
              <p className="mt-1 text-[11px] text-faint">
                Each node gets the same config; use DNS or Keepalived for
                failover between them.
              </p>
            )}
          </div>
        )}
      </div>
      <div className="rounded-md border border-line p-2">
        <label className="flex items-center gap-2 text-[13px] font-medium">
          <input
            type="radio"
            name="placement"
            checked={!nodesOn}
            onChange={() =>
              onChange({
                kind: "cluster",
                cluster_id: cid || opts.edgeGroups[0]?.cluster.id || "",
              })
            }
          />
          A cluster's edge group
        </label>
        {!nodesOn && (
          <div className="mt-1.5 pl-5">
            <Select
              aria-label="Cluster"
              value={cid}
              onChange={(e) =>
                onChange({ kind: "cluster", cluster_id: e.target.value })
              }
            >
              <option value="">Choose a cluster…</option>
              {opts.clusters.map((c) => {
                const g = opts.edgeGroups.find((x) => x.cluster.id === c.id);
                return (
                  <option key={c.id} value={c.id} disabled={!g}>
                    {c.name}{" "}
                    {g
                      ? `· ${g.edges} edge${g.edges === 1 ? "" : "s"}`
                      : "· no ingress edges (set up Ingress → Profile)"}
                  </option>
                );
              })}
            </Select>
            <p className="mt-1 text-[11px] text-faint">
              The cluster's ingress profile decides the edges; changing the
              profile moves the endpoint with it.
            </p>
          </div>
        )}
      </div>
    </div>
  );
}

/** "node-1" or "cluster prod · 2 edges". */
export function placementLabel(
  site: Site,
  host: (id: string) => string,
  clusterName: (id: string) => string,
) {
  const p = site.config.placement;
  if (p.kind === "cluster")
    return `cluster ${clusterName(p.cluster_id)} · ${site.config.edges.length} edge${site.config.edges.length === 1 ? "" : "s"}`;
  return p.server_ids.length ? p.server_ids.map(host).join(", ") : "—";
}
