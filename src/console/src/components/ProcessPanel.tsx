/**
 * htop-like process view for one node: per-core CPU meters, memory and swap
 * bars, tasks / load / uptime, and a sortable process table.
 *
 * Polls `GET /servers/{id}/processes` at the chosen interval (default 5 s;
 * Live = 2 s), only while the panel is mounted and the browser tab is
 * visible. Typing a filter narrows the rows already shown at once, then
 * refreshes from the node (debounced), so the list never waits on the network.
 *
 * Linux and Windows nodes share the panel; columns and actions follow what
 * the node reports (`summary.os`, `actions`). With `process.manage` each row
 * has a menu: signals and nice on Linux, End task / End process / priority
 * class on Windows (D-080). The server asks for MFA step-up and the node
 * refuses protected processes and reused PIDs.
 */
import { useMutation, useQuery } from "@tanstack/react-query";
import { ArrowDown, Gauge, RefreshCw, Search, Skull, X } from "lucide-react";
import { useEffect, useMemo, useState } from "react";

import { get, post } from "../api/client";
import { errorText, useToast } from "../app/toast";
import { Button, Confirm, Field, Input, Modal, RowMenu, Select, Switch, type RowAction } from "./ui";

type SortKey = "cpu" | "mem" | "pid" | "time" | "name";

interface Proc {
  pid: number;
  ppid: number | null;
  user: string;
  /** htop letter on Linux; null on Windows. */
  state: string | null;
  cpu: number;
  mem: number;
  virt: number;
  mem_pct: number;
  time: number;
  /** Start time, epoch seconds: actions name it so a reused PID is never hit. */
  started: number;
  /** Linux only. */
  nice?: number | null;
  name: string;
  cmd: string;
}

/** What the node can do to a process (absent on older agents). */
interface Capabilities {
  signals: string[];
  tree: boolean;
  nice: boolean;
  priority_classes: string[];
}

interface Snapshot {
  summary: {
    os?: string;
    cpus: number[];
    cpu_total: number;
    /** Null on Windows, which has no load average. */
    load: [number, number, number] | null;
    mem_total: number;
    mem_used: number;
    swap_total: number;
    swap_used: number;
    tasks: number;
    running: number;
    uptime: number;
  };
  matched: number;
  processes: Proc[];
  actions?: Capabilities;
}

const SIGNAL_LABEL: Record<string, string> = { term: "Send SIGTERM", kill: "Send SIGKILL", hup: "Send SIGHUP", int: "Send SIGINT" };
const CLASS_LABEL: Record<string, string> = {
  idle: "Idle",
  below_normal: "Below normal",
  normal: "Normal",
  above_normal: "Above normal",
  high: "High",
};

type Pending = { kind: "signal"; proc: Proc; signal: string; tree: boolean } | { kind: "priority"; proc: Proc };

const INTERVALS: { label: string; ms: number }[] = [
  { label: "Live (2s)", ms: 2000 },
  { label: "5s", ms: 5000 },
  { label: "10s", ms: 10000 },
  { label: "30s", ms: 30000 },
];

const KEY = "forge-proc-interval";

