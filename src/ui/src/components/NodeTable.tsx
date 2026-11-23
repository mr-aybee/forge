import { useQuery } from "@tanstack/react-query";
import { nodeName } from "../api/types";
import { Link } from "@tanstack/react-router";
import { ArrowUpDown, ChevronRight } from "lucide-react";
import { useMemo, useState } from "react";

import { get } from "../api/client";
import type { MetricSample } from "../api/hooks";
import { nodeFacts, nodeTags, type Cluster, type OnboardingNode, type Server } from "../api/types";
import { CancelJobButton } from "./CancelJob";
import { onboardingName, onboardingStatus } from "./OnboardingDrawer";
import { NodeTrafficCell, useTraffic } from "./Traffic";
import { Badge, Status, Table, Td, ago, bytes, time } from "./ui";

type SortKey = "hostname" | "status" | "last_seen_at" | "role";

/** Latest resource sample per node (refreshes with the list). */
export const useLatestMetrics = () =>
  useQuery({ queryKey: ["metrics-latest"], queryFn: () => get<Record<string, MetricSample>>("/api/v1/metrics/latest"), refetchInterval: 15_000 });

function Meter({ value, label }: { value?: number; label?: string }) {
  if (value === undefined || Number.isNaN(value)) return <span className="text-faint">—</span>;
  const tone = value > 90 ? "bg-bad" : value > 75 ? "bg-warn" : "bg-accent";
  return (
    <div className="flex min-w-[88px] items-center gap-2" title={label}>
      <div className="h-1.5 flex-1 overflow-hidden rounded-full bg-panel-2">
        <div className={`h-full rounded-full ${tone}`} style={{ width: `${Math.min(100, value)}%` }} />
      </div>
      <span className="w-9 text-right text-xs text-muted tabular-nums">{value.toFixed(0)}%</span>
    </div>
  );
}

/** OS label: `os_version` from facts, else the facts or enrolment OS; `—` when unknown. */
export function nodeOs(n: Server): string {
  const f = nodeFacts(n);
  return f.os_version ?? f.os ?? (n.os?.trim() || "—");
}

/**
 * CPU, RAM and disk totals: facts first, the latest telemetry sample as
 * fallback. Each part is `undefined` when unknown.
 */
export function nodeSpecs(n: Server, m?: Partial<MetricSample> | null) {
  const f = nodeFacts(n);
  const pos = (v: unknown) => (typeof v === "number" && Number.isFinite(v) && v > 0 ? v : undefined);
  return {
    cpus: f.cpus ?? pos(m?.cpus),
    mem: f.mem_total_bytes ?? pos(m?.mem_total),
    disk: f.disk_total_bytes ?? pos(m?.disk_total),
  };
}

/** "2 vCPU · 1.9 GiB RAM · 6.4 GiB disk"; only the known parts; `—` when none. */
export function specsText(s: ReturnType<typeof nodeSpecs>): string {
  const parts = [s.cpus && `${s.cpus} vCPU`, s.mem && `${bytes(s.mem)} RAM`, s.disk && `${bytes(s.disk)} disk`].filter(Boolean);
  return parts.length ? parts.join(" · ") : "—";
}

/** Specs stacked in a narrow cell: "2 vCPU · 770 MiB" over "7.0 GiB disk". */
function SpecsCell({ s }: { s: ReturnType<typeof nodeSpecs> }) {
  const top = [s.cpus && `${s.cpus} vCPU`, s.mem && bytes(s.mem)].filter(Boolean).join(" · ");
  const disk = s.disk ? `${bytes(s.disk)} disk` : "";
  if (!top && !disk) return <>—</>;
  return (
    <span className="flex flex-col" title={specsText(s)}>
      {top && <span>{top}</span>}
      {disk && <span className="text-[10px] opacity-80">{disk}</span>}
    </span>
  );
}

/** One tag: `key:value` as muted key plus value; a button when `onClick` is set. */
export function TagChip({ tag, onClick }: { tag: string; onClick?: (t: string) => void }) {
  const i = tag.indexOf(":");
  const body =
    i > 0 ? (
      <>
        <span className="text-faint">{tag.slice(0, i)}:</span>
        <span className="truncate">{tag.slice(i + 1)}</span>
      </>
    ) : (
      <span className="truncate">{tag}</span>
    );
  const cls = "inline-flex max-w-[140px] min-w-0 items-center rounded border border-line bg-panel-2 px-1.5 py-px font-mono text-[11px] text-fg";
  return onClick ? (
    <button type="button" className={`${cls} hover:border-line-strong focus-visible:outline-accent`} title={`Filter by ${tag}`} onClick={() => onClick(tag)}>
      {body}
    </button>
  ) : (
    <span className={cls} title={tag}>
      {body}
    </span>
  );
}

