import { Activity, Cpu, HardDrive, MemoryStick, Network } from "lucide-react";
import { type ReactNode, useMemo, useState } from "react";

import { useNodeMetrics, type MetricSample, type Volume } from "../../api/hooks";
import type { Server } from "../../api/types";
import { ContainerCharts } from "../containers/ContainerCharts";
import { TimeSeries, fmtBytes, fmtPct, fmtRate } from "../charts/TimeSeries";
import { ErrorNote, FadeSwitch, Panel, Segmented, Select, Table, Td } from "../ui";
import { Connectivity } from "./Connectivity";
import { NodeHistory } from "./NodeHistory";

const RANGES = [
  { label: "5 min", ms: 5 * 60_000 },
  { label: "15 min", ms: 15 * 60_000 },
  { label: "1 hour", ms: 60 * 60_000 },
];
const REFRESH = [
  { label: "3 s", ms: 3000 },
  { label: "5 s", ms: 5000 },
  { label: "10 s", ms: 10_000 },
  { label: "30 s", ms: 30_000 },
  { label: "Paused", ms: 0 },
];

const pct = (a: number, b: number) => (b > 0 ? (a / b) * 100 : 0);

function uptime(s: number) {
  const d = Math.floor(s / 86400);
  const h = Math.floor((s % 86400) / 3600);
  const m = Math.floor((s % 3600) / 60);
  return d ? `${d}d ${h}h` : h ? `${h}h ${m}m` : `${m}m`;
}

function Tile({ icon, label, value, sub, ratio }: { icon: React.ReactNode; label: string; value: string; sub?: string; ratio?: number }) {
  const tone = ratio === undefined ? "bg-accent" : ratio > 90 ? "bg-bad" : ratio > 75 ? "bg-warn" : "bg-accent";
  return (
    <div className="rounded-lg border border-line bg-panel p-3.5 shadow-[var(--shadow-card)]">
      <div className="flex items-center gap-2 text-xs text-muted">
        {icon}
        {label}
      </div>
      <div className="mt-1.5 text-xl font-semibold tabular-nums">{value}</div>
      {ratio !== undefined && (
        <div className="mt-2 h-1.5 overflow-hidden rounded-full bg-panel-2">
          <div className={`h-full rounded-full transition-[width] duration-500 ${tone}`} style={{ width: `${Math.min(100, ratio)}%` }} />
        </div>
      )}
      {sub && <div className="mt-1.5 truncate text-xs text-faint">{sub}</div>}
    </div>
  );
}

function ChartPanel({ title, legend, children }: { title: string; legend?: { label: string; color: string; value?: string }[]; children: React.ReactNode }) {
  return (
    <section className="rounded-lg border border-line bg-panel p-3.5 shadow-[var(--shadow-card)]">
      <div className="mb-2 flex flex-wrap items-center gap-x-4 gap-y-1">
        <h3 className="text-[13px] font-semibold">{title}</h3>
        {legend?.map((l) => (
          <span key={l.label} className="flex items-center gap-1.5 text-xs text-muted">
            <span className="size-2 rounded-full" style={{ background: l.color }} />
            {l.label}
            {l.value && <span className="font-medium text-fg tabular-nums">{l.value}</span>}
          </span>
        ))}
      </div>
      {children}
    </section>
  );
}

/** Overview tab: live resource usage, or the history from the analytics database. */
export function NodeOverview({ node }: { node: Server }) {
  const [mode, setMode] = useState<"live" | "history">("live");
  // The toggle leads the one-line toolbar each view renders (status left, range/refresh right).
  const toggle = (
    <Segmented
      label="Live or history"
      value={mode}
      onChange={setMode}
      options={[
        { id: "live", label: "Live" },
        { id: "history", label: "History" },
      ]}
    />
  );
  return (
    <div className="space-y-4">
      <FadeSwitch k={mode}>{mode === "live" ? <LiveOverview node={node} lead={toggle} /> : <NodeHistory node={node} lead={toggle} />}</FadeSwitch>
      <ContainerCharts server={node.id} />
    </div>
  );
}