function bytes(n: number): string {
  const u = ["B", "K", "M", "G", "T"];
  let i = 0;
  let v = n;
  while (v >= 1024 && i < u.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v >= 100 || i === 0 ? v.toFixed(0) : v.toFixed(1)}${u[i]}`;
}

function clock(secs: number): string {
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  const s = secs % 60;
  return h > 0 ? `${h}:${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}` : `${m}:${String(s).padStart(2, "0")}`;
}

function uptime(secs: number): string {
  const d = Math.floor(secs / 86400);
  return `${d > 0 ? `${d} day${d === 1 ? "" : "s"}, ` : ""}${clock(secs % 86400)}`;
}

/** htop-style bar: `[|||||     42.1%]`, coloured by load. */
function Meter({ label, pct, text }: { label: string; pct: number; text: string }) {
  const p = Math.max(0, Math.min(100, pct));
  const tone = p >= 90 ? "bg-bad" : p >= 70 ? "bg-warn" : "bg-ok";
  return (
    <div className="flex items-center gap-1.5 font-mono text-[11.5px] leading-5">
      <span className="w-7 shrink-0 text-right text-accent">{label}</span>
      <span className="text-faint">[</span>
      <span className="relative h-2.5 flex-1 overflow-hidden rounded-sm bg-panel-2">
        <span className={`absolute inset-y-0 left-0 ${tone} transition-[width] duration-500`} style={{ width: `${p}%` }} />
      </span>
      <span className="w-[86px] shrink-0 text-right tabular-nums">{text}</span>
      <span className="text-faint">]</span>
    </div>
  );
}

const COLS: { key: string; label: string; sort?: SortKey; className: string }[] = [
  { key: "pid", label: "PID", sort: "pid", className: "w-16 text-right" },
  { key: "user", label: "USER", className: "w-24" },
  { key: "nice", label: "NI", className: "w-8 text-right" },
  { key: "state", label: "S", className: "w-5 text-center" },
  { key: "cpu", label: "CPU%", sort: "cpu", className: "w-14 text-right" },
  { key: "mem_pct", label: "MEM%", sort: "mem", className: "w-14 text-right" },
  { key: "virt", label: "VIRT", className: "w-16 text-right" },
  { key: "mem", label: "RES", sort: "mem", className: "w-16 text-right" },
  { key: "time", label: "TIME+", sort: "time", className: "w-20 text-right" },
  { key: "cmd", label: "Command", sort: "name", className: "" },
];

/** Signal or End task / End process confirmation, with the tree option. */
function SignalDialog({ pending, windows, onClose, onDone, serverId }: { pending: Extract<Pending, { kind: "signal" }>; windows: boolean; onClose: () => void; onDone: () => void; serverId: string }) {
  const toast = useToast();
  const [tree, setTree] = useState(pending.tree);
  const p = pending.proc;
  const m = useMutation({
    mutationFn: () => post<{ affected?: number[] }>(`/api/v1/servers/${serverId}/processes/${p.pid}/signal`, { start_time: p.started, signal: pending.signal, tree }),
    onSuccess: (r) => {
      const n = r.affected?.length ?? 1;
      toast(windows ? `${pending.signal === "kill" ? "Ended" : "Asked to close"} ${n === 1 ? p.name : `${n} processes`}` : `Sent SIG${pending.signal.toUpperCase()} to ${n === 1 ? p.name : `${n} processes`}`);
      onDone();
    },
    onError: (e) => toast(errorText(e), "error"),
  });
  const title = windows ? (pending.signal === "kill" ? (tree ? "End process tree" : "End process") : "End task") : `Send SIG${pending.signal.toUpperCase()}`;
  return (
    <Confirm
      open
      onOpenChange={(o) => !o && onClose()}
      title={title}
      confirmLabel={title}
      busy={m.isPending}
      onConfirm={() => m.mutate()}
      body={
        <div className="space-y-3">
          <p>
            <span className="font-mono text-fg">{p.name}</span> (PID {p.pid}, {p.user || "unknown user"})
          </p>
          <p className="truncate font-mono text-xs text-faint" title={p.cmd}>
            {p.cmd}
          </p>
          {windows && pending.signal === "term" && (
            <p className="text-xs">Asks the process to close. Programs without a window the service can reach (most desktop apps and console programs) refuse; use End process then.</p>
          )}
          {windows && pending.signal === "kill" && <p className="text-xs">Ends the process at once. Unsaved work in it is lost.</p>}
          {!windows && pending.signal === "kill" && <p className="text-xs">SIGKILL cannot be caught: the process ends at once without cleaning up.</p>}
          <Switch checked={tree} onChange={setTree} label="Include child processes" />
        </div>
      }
    />
  );
}

/** Nice value (Linux) or priority class (Windows). */
function PriorityDialog({ proc, windows, classes, onClose, onDone, serverId }: { proc: Proc; windows: boolean; classes: string[]; onClose: () => void; onDone: () => void; serverId: string }) {
  const toast = useToast();
  const [nice, setNice] = useState(String(proc.nice ?? 0));
  const [cls, setCls] = useState("normal");
  const n = Number(nice);
  const valid = windows ? classes.includes(cls) : Number.isInteger(n) && n >= -20 && n <= 19 && nice.trim() !== "";
  const m = useMutation({
    mutationFn: () => post(`/api/v1/servers/${serverId}/processes/${proc.pid}/priority`, windows ? { start_time: proc.started, class: cls } : { start_time: proc.started, nice: n }),
    onSuccess: () => {
      toast(windows ? `${proc.name}: priority ${CLASS_LABEL[cls] ?? cls}` : `${proc.name}: nice ${n}`);
      onDone();
    },
    onError: (e) => toast(errorText(e), "error"),
  });
  return (
    <Modal open onOpenChange={(o) => !o && onClose()} title={windows ? "Priority class" : "Renice"}>
      <div className="space-y-3">
        <p className="text-muted">
          <span className="font-mono text-fg">{proc.name}</span> (PID {proc.pid})
        </p>
        {windows ? (
          <Field label="Priority class" hint="Realtime is not offered: it can starve Windows itself.">
            <Select aria-label="Priority class" value={cls} onChange={(e) => setCls(e.target.value)}>
              {classes.map((c) => (
                <option key={c} value={c}>
                  {CLASS_LABEL[c] ?? c}
                </option>
              ))}
            </Select>
          </Field>
        ) : (
          <Field label="Nice value" hint="-20 runs first, 19 runs last. Current value preselected.">
            <Input type="number" min={-20} max={19} step={1} value={nice} onChange={(e) => setNice(e.target.value)} aria-label="Nice value" />
          </Field>
        )}
        <div className="flex justify-end gap-2">
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" disabled={!valid || m.isPending} onClick={() => m.mutate()}>
            Apply
          </Button>
        </div>
      </div>
    </Modal>
  );
}

export function ProcessPanel({ serverId, online, canManage = false }: { serverId: string; online: boolean; canManage?: boolean }) {
  const [sort, setSort] = useState<SortKey>("cpu");
  const [interval, setIntervalMs] = useState<number>(() => {
    const v = Number(typeof localStorage !== "undefined" ? localStorage.getItem(KEY) : NaN);
    return INTERVALS.some((i) => i.ms === v) ? v : 5000;
  });
  const [filter, setFilter] = useState("");
  const [pending, setPending] = useState<Pending | null>(null);
  const [remoteFilter, setRemoteFilter] = useState("");
  const [visible, setVisible] = useState(() => typeof document === "undefined" || document.visibilityState === "visible");

  useEffect(() => {
    const on = () => setVisible(document.visibilityState === "visible");
    document.addEventListener("visibilitychange", on);
    return () => document.removeEventListener("visibilitychange", on);
  }, []);
  // Debounce the server-side filter; the local one applies immediately.
  useEffect(() => {
    const t = setTimeout(() => setRemoteFilter(filter.trim()), 350);
    return () => clearTimeout(t);
  }, [filter]);
  useEffect(() => {
    try {
      localStorage.setItem(KEY, String(interval));
    } catch {
      /* not persisted */
    }
  }, [interval]);

  const q = useQuery({
    queryKey: ["processes", serverId, sort, remoteFilter],
    queryFn: () =>
      get<Snapshot>(`/api/v1/servers/${serverId}/processes?sort=${sort}&limit=150&filter=${encodeURIComponent(remoteFilter)}`),
    enabled: online && visible,
    refetchInterval: online && visible ? interval : false,
    placeholderData: (prev) => prev,
    retry: false,
  });
  const data = q.data;

  const rows = useMemo(() => {
    const all = data?.processes ?? [];
    const n = filter.trim().toLowerCase();
    return n ? all.filter((p) => `${p.name} ${p.cmd} ${p.user}`.toLowerCase().includes(n)) : all;
  }, [data, filter]);

  if (!online) return <p className="p-4 text-muted">The node is offline; processes are shown while it is connected.</p>;
  const s = data?.summary;
  const windows = s?.os === "windows";
  const caps = data?.actions;
  const showActions = canManage && !!caps;
  // Columns the node reports: no state letters or nice values on Windows.
  const cols = COLS.filter((c) => (c.key === "state" ? !windows : c.key === "nice" ? rows.some((p) => typeof p.nice === "number") : true));
  const has = (k: string) => cols.some((c) => c.key === k);

  const menu = (p: Proc): RowAction[] => {
    if (!caps) return [];
    const acts: RowAction[] = [];
    if (windows) {
      if (caps.signals.includes("term")) acts.push({ label: "End task", icon: X, onSelect: () => setPending({ kind: "signal", proc: p, signal: "term", tree: false }) });
      if (caps.signals.includes("kill")) {
        acts.push({ label: "End process", icon: Skull, danger: true, onSelect: () => setPending({ kind: "signal", proc: p, signal: "kill", tree: false }) });
        if (caps.tree) acts.push({ label: "End process tree", icon: Skull, danger: true, onSelect: () => setPending({ kind: "signal", proc: p, signal: "kill", tree: true }) });
      }
      if (caps.priority_classes.length) acts.push({ label: "Priority class…", icon: Gauge, onSelect: () => setPending({ kind: "priority", proc: p }) });
    } else {
      if (caps.nice) acts.push({ label: "Renice…", icon: Gauge, onSelect: () => setPending({ kind: "priority", proc: p }) });
      for (const sig of caps.signals) {
        acts.push({ label: SIGNAL_LABEL[sig] ?? sig, icon: sig === "kill" ? Skull : X, danger: sig === "kill" || sig === "term", onSelect: () => setPending({ kind: "signal", proc: p, signal: sig, tree: false }) });
      }
    }
    return acts;
  };
  const done = () => {
    setPending(null);
    void q.refetch();
  };
  const cores = s?.cpus ?? [];
  const half = Math.ceil(cores.length / 2);

  return (
    <div className="space-y-2">
      {/* htop header: CPU meters left, memory and stats right */}
      <div className="grid gap-x-6 gap-y-1 rounded-md border border-line bg-panel p-3 md:grid-cols-2">
        <div className="grid grid-cols-2 gap-x-4">
          {[cores.slice(0, half), cores.slice(half)].map((col, ci) => (
            <div key={ci}>
              {col.map((c, i) => (
                <Meter key={i} label={String(ci * half + i + 1)} pct={c} text={`${c.toFixed(1)}%`} />
              ))}
            </div>
          ))}
          {!cores.length && <span className="text-xs text-muted">{q.isFetching ? "Sampling…" : "No data yet"}</span>}
        </div>
        <div>
          {s && (
            <>
              <Meter label="Mem" pct={s.mem_total ? (s.mem_used * 100) / s.mem_total : 0} text={`${bytes(s.mem_used)}/${bytes(s.mem_total)}`} />
              <Meter label="Swp" pct={s.swap_total ? (s.swap_used * 100) / s.swap_total : 0} text={`${bytes(s.swap_used)}/${bytes(s.swap_total)}`} />
              <div className="mt-1 space-y-0.5 pl-9 font-mono text-[11.5px] leading-5">
                <div>
                  <span className="text-accent">Tasks:</span> <b>{s.tasks}</b>; <span className="text-ok">{s.running}</span> running
                </div>
                {s.load ? (
                  <div>
                    <span className="text-accent">Load average:</span> {s.load.map((l) => l.toFixed(2)).join(" ")}
                  </div>
                ) : (
                  <div>
                    <span className="text-accent">CPU:</span> {s.cpu_total.toFixed(1)}%
                  </div>
                )}
                <div>
                  <span className="text-accent">Uptime:</span> {uptime(s.uptime)}
                </div>
              </div>
            </>
          )}
        </div>
      </div>

      {/* toolbar */}
      <div className="flex flex-wrap items-center gap-2">
        <div className="flex min-w-48 flex-1 items-center gap-1.5 rounded-md border border-line bg-panel px-2.5 focus-within:border-accent">
          <Search className="size-3.5 text-faint" />
          <input
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            placeholder="Filter by name, command or user"
            aria-label="Filter processes"
            data-bare className="h-8 w-full bg-transparent text-[13px] outline-none placeholder:text-faint"
          />
        </div>
        <div className="w-32">
          <Select aria-label="Refresh interval" value={String(interval)} onChange={(e) => setIntervalMs(Number(e.target.value))}>
            {INTERVALS.map((i) => (
              <option key={i.ms} value={String(i.ms)}>
                {i.label}
              </option>
            ))}
          </Select>
        </div>
        <Button onClick={() => void q.refetch()} disabled={q.isFetching} title="Refresh now">
          <RefreshCw className={`size-4 ${q.isFetching ? "animate-spin" : ""}`} /> Refresh
        </Button>
        <span className="text-xs text-muted tabular-nums">
          {q.error
            ? errorText(q.error)
            : q.dataUpdatedAt
              ? `${rows.length} of ${data?.matched ?? 0} shown · ${new Date(q.dataUpdatedAt).toLocaleTimeString()}`
              : ""}
        </span>
      </div>

      {/* process table */}
      <div className="max-h-[480px] overflow-auto rounded-md border border-line">
        <table className="w-full border-collapse font-mono text-[12px]">
          <thead className="sticky top-0 z-10 bg-accent text-bg">
            <tr>
              {cols.map((c) => (
                <th
                  key={c.key}
                  className={`px-1.5 py-1 text-left font-semibold ${c.className} ${c.sort ? "cursor-pointer select-none hover:bg-black/15" : ""} ${c.sort === sort ? "bg-black/25" : ""}`}
                  onClick={() => c.sort && setSort(c.sort)}
                  aria-sort={c.sort === sort ? "descending" : undefined}
                >
                  <span className="inline-flex items-center gap-0.5">
                    {c.label}
                    {c.sort === sort && <ArrowDown className="size-3" />}
                  </span>
                </th>
              ))}
              {showActions && <th className="w-9" aria-label="Actions" />}
            </tr>
          </thead>
          <tbody>
            {rows.map((p) => (
              <tr key={p.pid} className="border-b border-line/50 hover:bg-accent-soft">
                <td className="px-1.5 py-0.5 text-right tabular-nums">{p.pid}</td>
                <td className="max-w-40 truncate px-1.5 py-0.5 text-accent" title={p.user}>
                  {p.user || "-"}
                </td>
                {has("nice") && <td className={`px-1.5 py-0.5 text-right tabular-nums ${(p.nice ?? 0) < 0 ? "text-warn" : (p.nice ?? 0) > 0 ? "text-muted" : ""}`}>{p.nice ?? ""}</td>}
                {has("state") && <td className={`px-1.5 py-0.5 text-center ${p.state === "R" ? "text-ok" : p.state === "D" || p.state === "Z" ? "text-bad" : "text-muted"}`}>{p.state ?? ""}</td>}
                <td className={`px-1.5 py-0.5 text-right tabular-nums ${p.cpu >= 50 ? "text-bad" : p.cpu >= 10 ? "text-warn" : ""}`}>{p.cpu.toFixed(1)}</td>
                <td className="px-1.5 py-0.5 text-right tabular-nums">{p.mem_pct.toFixed(1)}</td>
                <td className="px-1.5 py-0.5 text-right tabular-nums text-muted">{bytes(p.virt)}</td>
                <td className="px-1.5 py-0.5 text-right tabular-nums">{bytes(p.mem)}</td>
                <td className="px-1.5 py-0.5 text-right tabular-nums text-muted">{clock(p.time)}</td>
                <td className="max-w-0 truncate px-1.5 py-0.5" title={p.cmd}>
                  <span className="font-semibold">{p.name}</span>
                  {p.cmd !== p.name && <span className="text-muted"> {p.cmd.startsWith(p.name) ? p.cmd.slice(p.name.length) : p.cmd}</span>}
                </td>
                {showActions && (
                  <td className="px-0.5 py-0 text-right">
                    <RowMenu actions={menu(p)} label={`Actions for ${p.name} (${p.pid})`} />
                  </td>
                )}
              </tr>
            ))}
            {!rows.length && (
              <tr>
                <td colSpan={cols.length + (showActions ? 1 : 0)} className="px-3 py-4 text-center text-muted">
                  {q.isFetching ? "Loading…" : filter ? "No process matches the filter" : "No processes"}
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>
      {pending?.kind === "signal" && <SignalDialog key={`${pending.proc.pid}-${pending.signal}`} pending={pending} windows={windows} serverId={serverId} onClose={() => setPending(null)} onDone={done} />}
      {pending?.kind === "priority" && (
        <PriorityDialog key={pending.proc.pid} proc={pending.proc} windows={windows} classes={caps?.priority_classes ?? []} serverId={serverId} onClose={() => setPending(null)} onDone={done} />
      )}
    </div>
  );
}
