import { Globe, KeyRound, Lock } from "lucide-react";
import type { ReactNode } from "react";

import { analyticsUnavailable } from "../../api/analytics";
import { useSlots } from "../../api/deployments";
import { useJob, useUsers } from "../../api/hooks";
import { useLogIngestKeys } from "../../api/logs";
import type { InstanceHealth, Server, Service } from "../../api/types";
import { IngressLink } from "../ingress/ServiceRoutes";
import { sitesForApp, useSites } from "../ingress/sites";
import { liveState } from "../InstanceState";
import { ago, time } from "../ui";

type Tone = "ok" | "warn" | "bad" | "muted";
const TEXT: Record<Tone, string> = { ok: "text-ok", warn: "text-warn", bad: "text-bad", muted: "text-muted" };
const DOT: Record<Tone, string> = { ok: "bg-ok", warn: "bg-warn", bad: "bg-bad", muted: "bg-faint" };

function Cell({ label, children, title }: { label: string; children: ReactNode; title?: string }) {
  return (
    <div className="min-w-0 bg-panel px-3 py-1.5" title={title}>
      <div className="text-[10px] font-semibold tracking-wide text-faint uppercase">{label}</div>
      <div className="flex min-w-0 items-center gap-1.5 text-[13px]">{children}</div>
    </div>
  );
}

const Dot = ({ tone }: { tone: Tone }) => <span aria-hidden className={`size-1.5 shrink-0 rounded-full ${DOT[tone]}`} />;

/**
 * The service at a glance: replicas running and healthy, the last deploy
 * (result, when, who, release), the endpoint and the log ingest keys. Every
 * cell shows `—` when its data is missing or the caller may not read it.
 */
export function ServiceStatusStrip({ a, nodes, health, onLogKeys }: { a: Service; nodes: Server[]; health?: InstanceHealth[]; onLogKeys: () => void }) {
  const instances = Array.isArray(a.instances) ? a.instances : [];
  const byId = new Map(nodes.map((n) => [n.id, n]));

  // Replicas: placements that serve traffic, how many run, how many are healthy.
  const active = instances.filter((i) => i.role !== "standby");
  const running = active.filter((i) => liveState(i, byId.get(i.server_id)).kind === "running").length;
  const healthy = health ? active.filter((i) => health.find((h) => h.server_id === i.server_id)?.status === "healthy").length : undefined;
  const standbys = instances.length - active.length;
  const runTone: Tone = active.length === 0 ? "muted" : running === active.length && (healthy === undefined || healthy === active.length) ? "ok" : running === 0 ? "bad" : "warn";

  // Last deploy: the newest job across the nodes.
  const deployed = instances.filter((i) => i.status !== "pending");
  const last = [...deployed].sort((x, y) => (y.updated_at ?? 0) - (x.updated_at ?? 0))[0];
  const failed = deployed.filter((i) => i.status === "failed").length;
  const busy = deployed.some((i) => i.status.endsWith("ing"));
  const deployTone: Tone = !last ? "muted" : failed ? "bad" : busy ? "warn" : "ok";
  const deployLabel = !last ? "never deployed" : failed ? `failed on ${failed} of ${deployed.length}` : busy ? "in progress" : "succeeded";
  const job = useJob(last?.last_job_id ?? undefined).data;
  const users = useUsers().data;
  const who = job?.principal?.type === "user" ? (Array.isArray(users) ? users.find((u) => u.id === job.principal.id) : undefined) : undefined;
  const by = who ? who.display_name || who.username || who.email || "" : job?.principal?.type === "user" ? "" : job?.principal?.type ?? "";
  const release = useSlots(a.id).data?.current_release ?? 0;

  // Endpoint: the first route, with how many more.
  const sites = useSites();
  const routes = sitesForApp(sites.data, a.id);
  const first = routes[0];
  const domain = first ? (first.config.spec.domains[0] ?? first.name) : undefined;

  // Log ingest keys; analytics off counts as unavailable, not zero.
  const keys = useLogIngestKeys(a.id);
  const keysOff = analyticsUnavailable(keys.error);
  const keyCount = keys.data?.length;

  return (
    <div className="mb-3 grid grid-cols-2 gap-px overflow-hidden rounded-lg border border-line bg-line lg:grid-cols-4">
      <Cell label="Replicas" title={`${running} of ${active.length} replicas running${healthy !== undefined ? `, ${healthy} healthy` : ""}${standbys ? `; ${standbys} standby` : ""}`}>
        <Dot tone={runTone} />
        <span className={`font-mono text-xs tabular-nums ${TEXT[runTone]}`}>
          {active.length ? `${running}/${active.length}` : "—"}
        </span>
        <span className="min-w-0 truncate text-muted">
          {active.length ? "running" : "no replicas"}
          {healthy !== undefined && active.length > 0 && ` · ${healthy === active.length ? "healthy" : `${healthy} healthy`}`}
          {standbys > 0 && ` · +${standbys} standby`}
        </span>
      </Cell>
      <Cell label="Last deploy" title={last ? `${deployLabel} · ${time(last.updated_at)}${by ? ` by ${by}` : ""}${release ? ` · release #${release}` : ""}` : "Not deployed to any node yet"}>
        <Dot tone={deployTone} />
        <span className={`shrink-0 ${TEXT[deployTone]}`}>{deployLabel}</span>
        {last && (
          <span className="min-w-0 truncate text-xs text-muted">
            {ago(last.updated_at)}
            {by && ` · ${by}`}
            {release > 0 && <span className="font-mono text-[11px]"> · #{release}</span>}
          </span>
        )}
      </Cell>
      <Cell label="Endpoint">
        {first && domain ? (
          <>
            <IngressLink site={first} className="inline-flex min-w-0 items-center gap-1 text-accent hover:underline">
              {first.config.spec.tls === "off" ? <Globe className="size-3.5 shrink-0" aria-hidden /> : <Lock className="size-3.5 shrink-0 text-ok" aria-hidden />}
              <span className="truncate" title={domain}>
                {domain}
              </span>
            </IngressLink>
            {routes.length > 1 && <span className="shrink-0 text-xs text-faint">+{routes.length - 1}</span>}
          </>
        ) : (
          <span className="text-muted">{sites.isLoading ? "…" : sites.isError ? "—" : "none"}</span>
        )}
      </Cell>
      <Cell label="Log ingest keys">
        <button type="button" onClick={onLogKeys} className="inline-flex min-w-0 items-center gap-1.5 rounded text-left hover:text-accent focus-visible:outline-2 focus-visible:outline-accent" title="Show the log ingest keys">
          <KeyRound className="size-3.5 shrink-0 text-faint" aria-hidden />
          <span className="truncate">{keysOff ? "analytics off" : keyCount === undefined ? "—" : `${keyCount} key${keyCount === 1 ? "" : "s"}`}</span>
        </button>
      </Cell>
    </div>
  );
}