/** Live resource usage and connectivity to the node's peers. */
function LiveOverview({ node, lead }: { node: Server; lead?: ReactNode }) {
  const [range, setRange] = useState(RANGES[1]?.ms ?? 900_000);
  const [refresh, setRefresh] = useState(3000);
  const online = node.status === "online";
  const { samples, volumes, error } = useNodeMetrics(node.id, online ? refresh : 0, RANGES[RANGES.length - 1]?.ms ?? 3_600_000);
  const now = samples.length ? Math.max(Date.now(), samples[samples.length - 1]?.t ?? 0) : Date.now();
  const from = now - range;
  const shown = useMemo(() => samples.filter((s) => s.t >= from), [samples, from]);
  const latest: MetricSample | undefined = samples[samples.length - 1];
  const times = shown.map((s) => s.t);

  const accent = "var(--color-accent)";
  const accent2 = "var(--color-accent-2)";
  const warn = "var(--color-warn)";

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
        {lead}
        <span className={`flex min-w-0 items-center gap-1.5 text-xs ${online && refresh ? "text-ok" : "text-faint"}`}>
          <span className={`size-2 rounded-full ${online && refresh ? "animate-pulse bg-ok" : "bg-faint"}`} />
          {online ? (refresh ? "Live" : "Paused") : "Agent offline — showing the last samples received"}
        </span>
        <div className="ml-auto flex items-center gap-2 text-xs text-muted">
          <label className="flex items-center gap-1.5">
            Range
            <Select className="w-auto py-1 text-xs" value={range} onChange={(e) => setRange(Number(e.target.value))}>
              {RANGES.map((r) => (
                <option key={r.ms} value={r.ms}>
                  {r.label}
                </option>
              ))}
            </Select>
          </label>
          <label className="flex items-center gap-1.5">
            Refresh
            <Select className="w-auto py-1 text-xs" value={refresh} onChange={(e) => setRefresh(Number(e.target.value))} disabled={!online}>
              {REFRESH.map((r) => (
                <option key={r.ms} value={r.ms}>
                  {r.label}
                </option>
              ))}
            </Select>
          </label>
        </div>
      </div>
      <ErrorNote error={error} />

      <div className="grid grid-cols-2 gap-3 lg:grid-cols-4">
        <Tile
          icon={<Cpu className="size-4" />}
          label="CPU"
          value={latest ? fmtPct(latest.cpu) : "—"}
          ratio={latest?.cpu}
          sub={latest ? `${latest.cpus} cores · load ${latest.load1.toFixed(2)}` : undefined}
        />
        <Tile
          icon={<MemoryStick className="size-4" />}
          label="Memory"
          value={latest ? fmtPct(pct(latest.mem_used, latest.mem_total)) : "—"}
          ratio={latest ? pct(latest.mem_used, latest.mem_total) : undefined}
          sub={latest ? `${fmtBytes(latest.mem_used)} of ${fmtBytes(latest.mem_total)} · ${fmtBytes(latest.mem_total - latest.mem_used)} free` : undefined}
        />
        <Tile
          icon={<HardDrive className="size-4" />}
          label="Disk"
          value={latest?.disk_total ? fmtPct(pct(latest.disk_used, latest.disk_total)) : "—"}
          ratio={latest?.disk_total ? pct(latest.disk_used, latest.disk_total) : undefined}
          sub={latest?.disk_total ? `${fmtBytes(latest.disk_used)} of ${fmtBytes(latest.disk_total)}` : undefined}
        />
        <Tile
          icon={<Network className="size-4" />}
          label="Network"
          value={latest ? `↓ ${fmtRate(latest.rx)}` : "—"}
          sub={latest ? `↑ ${fmtRate(latest.tx)} · up ${uptime(latest.uptime)}` : undefined}
        />
      </div>

      <div className="grid gap-4 xl:grid-cols-2">
        <ChartPanel title="CPU utilization" legend={[{ label: "busy", color: accent, value: latest ? fmtPct(latest.cpu) : undefined }]}>
          <TimeSeries times={times} from={from} to={now} max={100} format={fmtPct} series={[{ label: "CPU", color: accent, values: shown.map((s) => s.cpu) }]} />
        </ChartPanel>
        <ChartPanel
          title="Memory"
          legend={[
            { label: "used", color: accent, value: latest ? fmtBytes(latest.mem_used) : undefined },
            { label: "swap", color: warn, value: latest?.swap_total ? fmtBytes(latest.swap_used) : undefined },
          ]}
        >
          <TimeSeries
            times={times}
            from={from}
            to={now}
            max={latest?.mem_total || undefined}
            format={fmtBytes}
            series={[
              { label: "Used", color: accent, values: shown.map((s) => s.mem_used) },
              { label: "Swap", color: warn, values: shown.map((s) => (s.swap_total ? s.swap_used : null)) },
            ]}
          />
        </ChartPanel>
        <ChartPanel
          title="Network throughput"
          legend={[
            { label: "in", color: accent, value: latest ? fmtRate(latest.rx) : undefined },
            { label: "out", color: accent2, value: latest ? fmtRate(latest.tx) : undefined },
          ]}
        >
          <TimeSeries
            times={times}
            from={from}
            to={now}
            format={fmtRate}
            series={[
              { label: "In", color: accent, values: shown.map((s) => s.rx) },
              { label: "Out", color: accent2, values: shown.map((s) => s.tx) },
            ]}
          />
        </ChartPanel>
        <ChartPanel title="Disk usage & load" legend={[{ label: "disk used", color: accent, value: latest?.disk_total ? fmtPct(pct(latest.disk_used, latest.disk_total)) : undefined }, { label: "load per core", color: warn }]}>
          <TimeSeries
            times={times}
            from={from}
            to={now}
            max={100}
            format={(v) => v.toFixed(0)}
            series={[
              { label: "Disk %", color: accent, values: shown.map((s) => (s.disk_total ? pct(s.disk_used, s.disk_total) : null)) },
              { label: "Load/core %", color: warn, values: shown.map((s) => (s.cpus ? (s.load1 / s.cpus) * 100 : null)) },
            ]}
          />
        </ChartPanel>
      </div>
      <p className="flex items-center gap-1.5 text-xs text-faint">
        <Activity className="size-3.5" /> Samples come from the node's agent with each heartbeat. This live view is kept in memory for about an hour; switch to History for the per-minute record in the analytics database.
      </p>

      <Panel title="Storage" description={volumes.length ? `${volumes.length} mounted volume${volumes.length === 1 ? "" : "s"}, as reported by the agent with each sample.` : undefined}>
        <Volumes volumes={volumes} />
      </Panel>

      <Panel title="Inter-node connectivity" description={`Checks run from ${node.hostname} itself, so they show this node's own path to its peers.`}>
        <Connectivity node={node} />
      </Panel>
    </div>
  );
}

