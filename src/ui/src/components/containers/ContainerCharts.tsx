import { Link } from "@tanstack/react-router";
import { CircleOff } from "lucide-react";
import { useState } from "react";

import { analyticsUnavailable } from "../../api/analytics";
import { useContainerMetrics, type ContainerSeries } from "../../api/containerTelemetry";
import { TimeSeries, fmtBytes, fmtPct, fmtRate, type Series } from "../charts/TimeSeries";
import { Empty, ErrorNote, Segmented, Skeleton } from "../ui";

const WINDOWS = [
  { id: "1", label: "1 h" },
  { id: "6", label: "6 h" },
  { id: "24", label: "24 h" },
  { id: "168", label: "7 d" },
] as const;
type WindowId = (typeof WINDOWS)[number]["id"];

/** Line colours from the theme tokens, one per container. */
const COLORS = ["var(--color-accent)", "var(--color-ok)", "var(--color-warn)", "var(--color-level-service)", "var(--color-level-cluster)", "var(--color-level-env)", "var(--color-bad)", "var(--color-level-node)"];
/** Containers drawn on the node view; the rest are listed as "+n more". */
const MAX_LINES = COLORS.length;

function Chart({ title, legend, children }: { title: string; legend: { label: string; color: string; value?: string }[]; children: React.ReactNode }) {
  return (
    <section className="min-w-0 rounded-lg border border-line bg-panel">
      <header className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1 border-b border-line px-3 py-1.5">
        <h3 className="text-xs font-semibold">{title}</h3>
        {legend.map((l) => (
          <span key={l.label} className="inline-flex min-w-0 items-center gap-1 text-[11px] text-muted" title={l.value ? `${l.label}: ${l.value}` : l.label}>
            <span className="size-2 shrink-0 rounded-full" style={{ background: l.color }} aria-hidden />
            <span className="max-w-[140px] truncate">{l.label}</span>
            {l.value && <span className="font-mono tabular-nums text-faint">{l.value}</span>}
          </span>
        ))}
      </header>
      <div className="px-2 py-2">{children}</div>
    </section>
  );
}

/** Points of every series on one shared time axis; missing minutes are gaps. */
function align(list: ContainerSeries[]) {
  const times = [...new Set(list.flatMap((c) => c.points.map((p) => p.t)))].sort((a, b) => a - b);
  const at = list.map((c) => new Map(c.points.map((p) => [p.t, p])));
  return { times, at };
}

const avg = (c: ContainerSeries) => (c.points.length ? c.points.reduce((n, p) => n + p.cpu_avg, 0) / c.points.length : 0);

/**
 * CPU, memory and network history of a node's containers (`name` unset, one
 * line per container, busiest first) or of one container. Data comes from
 * the analytics database, written once a minute by the node.
 */
export function ContainerCharts({ server, name }: { server: string; name?: string }) {
  const [win, setWin] = useState<WindowId>("6");
  const q = useContainerMetrics(server, name, Number(win));
  const off = analyticsUnavailable(q.error);
  const header = (
    <div className="mb-2 flex min-w-0 flex-wrap items-center gap-2">
      <h2 className="text-[13px] font-semibold">{name ? "Resource history" : "Containers"}</h2>
      {q.data && <span className="text-[11px] text-faint">{q.data.resolution === "hour" ? "hourly" : "per minute"}</span>}
      <div className="ml-auto">
        <Segmented label="Window" value={win} options={WINDOWS} onChange={setWin} />
      </div>
    </div>
  );
  if (q.isPending) return (<div>{header}<Skeleton rows={4} /></div>);
  if (off)
    return (
      <div>
        {header}
        <div className="rounded-lg border border-line">
          <Empty icon={<CircleOff className="size-5" />} title="Container history is not available">
            {off} Turn analytics on in <Link to="/settings" className="text-accent hover:underline">Settings</Link> to keep container metrics.
          </Empty>
        </div>
      </div>
    );
  if (q.isError) return (<div>{header}<ErrorNote error={q.error} /></div>);

  const all = [...q.data.containers].filter((c) => c.points.length).sort((a, b) => avg(b) - avg(a));
  if (all.length === 0)
    return (
      <div>
        {header}
        <p className="rounded-lg border border-dashed border-line px-3 py-4 text-center text-xs text-muted">
          No container metrics in this window. The node sends them once a minute while container collection is on (node <b>Details</b> → Container telemetry).
        </p>
      </div>
    );
  const shown = all.slice(0, MAX_LINES);
  const { times, at } = align(shown);
  const from = q.data.from || times[0] || 0;
  const to = q.data.to || times[times.length - 1] || 0;
  const color = (i: number) => COLORS[i % COLORS.length] ?? "var(--color-accent)";
  const last = (c: ContainerSeries) => c.points[c.points.length - 1];
  const series = (pick: (p: ContainerSeries["points"][number]) => number, label?: (c: ContainerSeries) => string): Series[] =>
    shown.map((c, i) => ({ label: label?.(c) ?? c.container_name, color: color(i), values: times.map((t) => { const p = at[i]?.get(t); return p ? pick(p) : null; }) }));
  const single = shown.length === 1 ? shown[0] : undefined;
  const legend = (fmt: (c: ContainerSeries) => string) => shown.map((c, i) => ({ label: c.container_name, color: color(i), value: fmt(c) }));
  const memLimit = single ? last(single)?.mem_limit : undefined;

  return (
    <div className="min-w-0">
      {header}
      <div className="grid gap-3 xl:grid-cols-3">
        <Chart title="CPU" legend={legend((c) => fmtPct(last(c)?.cpu_avg ?? 0))}>
          <TimeSeries times={times} from={from} to={to} height={140} format={fmtPct} series={series((p) => p.cpu_avg)} />
        </Chart>
        <Chart title="Memory" legend={legend((c) => fmtBytes(last(c)?.mem_used ?? 0))}>
          <TimeSeries times={times} from={from} to={to} height={140} max={memLimit || undefined} format={fmtBytes} series={series((p) => p.mem_used)} />
        </Chart>
        {single ? (
          <Chart
            title="Network"
            legend={[
              { label: "↓ in", color: color(0), value: fmtRate(last(single)?.rx ?? 0) },
              { label: "↑ out", color: color(1), value: fmtRate(last(single)?.tx ?? 0) },
            ]}
          >
            <TimeSeries
              times={times}
              from={from}
              to={to}
              height={140}
              format={fmtRate}
              series={[
                { label: "In", color: color(0), values: times.map((t) => at[0]?.get(t)?.rx ?? null) },
                { label: "Out", color: color(1), values: times.map((t) => at[0]?.get(t)?.tx ?? null) },
              ]}
            />
          </Chart>
        ) : (
          <Chart title="Network (↓ in + ↑ out)" legend={legend((c) => fmtRate((last(c)?.rx ?? 0) + (last(c)?.tx ?? 0)))}>
            <TimeSeries times={times} from={from} to={to} height={140} format={fmtRate} series={series((p) => p.rx + p.tx)} />
          </Chart>
        )}
      </div>
      {all.length > shown.length && <p className="mt-1 text-[11px] text-faint">Showing the {shown.length} busiest of {all.length} containers.</p>}
      {single && (last(single)?.restarts ?? 0) > 0 && <p className="mt-1 text-[11px] text-warn">Restarted {last(single)?.restarts} times (Docker restart count).</p>}
    </div>
  );
}
