// Per-node health: a status badge whose popover lists that node's checks,
// and the amber banner for health warnings. Shared by the service page,
// the Health Checks tab (a one-line strip) and the node's Services tab.
import * as Popover from "@radix-ui/react-popover";
import { Link } from "@tanstack/react-router";
import { AlertTriangle } from "lucide-react";

import type { HealthWarning, InstanceCheck, InstanceHealth, InstanceHealthStatus } from "../../api/types";
import { Badge, ago, time } from "../ui";

const STATUS: Record<InstanceHealthStatus, { label: string; tone: "ok" | "warn" | "bad" | "muted" }> = {
  healthy: { label: "Healthy", tone: "ok" },
  degraded: { label: "Degraded", tone: "warn" },
  unhealthy: { label: "Unhealthy", tone: "bad" },
  unknown: { label: "Unknown", tone: "muted" },
};

const REASON: Record<string, string> = {
  app_failure: "App answered with a failure",
  timeout: "Timed out",
  connect_failed: "Connection failed",
  ambiguous_port: "Listening port is ambiguous",
  node_offline: "Node offline",
  stale: "Result is stale",
  unsupported_executor: "Can't run on this node",
  tls: "TLS handshake failed",
  auth: "Authentication refused",
  invalid_config: "Not allowed or not valid",
  permission_denied: "Vault value refused",
  location_unavailable: "Forge did not answer",
};

/** Reasons whose detail is the whole explanation ("This node can't confine command checks…"). */
const DETAIL_SAYS_IT = new Set(["unsupported_executor", "permission_denied", "location_unavailable"]);

const STATE_TONE: Record<string, "ok" | "warn" | "bad" | "muted"> = {
  passing: "ok",
  succeeded: "ok",
  failing: "bad",
  exhausted: "bad",
  pending: "warn",
  waiting: "warn",
};

const reasonText = (r?: string | null) => (r ? (REASON[r] ?? r.replace(/_/g, " ")) : undefined);
const passing = (c: InstanceCheck) => c.state === "passing" || c.state === "succeeded";
const sentence = (s: string) => s.charAt(0).toUpperCase() + s.slice(1);

/** Why a check isn't passing: the node's own sentence when it refused to run it, else "reason: detail". */
function whyOf(c: InstanceCheck): string {
  if (passing(c)) return "";
  if (c.reason && c.detail && DETAIL_SAYS_IT.has(c.reason)) return sentence(c.detail);
  return [reasonText(c.reason), c.detail].filter(Boolean).join(": ");
}

/** The badge label: "Node offline" when every check is unknown because the node is offline. */
function labelOf(h: InstanceHealth) {
  const checks = h.checks ?? [];
  if (h.status === "unknown" && checks.length > 0 && checks.every((c) => c.reason === "node_offline")) return "Node offline";
  return STATUS[h.status]?.label ?? "Unknown";
}