function Volumes({ volumes }: { volumes: Volume[] }) {
  if (volumes.length === 0) return <p className="px-4 py-6 text-[13px] text-muted">Waiting for the agent to report its volumes…</p>;
  const sorted = [...volumes].sort((a, b) => a.mount_point.localeCompare(b.mount_point));
  const total = volumes.reduce((a, v) => a + v.total, 0);
  const used = volumes.reduce((a, v) => a + v.used, 0);
  return (
    <Table head={["Mount point", "Device", "File system", "Total", "Used", "Available", "Used %"]}>
      {sorted.map((v) => {
        const p = pct(v.used, v.total);
        const tone = p > 90 ? "bg-bad" : p > 75 ? "bg-warn" : "bg-accent";
        return (
          <tr key={v.mount_point} className="hover:bg-panel-2/60">
            <Td className="font-mono text-xs font-medium">{v.mount_point}</Td>
            <Td className="max-w-[220px] truncate font-mono text-xs text-muted" title={v.device}>
              {v.device}
            </Td>
            <Td className="text-xs text-muted">
              {v.file_system}
              {v.removable && " · removable"}
            </Td>
            <Td className="tabular-nums">{fmtBytes(v.total)}</Td>
            <Td className="tabular-nums">{fmtBytes(v.used)}</Td>
            <Td className="tabular-nums">{fmtBytes(v.available)}</Td>
            <Td>
              <div className="flex min-w-[140px] items-center gap-2">
                <div className="h-1.5 flex-1 overflow-hidden rounded-full bg-panel-2">
                  <div className={`h-full rounded-full ${tone}`} style={{ width: `${Math.min(100, p)}%` }} />
                </div>
                <span className={`w-11 text-right text-xs tabular-nums ${p > 90 ? "text-bad" : p > 75 ? "text-warn" : "text-muted"}`}>{fmtPct(p)}</span>
              </div>
            </Td>
          </tr>
        );
      })}
      {volumes.length > 1 && (
        <tr className="bg-panel-2/40 text-[13px] font-medium">
          <Td colSpan={3}>All volumes</Td>
          <Td className="tabular-nums">{fmtBytes(total)}</Td>
          <Td className="tabular-nums">{fmtBytes(used)}</Td>
          <Td className="tabular-nums">{fmtBytes(total - used)}</Td>
          <Td className="text-xs text-muted tabular-nums">{fmtPct(pct(used, total))}</Td>
        </tr>
      )}
    </Table>
  );
}
