import { Link } from "@tanstack/react-router";
import { ArrowRight, Globe, Lock, Waypoints } from "lucide-react";

import type { Service } from "../../api/types";
import { Badge, Button, Skeleton } from "../ui";
import { fanOut, sitesForApp, useSites } from "./sites";
import { balanceLabel, type Site } from "./types";

/** The Ingress page that owns `site`: the cluster's when it is linked, else the workspace one. */
export function IngressLink({ site, children, className = "" }: { site: Site; children: React.ReactNode; className?: string }) {
  const cid = site.config.cluster_id;
  return cid ? (
    <Link to="/ingress" search={{ cluster: cid }} className={className} title="Open the cluster's Ingress page">
      {children}
    </Link>
  ) : (
    <Link to="/ingress" className={className} title="Open Ingress">
      {children}
    </Link>
  );
}

/**
 * **Routes**: the endpoints that send traffic to this service, one line each:
 * domain → Ingress page, balancing, TLS, edges, and the replicas/standbys it
 * fans out to. Thin border, no panel-in-panel.
 */
export function ServiceRoutes({ app, onExpose, className = "" }: { app: Service; onExpose?: () => void; className?: string }) {
  const q = useSites();
  const routes = sitesForApp(q.data, app.id);
  const { replicas, standbys } = fanOut(app.instances);
  const fan = `${replicas} replica${replicas === 1 ? "" : "s"}${standbys ? ` + ${standbys} standby` : ""}`;
  return (
    <section className={`rounded-md border border-line ${className}`} aria-label="Routes">
      <header className="flex items-center gap-1.5 border-b border-line px-3 py-1.5 text-[11px] font-semibold tracking-wide text-faint uppercase">
        <Waypoints className="size-3.5" aria-hidden /> Routes
        {routes.length > 0 && <span className="font-normal normal-case tracking-normal">{routes.length} endpoint{routes.length === 1 ? "" : "s"} · Nginx fans out to {fan}</span>}
        {onExpose && routes.length > 0 && (
          <button type="button" className="ml-auto font-normal normal-case tracking-normal text-accent hover:underline" onClick={onExpose}>
            Expose through ingress
          </button>
        )}
      </header>
      {q.isLoading ? (
        <div className="p-3">
          <Skeleton rows={1} />
        </div>
      ) : q.isError ? (
        <p className="px-3 py-2 text-xs text-muted">Endpoints could not be loaded; the service still runs. Open Ingress to see its routes.</p>
      ) : routes.length === 0 ? (
        <div className="flex flex-wrap items-center justify-between gap-2 px-3 py-2.5">
          <span className="text-xs text-muted">No endpoint routes to this service yet.</span>
          {onExpose && (
            <Button className="h-7 px-2.5 text-xs" onClick={onExpose}>
              <Globe className="size-3.5" /> Expose through ingress
            </Button>
          )}
        </div>
      ) : (
        <ul className="divide-y divide-line">
          {routes.map((s) => {
            const spec = s.config.spec;
            const domains = spec.domains;
            const asStandby = !(s.config.targets ?? []).some((t) => t.kind === "app" && t.service_id === app.id);
            const port = [...(s.config.targets ?? []), ...(s.config.standby_targets ?? [])].find((t) => t.kind === "app" && t.service_id === app.id);
            const edges = s.config.edges.length;
            const tls = spec.tls === "off" ? "HTTP" : spec.tls === "acme" ? "Let's Encrypt" : "TLS";
            return (
              <li key={s.id} className="flex flex-wrap items-center gap-x-2 gap-y-1 px-3 py-1.5 text-xs">
                <IngressLink site={s} className="inline-flex min-w-0 max-w-full items-center gap-1 font-medium text-accent hover:underline">
                  {spec.tls === "off" ? <Globe className="size-3.5 shrink-0" aria-hidden /> : <Lock className="size-3.5 shrink-0 text-ok" aria-hidden />}
                  <span className="truncate" title={domains.join(", ") || s.name}>
                    {domains[0] ?? s.name}
                  </span>
                  {domains.length > 1 && <span className="shrink-0 text-faint">+{domains.length - 1}</span>}
                </IngressLink>
                <ArrowRight className="size-3 shrink-0 text-faint" aria-hidden />
                <span className="font-mono text-[11px] tabular-nums text-muted" title="Port Nginx forwards to on each node">
                  :{port && port.kind === "app" && port.port ? port.port : "—"}
                </span>
                <span className="text-muted" title={asStandby ? "This service is a standby target: used only when every active target is down" : `Nginx spreads requests over the replicas: ${balanceLabel(spec.balance)}`}>
                  {asStandby ? "standby target" : `→ ${fan} · ${balanceLabel(spec.balance)}`}
                </span>
                <span className="ml-auto flex shrink-0 items-center gap-1">
                  <Badge tone={spec.tls === "off" ? "muted" : "ok"}>{tls}</Badge>
                  <Badge tone={edges ? "muted" : "warn"}>{edges ? `${edges} edge${edges === 1 ? "" : "s"}` : "no edges"}</Badge>
                </span>
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}