/** Health badge for one node; click or Enter opens its checks. `undefined` health renders "—". */
export function InstanceHealthBadge({ health, nodeName }: { health?: InstanceHealth | null; nodeName?: string }) {
  if (!health) return <span className="text-xs text-faint" title="No health data for this node">—</span>;
  const s = STATUS[health.status] ?? STATUS.unknown;
  const label = labelOf(health);
  return (
    <Popover.Root>
      <Popover.Trigger
        className="inline-flex rounded focus-visible:outline-2 focus-visible:outline-accent"
        aria-label={`Health${nodeName ? ` on ${nodeName}` : ""}: ${label}. Show checks`}
      >
        <Badge tone={s.tone}>
          <span aria-hidden className="mr-1.5 size-1.5 rounded-full bg-current" />
          {label}
        </Badge>
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content align="start" sideOffset={4} collisionPadding={8} className="animate-in z-50 w-[min(26rem,calc(100vw-1rem))] rounded-lg border border-line bg-panel p-2 text-xs shadow-[var(--shadow-pop)]">
          <div className="mb-1.5 flex items-center gap-2 px-1">
            <span className="font-medium">{nodeName ?? health.server_id.slice(0, 8)}</span>
            <Badge tone={s.tone}>{label}</Badge>
          </div>
          <HealthChecksList checks={health.checks} />
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}

/** One line per check: purpose, name, state, and why when not passing. */
export function HealthChecksList({ checks }: { checks?: InstanceCheck[] | null }) {
  const list = Array.isArray(checks) ? checks : [];
  if (list.length === 0) return <p className="px-1 text-faint">No check results reported yet.</p>;
  return (
    <ul className="divide-y divide-line">
      {list.map((c, i) => {
        const why = whyOf(c);
        return (
          <li key={`${c.purpose}-${c.name}-${i}`} className="min-w-0 px-1 py-1">
            <div className="flex min-w-0 items-center gap-2">
              <span className="w-16 shrink-0 text-[11px] text-faint">{c.purpose}</span>
              <span className="min-w-0 flex-1 truncate font-mono text-[11px]" title={c.name}>
                {c.default ? "Default" : c.name || "—"}
              </span>
              {c.stale && <span className="text-[11px] text-warn">stale</span>}
              <Badge tone={STATE_TONE[c.state] ?? "muted"}>{c.state || "unknown"}</Badge>
            </div>
            {why && <div className="mt-0.5 pl-[4.5rem] break-words text-[11px] text-muted select-text">{why}</div>}
            {c.received_at ? (
              <div className="pl-[4.5rem] font-mono text-[10.5px] text-faint tabular-nums" title={time(c.received_at)}>
                {ago(c.received_at)}
              </div>
            ) : null}
          </li>
        );
      })}
    </ul>
  );
}

/**
 * How one configured check is doing across the nodes: "2/2 passing",
 * "can't run" (every node refuses it), "no results yet". The popover lists
 * each node with its reason.
 * Renders nothing when the app isn't placed anywhere.
 */
export function CheckResultPill({ instances, purpose, name, hostOf, dot }: { instances?: InstanceHealth[] | null; purpose: string; name: string; hostOf: (id: string) => string; /** Only a dot, for dense rows; the label is the tooltip. */ dot?: boolean }) {
  const rows = (Array.isArray(instances) ? instances : []).flatMap((h) =>
    (Array.isArray(h.checks) ? h.checks : []).filter((c) => !c.default && c.purpose === purpose && c.name === name).map((c) => ({ node: hostOf(h.server_id), c })),
  );
  if (rows.length === 0) return null;
  const ok = rows.filter((r) => passing(r.c)).length;
  const failed = rows.some((r) => r.c.state === "failing" || r.c.state === "exhausted");
  const refused = rows.every((r) => r.c.state === "unknown" && r.c.reason === "unsupported_executor");
  const disabled = rows.every((r) => r.c.state === "disabled");
  const waiting = rows.every((r) => !r.c.received_at);
  const tone = disabled || waiting ? "muted" : failed ? "bad" : ok === rows.length ? "ok" : "warn";
  const label = disabled ? "disabled" : waiting ? "no results yet" : refused ? "can't run" : `${ok}/${rows.length} passing`;
  return (
    <Popover.Root>
      <Popover.Trigger className={`inline-flex shrink-0 rounded focus-visible:outline-2 focus-visible:outline-accent ${dot ? "size-5 place-items-center justify-center hover:bg-panel-2" : ""}`} aria-label={`Results of ${name}: ${label}. Show nodes`} title={dot ? label : undefined}>
        {dot ? (
          <span aria-hidden className={`size-2 rounded-full ${DOT[tone]}`} />
        ) : (
          <Badge tone={tone}>
            <span aria-hidden className="mr-1.5 size-1.5 rounded-full bg-current" />
            <span className="font-mono tabular-nums">{label}</span>
          </Badge>
        )}
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content align="end" sideOffset={4} collisionPadding={8} className="animate-in z-50 w-[min(24rem,calc(100vw-1rem))] rounded-lg border border-line bg-panel p-2 text-xs shadow-[var(--shadow-pop)]">
          <ul className="divide-y divide-line">
            {rows.map(({ node, c }, i) => {
              const why = whyOf(c);
              return (
                <li key={`${node}-${i}`} className="min-w-0 px-1 py-1">
                  <div className="flex min-w-0 items-center gap-2">
                    <span className="min-w-0 flex-1 truncate font-medium" title={node}>{node}</span>
                    {c.stale && <span className="text-[11px] text-warn">stale</span>}
                    <span className="font-mono text-[10.5px] text-faint tabular-nums" title={c.received_at ? time(c.received_at) : undefined}>{c.received_at ? ago(c.received_at) : ""}</span>
                    <Badge tone={STATE_TONE[c.state] ?? "muted"}>{c.state || "unknown"}</Badge>
                  </div>
                  {why && <div className="mt-0.5 break-words text-[11px] text-muted select-text">{why}</div>}
                </li>
              );
            })}
          </ul>
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}

// ------------------------------------------------------------------ summary

const notPassing = (c: InstanceCheck) => !passing(c) && c.state !== "disabled";
const lastSeen = (checks: InstanceCheck[]) => checks.reduce<number | undefined>((m, c) => (typeof c.received_at === "number" && (m === undefined || c.received_at > m) ? c.received_at : m), undefined);

/** The worst status across nodes: unhealthy > degraded > unknown > healthy. Empty: unknown. */
export function overallStatus(instances: InstanceHealth[]): InstanceHealthStatus {
  const list = Array.isArray(instances) ? instances : [];
  if (list.length === 0) return "unknown";
  if (list.some((i) => i.status === "unhealthy")) return "unhealthy";
  if (list.some((i) => i.status === "degraded")) return "degraded";
  if (list.some((i) => i.status !== "healthy")) return "unknown";
  return "healthy";
}

const DOT = { ok: "bg-ok", warn: "bg-warn", bad: "bg-bad", muted: "bg-faint" } as const;
const TEXT = { ok: "text-ok", warn: "text-warn", bad: "text-bad", muted: "text-muted" } as const;

/**
 * Status, compact: one line with the overall dot and label, "N of M nodes
 * healthy · last result …", then a chip per node. Each chip opens that
 * node's checks (failing ones first). The full status lives on Overview.
 * Never crashes on missing fields; empty data keeps the line.
 */
export function HealthSummary({ instances, hostOf }: { instances?: InstanceHealth[] | null; hostOf: (id: string) => string }) {
  const list = Array.isArray(instances) ? instances : [];
  const overall = STATUS[overallStatus(list)] ?? STATUS.unknown;
  const healthy = list.filter((i) => i.status === "healthy").length;
  const last = lastSeen(list.flatMap((i) => (Array.isArray(i.checks) ? i.checks : [])));
  return (
    <section aria-label="Health status" className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1.5 rounded-md border border-line px-3 py-1.5 text-xs">
      <span className="inline-flex min-w-0 items-center gap-1.5">
        <span aria-hidden className={`size-2 shrink-0 rounded-full ${DOT[overall.tone]}`} />
        <span className={`font-semibold ${TEXT[overall.tone]}`}>{overall.label}</span>
      </span>
      <span className="min-w-0 text-muted">
        {list.length ? (
          <>
            <span className="font-mono text-[11px] tabular-nums text-fg">{healthy}</span> of <span className="font-mono text-[11px] tabular-nums text-fg">{list.length}</span> node{list.length === 1 ? "" : "s"} healthy
          </>
        ) : (
          "Not placed on any node"
        )}
        <span className="text-faint"> · </span>
        <span title={last ? time(last) : "No results reported yet"}>{last ? `last result ${ago(last)}` : "no results yet"}</span>
      </span>
      {list.length > 0 && (
        <ul className="flex min-w-0 flex-wrap items-center gap-1.5 sm:ml-auto" aria-label="Nodes">
          {list.map((h) => (
            <li key={h.server_id} className="min-w-0">
              <NodeChip health={h} name={hostOf(h.server_id)} />
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

/** A node as a chip with its status dot; opens its checks, failing ones first. */
function NodeChip({ health: h, name }: { health: InstanceHealth; name: string }) {
  const checks = Array.isArray(h.checks) ? h.checks : [];
  const failing = checks.filter(notPassing);
  const s = STATUS[h.status] ?? STATUS.unknown;
  const label = labelOf(h);
  const last = lastSeen(checks);
  return (
    <Popover.Root>
      <Popover.Trigger
        className={`inline-flex max-w-[14rem] min-w-0 items-center gap-1.5 rounded-full border px-2 py-0.5 hover:bg-panel-2 focus-visible:outline-2 focus-visible:outline-accent ${failing.length ? "border-line-strong" : "border-line"}`}
        aria-label={`${name}: ${label}${failing.length ? `, ${failing.length} not passing` : ""}. Show checks`}
        title={`${name}: ${label}`}
      >
        <span aria-hidden className={`size-1.5 shrink-0 rounded-full ${DOT[s.tone]}`} />
        <span className="min-w-0 truncate">{name}</span>
        {failing.length > 0 && <span className={`font-mono text-[11px] tabular-nums ${TEXT[s.tone === "ok" ? "warn" : s.tone]}`}>{failing.length}</span>}
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content align="end" sideOffset={4} collisionPadding={8} className="animate-in z-50 w-[min(26rem,calc(100vw-1rem))] rounded-lg border border-line bg-panel p-2 text-xs shadow-[var(--shadow-pop)]">
          <div className="mb-1.5 flex min-w-0 items-center gap-2 px-1">
            <Link to="/nodes/$nodeId" params={{ nodeId: h.server_id }} className="min-w-0 truncate font-medium hover:text-accent" title={name}>
              {name}
            </Link>
            <Badge tone={s.tone}>{label}</Badge>
            <span className="ml-auto font-mono text-[10.5px] text-faint tabular-nums" title={last ? time(last) : undefined}>
              {last ? ago(last) : "—"}
            </span>
          </div>
          {failing.length > 0 && <div className="px-1 pb-0.5 text-[11px] font-medium text-muted">Not passing</div>}
          <HealthChecksList checks={failing.length ? failing : checks} />
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}

/** Amber banner listing health warnings; renders nothing without warnings. */
export function HealthWarnings({ warnings, hostOf }: { warnings?: HealthWarning[] | null; hostOf: (id: string) => string }) {
  const list = Array.isArray(warnings) ? warnings : [];
  if (list.length === 0) return null;
  return (
    <div role="status" className="mb-3 rounded-md border border-warn/40 bg-warn-soft px-3 py-2 text-xs text-warn">
      <div className="mb-0.5 flex items-center gap-1.5 font-medium">
        <AlertTriangle className="size-3.5 shrink-0" /> Health check {list.length === 1 ? "warning" : `warnings (${list.length})`}
      </div>
      <ul className="space-y-0.5 pl-5">
        {list.map((w, i) => (
          <li key={`${w.code}-${w.server_id ?? ""}-${i}`} className="list-disc break-words">
            {w.server_id && <span className="font-medium">{hostOf(w.server_id)}: </span>}
            {w.message}
          </li>
        ))}
      </ul>
    </div>
  );
}