/** Up to `max` chips plus "+N" (the rest in its tooltip); `—` when there are none. */
export function TagChips({ tags, max = 3, onTag }: { tags: string[]; max?: number; onTag?: (t: string) => void }) {
  if (!tags.length) return <span className="text-faint">—</span>;
  const rest = tags.slice(max);
  return (
    <span className="flex min-w-0 flex-wrap items-center gap-1">
      {tags.slice(0, max).map((t) => (
        <TagChip key={t} tag={t} onClick={onTag} />
      ))}
      {rest.length > 0 && (
        <span className="font-mono text-[11px] text-muted tabular-nums" title={rest.join(", ")}>
          +{rest.length}
        </span>
      )}
    </span>
  );
}

/** Sortable node table shared by the Nodes page and cluster detail. */
export function NodeTable({
  nodes,
  clusters,
  actions,
  rowProps,
  showCluster = true,
  onboarding = [],
  onOpenOnboarding,
  onTag,
}: {
  nodes: Server[];
  clusters: Cluster[];
  actions?: (n: Server) => React.ReactNode;
  /** Extra props per node row, e.g. `useRowMenu().rowProps` for right-click. */
  rowProps?: (n: Server) => React.HTMLAttributes<HTMLTableRowElement>;
  showCluster?: boolean;
  /** Nodes being onboarded, shown first as "installing" / "failed" rows. */
  onboarding?: OnboardingNode[];
  onOpenOnboarding?: (o: OnboardingNode) => void;
  /** Clicking a tag chip calls this (the list adds it to the tag filter). */
  onTag?: (tag: string) => void;
}) {
  const [sort, setSort] = useState<{ key: SortKey; asc: boolean }>({ key: "role", asc: true });
  const metrics = useLatestMetrics().data ?? {};
  const rows = useMemo(() => {
    const sorted = [...nodes].sort((a, b) => {
      if (sort.key === "role") {
        // Control plane first, then by name.
        const r = Number(a.role !== "control") - Number(b.role !== "control");
        return r || nodeName(a).localeCompare(nodeName(b));
      }
      const av = a[sort.key] ?? "";
      const bv = b[sort.key] ?? "";
      return av < bv ? -1 : av > bv ? 1 : 0;
    });
    return sort.asc ? sorted : sorted.reverse();
  }, [nodes, sort]);

  const head = (label: string, key: SortKey) => (
    <button className="inline-flex items-center gap-1 uppercase" onClick={() => setSort((s) => ({ key, asc: s.key === key ? !s.asc : true }))}>
      {label} <ArrowUpDown className={`size-3 ${sort.key === key ? "text-accent" : ""}`} />
    </button>
  );

  const traffic = useTraffic().data;
  const columns = [head("Node", "hostname"), head("Role", "role"), head("Status", "status"), "CPU", "Memory", "Traffic", "OS", "Specs", "Tags"];
  if (showCluster) columns.push("Cluster");
  columns.push(head("Last seen", "last_seen_at"), "");

  return (
    <Table head={columns}>
      {onboarding.map((o) => (
        <OnboardingRow key={o.job_id ?? o.token_id} o={o} clusters={clusters} showCluster={showCluster} onOpen={() => onOpenOnboarding?.(o)} />
      ))}
      {rows.map((n) => {
        const m = n.status === "online" ? metrics[n.id] : undefined;
        const f = nodeFacts(n);
        const os = nodeOs(n);
        const arch = f.arch ?? (n.arch?.trim() || undefined);
        const specs = nodeSpecs(n, metrics[n.id]);
        return (
          <tr key={n.id} className="hover:bg-panel-2/60" {...rowProps?.(n)}>
            <Td>
              <Link to="/nodes/$nodeId" params={{ nodeId: n.id }} className="font-medium hover:text-accent">
                {nodeName(n)}
              </Link>
              <div className="font-mono text-[11px] text-faint">
                {n.display_name ? `${n.hostname} · ` : ""}agent {n.agent_version}
              </div>
            </Td>
            <Td>
              <Badge tone={n.role === "control" ? "accent" : "muted"}>{n.role === "control" ? "Control plane" : "Worker"}</Badge>
            </Td>
            <Td>
              <Status value={n.status} />
            </Td>
            <Td>
              <Meter value={m?.cpu} label={m ? `${m.cpus} cores, load ${m.load1.toFixed(2)}` : undefined} />
            </Td>
            <Td>
              <Meter value={m && m.mem_total ? (m.mem_used / m.mem_total) * 100 : undefined} />
            </Td>
            <Td>
              <NodeTrafficCell flow={traffic?.nodes?.[n.id]} />
            </Td>
            <Td className="max-w-[200px]">
              <div className="truncate text-[13px]" title={f.kernel ? `${os} · kernel ${f.kernel}` : os}>
                {os}
              </div>
              {arch && <div className="truncate font-mono text-[11px] text-faint">{arch}</div>}
            </Td>
            <Td className="font-mono text-[11px] leading-tight whitespace-nowrap text-muted tabular-nums">
              <SpecsCell s={specs} />
            </Td>
            <Td className="max-w-[260px]">
              <TagChips tags={nodeTags(n)} onTag={onTag} />
            </Td>
            {showCluster && (
              <Td>
                {n.cluster_id ? (
                  <Link to="/clusters/$clusterId" params={{ clusterId: n.cluster_id }} className="hover:text-accent">
                    <Badge tone="accent">{clusters.find((c) => c.id === n.cluster_id)?.name ?? "—"}</Badge>
                  </Link>
                ) : (
                  <span className="text-faint">None</span>
                )}
              </Td>
            )}
            <Td className="whitespace-nowrap text-muted">{ago(n.last_seen_at)}</Td>
            <Td className="text-right whitespace-nowrap">{actions?.(n)}</Td>
          </tr>
        );
      })}
    </Table>
  );
}

