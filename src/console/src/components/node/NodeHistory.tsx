import { History } from "lucide-react";
import { type ReactNode, useState } from "react";

import { analyticsUnavailable, useNodeReport, type Peak, type ProcessUsage } from "../../api/analytics";
import type { Server } from "../../api/types";
import { TimeSeries, fmtBytes, fmtPct, fmtRate } from "../charts/TimeSeries";
import { Empty, ErrorNote, Panel, Select, Table, Td } from "../ui";

const WINDOWS = [
  { label: "1 hour", hours: 1 },
  { label: "24 hours", hours: 24 },
  { label: "7 days", hours: 168 },
  { label: "30 days", hours: 720 },
];

const pct = (a: number, b: number) => (b > 0 ? (a / b) * 100 : 0);
const when = (t: number) => new Date(t).toLocaleString(undefined, { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" });

function PeakTile({ label, peak }: { label: string; peak: Peak | null }) {
  const tone = !peak ? "text-faint" : peak.value > 90 ? "text-bad" : peak.value > 75 ? "text-warn" : "text-fg";
  return (
    <div className="rounded-lg border border-line bg-panel p-3.5 shadow-[var(--shadow-card)]">
      <div className="text-xs text-muted">{label}</div>
      <div className={`mt-1.5 text-xl font-semibold tabular-nums ${tone}`}>{peak ? fmtPct(peak.value) : "—"}</div>
      <div className="mt-1 text-xs text-faint">{peak ? when(peak.t) : "no data in this window"}</div>
    </div>
  );
}

function Processes({ rows, by }: { rows: ProcessUsage[]; by: "cpu" | "mem" }) {
  if (rows.length === 0) return <p className="px-4 py-6 text-[13px] text-muted">No process data in this window yet.</p>;
  return (
    <Table head={["Process", "User", by === "cpu" ? "Avg CPU" : "Avg memory", by === "cpu" ? "Max CPU" : "Max memory", "Minutes in top"]}>
      {rows.map((p) => (
        <tr key={`${p.name}\u0000${p.user}`} className="hover:bg-panel-2/60">
          <Td className="max-w-[320px]">
            <div className="truncate font-mono text-xs font-medium">{p.name}</div>
            <div className="truncate font-mono text-[11px] text-faint" title={p.cmd}>
              {p.cmd}
            </div>
          </Td>
          <Td className="text-xs text-muted">{p.user || "—"}</Td>
          <Td className="tabular-nums">{by === "cpu" ? fmtPct(p.cpu_avg) : fmtBytes(p.mem_avg)}</Td>
          <Td className="tabular-nums">{by === "cpu" ? fmtPct(p.cpu_max) : fmtBytes(p.mem_max)}</Td>
          <Td className="tabular-nums text-muted">{p.minutes}</Td>
        </tr>
      ))}
    </Table>
  );
}

/** History tab body: trends, peaks and top processes from the analytics database. */
export function NodeHistory({ node, lead }: { node: Server; lead?: ReactNode }) {
  const [hours, setHours] = useState(24);
  const q = useNodeReport(node.id, hours);
  const r = q.data;
  const off = analyticsUnavailable(q.error);
  const accent = "var(--color-accent)";
  const accent2 = "var(--color-accent-2)";
  const warn = "var(--color-warn)";
  const to = r?.to ?? Date.now();
  const from = r?.from ?? to - hours * 3_600_000;
  const pts = r?.points ?? [];
  const times = pts.map((p) => p.t);

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
        {lead}
        <span className="flex min-w-0 items-center gap-1.5 text-xs text-muted">
          <History className="size-3.5" />
          {r ? `${r.resolution === "minute" ? "One point per minute" : "One point per hour"} · ${pts.length} points` : "History"}
        </span>
        <label className="ml-auto flex items-center gap-1.5 text-xs text-muted">
          Window
          <Select className="w-auto py-1 text-xs" value={hours} onChange={(e) => setHours(Number(e.target.value))}>
            {WINDOWS.map((w) => (
              <option key={w.hours} value={w.hours}>
                {w.label}
              </option>
            ))}
          </Select>
        </label>
      </div>

      {off ? (
        <Empty icon={<History className="size-5" />} title="History is not available">
          {off}
        </Empty>
      ) : (
        <ErrorNote error={q.error} />
      )}

      {r && (
        <>
          <div className="grid grid-cols-1 gap-3 sm:grid-cols-3">
            <PeakTile label="Peak CPU" peak={r.cpu_peak} />
            <PeakTile label="Peak memory" peak={r.mem_peak} />
            <PeakTile label="Peak disk" peak={r.disk_peak} />
          </div>

          <div className="grid gap-4 xl:grid-cols-2">
            <Panel title="CPU" description="Average and highest per point">
              <div className="p-3">
                <TimeSeries
                  times={times}
                  from={from}
                  to={to}
                  max={100}
                  format={fmtPct}
                  series={[
                    { label: "Average", color: accent, values: pts.map((p) => p.cpu_avg) },
                    { label: "Highest", color: warn, values: pts.map((p) => p.cpu_max) },
                  ]}
                />
              </div>
            </Panel>
            <Panel title="Memory" description="Used, average per point">
              <div className="p-3">
                <TimeSeries
                  times={times}
                  from={from}
                  to={to}
                  max={Math.max(0, ...pts.map((p) => p.mem_total)) || undefined}
                  format={fmtBytes}
                  series={[{ label: "Used", color: accent, values: pts.map((p) => p.mem_used) }]}
                />
              </div>
            </Panel>
            <Panel title="Disk" description="Used, percent of local disks">
              <div className="p-3">
                <TimeSeries
                  times={times}
                  from={from}
                  to={to}
                  max={100}
                  format={fmtPct}
                  series={[{ label: "Disk", color: accent, values: pts.map((p) => (p.disk_total ? pct(p.disk_used, p.disk_total) : null)) }]}
                />
              </div>
            </Panel>
            <Panel title="Network" description="Average throughput">
              <div className="p-3">
                <TimeSeries
                  times={times}
                  from={from}
                  to={to}
                  format={fmtRate}
                  series={[
                    { label: "In", color: accent, values: pts.map((p) => p.rx) },
                    { label: "Out", color: accent2, values: pts.map((p) => p.tx) },
                  ]}
                />
              </div>
            </Panel>
          </div>

          <div className="grid gap-4 xl:grid-cols-2">
            <Panel title="Top processes by CPU" description="Ten processes that used the most CPU in this window">
              <Processes rows={r.top_cpu} by="cpu" />
            </Panel>
            <Panel title="Top processes by memory" description="Ten processes that used the most memory in this window">
              <Processes rows={r.top_mem} by="mem" />
            </Panel>
          </div>
        </>
      )}
    </div>
  );
}