/** A node that is still being onboarded (or failed to): name, method, status, error, cluster. */
function OnboardingRow({ o, clusters, showCluster, onOpen }: { o: OnboardingNode; clusters: Cluster[]; showCluster: boolean; onOpen: () => void }) {
  const name = onboardingName(o);
  const failed = o.status === "failed";
  const cluster = o.cluster_id ? clusters.find((c) => c.id === o.cluster_id) : undefined;
  return (
    <tr className={`cursor-pointer hover:bg-panel-2/60 ${failed ? "bg-bad-soft/40" : ""}`} onClick={onOpen}>
      <Td className="max-w-[240px]">
        <button
          type="button"
          onClick={(e) => {
            e.stopPropagation();
            onOpen();
          }}
          className="block max-w-full truncate text-left font-medium hover:text-accent"
          title={name}
        >
          {name}
        </button>
        <div className="truncate font-mono text-[11px] text-faint" title={o.host ?? undefined}>
          {o.host && o.host !== name ? `${o.host} · ` : ""}
          {o.kind === "manual" ? "manual installer" : "assisted (SSH)"}
        </div>
      </Td>
      <Td>
        <Badge>Joining</Badge>
      </Td>
      <Td>
        <Status value={onboardingStatus(o)} />
      </Td>
      <Td colSpan={6} className="max-w-[360px]">
        {failed ? (
          <span className="block truncate font-mono text-[11px] text-bad" title={o.error ?? undefined}>
            {o.error ?? "Install failed: open for the console output."}
          </span>
        ) : (
          <span className="text-xs text-faint">{o.active ? "Installing the agent. Open to follow the console." : "—"}</span>
        )}
      </Td>
      {showCluster && (
        <Td>
          {o.cluster_id ? (
            <Link to="/clusters/$clusterId" params={{ clusterId: o.cluster_id }} className="hover:text-accent" onClick={(e) => e.stopPropagation()}>
              <Badge tone="accent">{cluster?.name ?? "—"}</Badge>
            </Link>
          ) : (
            <span className="text-faint">None</span>
          )}
        </Td>
      )}
      <Td className="whitespace-nowrap text-muted">
        <span title={time(o.updated_at)}>{o.updated_at ? ago(o.updated_at) : "—"}</span>
      </Td>
      <Td className="text-right whitespace-nowrap" onClick={(e) => e.stopPropagation()}>
        <span className="inline-flex items-center gap-1">
          {o.kind === "assisted" && o.job_id && o.active && <CancelJobButton job={{ id: o.job_id, status: "running" }} title={`Onboarding ${name}`} compact />}
          <button type="button" aria-label={`Open onboarding of ${name}`} onClick={onOpen} className="inline-grid size-8 place-items-center rounded-md text-muted hover:bg-panel-2 hover:text-fg">
            <ChevronRight className="size-4" />
          </button>
        </span>
      </Td>
    </tr>
  );
}
