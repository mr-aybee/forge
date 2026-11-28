import { AlertTriangle, ChevronDown, ChevronRight, Loader2, RotateCcw, RotateCw } from "lucide-react";
import { type ReactNode, useState } from "react";

import { useMyCapabilities } from "../api/access";
import {
  type AnalyticsConfirmation,
  type AnalyticsSetting,
  type AnalyticsSettings as Settings,
  type AnalyticsSettingsUpdate,
  type ApplyReport,
  type ApplyStatus,
  confirmationsOf,
  useAnalyticsSettings,
  useCancelAnalyticsRestart,
  useRestartAnalytics,
  useUpdateAnalyticsSettings,
} from "../api/analyticsSettings";
import { ApiError } from "../api/client";
import { errorText, useToast } from "../app/toast";
import { SettingsTabs } from "../components/SettingsTabs";
import { ago, Badge, bytes, Button, Confirm, duration, ErrorNote, Input, Modal, PageHeader, Panel, Skeleton, time, UnsavedBar } from "../components/ui";

// ------------------------------------------------------------------ setting metadata

type Unit = "days" | "vcpu" | "mib" | "secs" | "ms" | "rows" | "count";

interface Meta {
  label: string;
  unit: Unit;
  hint?: string;
  /** What 0 means, when it is special. */
  zero?: string;
  /** Fractional values allowed. */
  fractional?: boolean;
  /** Smallest non-zero value. */
  minNonZero?: number;
}

const META: Record<string, Meta> = {
  node_metrics_days: { label: "Minute-level history", unit: "days", hint: "Per-minute node metrics and container metrics (container_metrics_1m)." },
  top_processes_days: { label: "Top-process history", unit: "days" },
  node_rollup_days: { label: "Hourly rollups", unit: "days", hint: "Hourly node rollups and container metrics (container_metrics_1h)." },
  app_logs_days: { label: "Log history", unit: "days", hint: "Counted from ingestion time." },
  cpu_quota_vcpu: {
    label: "CPU cap",
    unit: "vcpu",
    zero: "no cap",
    fractional: true,
    minNonZero: 0.1,
    hint: "Rounded up to whole CPUs: the engine is pinned to them and its thread pools match. Pools resize on restart.",
  },
  memory_budget_mb: { label: "Memory cap", unit: "mib", hint: "The engine's max_server_memory_usage; work that would exceed it fails." },
  search_max_concurrent: { label: "Concurrent searches", unit: "count" },
  search_timeout_secs: { label: "Search timeout", unit: "secs" },
  search_max_lookback_days: { label: "Max search lookback", unit: "days", zero: "no limit" },
  search_threads: { label: "Threads per search", unit: "count", zero: "engine default" },
  search_memory_mb: { label: "Memory per query", unit: "mib", zero: "engine default" },
  ingest_max_queued_mb: { label: "Max queued ingest", unit: "mib" },
  ingest_max_batch_rows: { label: "Max batch size", unit: "rows" },
  ingest_flush_interval_ms: { label: "Flush interval", unit: "ms" },
  min_free_disk_mb: { label: "Min free disk", unit: "mib", hint: "Log batches are refused below this." },
};

const UNIT_LABEL: Record<Unit, string> = { days: "days", vcpu: "vCPU", mib: "MiB", secs: "s", ms: "ms", rows: "rows", count: "" };

const metaOf = (name: string): Meta => META[name] ?? { label: name, unit: "count" };

const MIB = 1024 * 1024;

function fmtValue(name: string, v: number | null | undefined): string {
  if (v === null || v === undefined) return "—";
  const m = metaOf(name);
  if (v === 0 && m.zero) return `0 (${m.zero})`;
  switch (m.unit) {
    case "mib":
      return bytes(v * MIB);
    case "days":
      return `${v} ${v === 1 ? "day" : "days"}`;
    case "vcpu":
      return `${v} vCPU`;
    case "secs":
      return `${v} s`;
    case "ms":
      return `${v} ms`;
    case "rows":
      return `${v.toLocaleString()} rows`;
    default:
      return String(v);
  }
}

const SECTIONS = [
  { id: "retention", title: "Retention", names: ["node_metrics_days", "top_processes_days", "node_rollup_days", "app_logs_days"] },
  { id: "cpu", title: "CPU and memory", names: ["cpu_quota_vcpu", "memory_budget_mb"] },
  { id: "search", title: "Search", names: ["search_max_concurrent", "search_timeout_secs", "search_max_lookback_days", "search_threads", "search_memory_mb"] },
  { id: "ingest", title: "Ingestion", names: ["ingest_max_queued_mb", "ingest_max_batch_rows", "ingest_flush_interval_ms"] },
  { id: "disk", title: "Disk protection", names: ["min_free_disk_mb"] },
] as const satisfies readonly { id: string; title: string; names: readonly string[] }[];
const KNOWN = new Set<string>(SECTIONS.flatMap((s) => s.names));

// ------------------------------------------------------------------ drafts

/** Edited fields only: a string is the typed value, null resets to forge.toml. */
type Draft = Record<string, string | null>;

/** The value to send for a draft entry, or an error message. */
function parseDraft(s: AnalyticsSetting, raw: string | null): { value: number | null } | { error: string } {
  if (raw === null) return { value: null };
  const t = raw.trim();
  if (!t) return { error: "Enter a value" };
  const n = Number(t);
  const m = metaOf(s.setting);
  if (!Number.isFinite(n)) return { error: "Not a number" };
  if (!m.fractional && !Number.isInteger(n)) return { error: "Whole numbers only" };
  if (n < s.min || n > s.max) return { error: `Between ${s.min} and ${s.max}` };
  if (m.minNonZero && n > 0 && n < m.minNonZero) return { error: `0 or at least ${m.minNonZero}` };
  return { value: n };
}

// ------------------------------------------------------------------ page

export function AnalyticsSettings() {
  const q = useAnalyticsSettings();
  const caps = useMyCapabilities();
  const canEdit = caps.data?.analytics_configure === true;
  const denied = q.error instanceof ApiError && q.error.status === 403;
  const missing = q.error instanceof ApiError && q.error.status === 404;
  const s = q.data;
  return (
    <>
      <PageHeader eyebrow="Administration" title="Settings" description="Settings of this Forge server. Your own account and preferences are under Profile." />
      <SettingsTabs />
      {denied ? (
        <Panel title="Analytics">
          <p className="p-4 text-[13px] text-muted">Only platform administrators can see analytics settings.</p>
        </Panel>
      ) : missing ? (
        <Panel title="Analytics">
          <p className="p-4 text-[13px] text-muted">This Forge server does not offer analytics settings. Update forge-server to change them here.</p>
        </Panel>
      ) : q.isError ? (
        <Panel title="Analytics">
          <ErrorNote error={q.error} />
        </Panel>
      ) : !s ? (
        <Panel title="Analytics">
          <Skeleton rows={6} />
        </Panel>
      ) : (
        <AnalyticsBody s={s} canEdit={canEdit} />
      )}
    </>
  );
}

function AnalyticsBody({ s, canEdit }: { s: Settings; canEdit: boolean }) {
  return (
    <div className="space-y-4">
      <StatusPanel s={s} />
      {s.engine !== "off" && <WorkerPanel w={s.worker} />}
      {s.jobs.length > 0 && <JobsPanel jobs={s.jobs} />}
      {s.engine !== "off" && (s.pending_restart || s.restart_at) && <RestartBanner s={s} canEdit={canEdit} />}
      {s.engine === "off" ? (
        <Panel title="Analytics is off">
          <p className="p-4 text-[13px] text-muted">
            No analytics engine is configured. Set <code className="font-mono text-xs">[analytics] engine</code> in forge.toml and restart forge-server to keep node history and logs.
          </p>
        </Panel>
      ) : (
        <SettingsForm s={s} canEdit={canEdit} />
      )}
    </div>
  );
}

// ------------------------------------------------------------------ status and usage

const ENGINE_LABEL: Record<Settings["engine"], string> = {
  off: "Off",
  embedded: "Embedded ClickHouse",
  clickhouse: "ClickHouse",
  duckdb: "DuckDB (recommended for small installs)",
  postgres: "PostgreSQL",
  memory: "In memory (tests)",
};

function Cell({ label, children, title }: { label: string; children: ReactNode; title?: string }) {
  return (
    <div className="min-w-0 px-3 py-2" title={title}>
      <div className="text-[11px] text-muted">{label}</div>
      <div className="truncate font-mono text-[12px] tabular-nums">{children}</div>
    </div>
  );
}

function StatusPanel({ s }: { s: Settings }) {
  const u = s.usage;
  const minFree = s.settings.find((x) => x.setting === "min_free_disk_mb")?.effective;
  const lowDisk = u.free_disk_bytes !== null && minFree !== undefined && u.free_disk_bytes < minFree * MIB;
  const engineBudget = u.server_memory_limit_bytes === 0 ? "host-derived" : bytes(u.server_memory_limit_bytes);
  return (
    <Panel
      title={ENGINE_LABEL[s.engine]}
      description={
        s.managed
          ? "Run by Forge on this server."
          : s.engine === "duckdb"
            ? "Runs inside the analytics worker; one database file per day. Recommended for small installs."
            : s.engine === "off"
              ? "No analytics engine."
              : "External engine; Forge connects to it."
      }
      actions={
        <div className="flex flex-wrap items-center gap-1.5">
          <Badge tone={s.managed || s.engine === "duckdb" ? "accent" : "muted"}>
            {s.managed ? "managed" : s.engine === "duckdb" ? "in-process" : "external"}
          </Badge>
          {s.engine !== "off" && <Badge tone={s.ready ? "ok" : "bad"}>{s.ready ? "ready" : "not ready"}</Badge>}
        </div>
      }
    >
      {s.last_error && (
        <div className="flex items-start gap-2 border-b border-line bg-bad-soft px-4 py-2 text-xs text-bad">
          <AlertTriangle className="mt-0.5 size-3.5 shrink-0" />
          <span className="min-w-0 break-words select-text">{s.last_error}</span>
        </div>
      )}
      {s.engine !== "off" && (
        <div className="grid grid-cols-2 divide-line sm:grid-cols-3 lg:grid-cols-6">
          <Cell label="CPU" title="vCPU in use / CPUs the engine may run on">
            {u.cpu_vcpu === null ? "—" : u.cpu_vcpu.toFixed(2)} vCPU
            {u.allowed_cpus ? <span className="text-muted"> / {u.allowed_cpus}</span> : null}
          </Cell>
          <Cell label="Process memory" title="Resident memory of the engine process">
            {bytes(u.process_memory_bytes)}
          </Cell>
          <Cell label="Engine memory" title={`Memory the engine tracks / its server budget; per query: ${bytes(u.per_query_memory_bytes)}`}>
            {bytes(u.engine_memory_bytes)}
            {u.server_memory_limit_bytes !== null && <span className="text-muted"> / {engineBudget}</span>}
            {u.per_query_memory_bytes ? <span className="text-muted"> · {bytes(u.per_query_memory_bytes)}/q</span> : null}
          </Cell>
          <Cell label="Free disk" title={minFree !== undefined ? `Log batches are refused below ${fmtValue("min_free_disk_mb", minFree)}` : undefined}>
            <span className={lowDisk ? "text-bad" : ""}>{bytes(u.free_disk_bytes)}</span>
            {minFree !== undefined && <span className="text-muted"> ≥ {fmtValue("min_free_disk_mb", minFree)}</span>}
          </Cell>
          <Cell label="Ingest queue" title={`${s.queue.queued_requests} queued request(s)`}>
            {bytes(s.queue.queued_bytes)}
            {s.queue.oldest_ms !== null && <span className="text-muted"> · {duration(Date.now() - s.queue.oldest_ms)} old</span>}
          </Cell>
          <Cell label="Last apply" title={s.last_apply ? time(s.last_apply.at) : undefined}>
            {s.last_apply ? ago(s.last_apply.at) : "—"}
          </Cell>
        </div>
      )}
    </Panel>
  );
}

// ------------------------------------------------------------------ scheduled jobs

function JobsPanel({ jobs }: { jobs: Settings["jobs"] }) {
  return (
    <Panel title="Scheduled jobs" description="Hourly: rollups, retention by whole day files, compaction.">
      <div className="divide-y divide-line">
        {jobs.map((j) => (
          <div key={j.name} className="grid grid-cols-2 gap-2 px-4 py-2 text-xs sm:grid-cols-4">
            <span className="font-medium">{j.name}</span>
            <span className="text-muted" title={time(j.last_run)}>
              {ago(j.last_run)} · {j.duration_ms} ms
            </span>
            <span className="tabular-nums">{j.done.toLocaleString()} done</span>
            {j.error ? (
              <span className="min-w-0 break-words text-bad select-text">{j.error}</span>
            ) : (
              <Badge tone="ok">ok</Badge>
            )}
          </div>
        ))}
      </div>
    </Panel>
  );
}

// ------------------------------------------------------------------ worker process

const WORKER_TONE: Record<Settings["worker"]["status"], "ok" | "warn" | "bad" | "muted"> = {
  off: "muted",
  starting: "warn",
  running: "ok",
  restarting: "warn",
  incompatible: "bad",
  stopped: "muted",
};

const RESTART_REASON: Record<NonNullable<Settings["worker"]["last_restart_reason"]>, string> = {
  exited: "it exited",
  unresponsive: "it stopped answering",
  memory_limit: "memory limit",
  start_failed: "it failed to start",
};

/** The forge-analytics-worker process: forge-server restarts it when it exits, hangs or passes its memory limit. */
function WorkerPanel({ w }: { w: Settings["worker"] }) {
  const overLimit = w.rss_bytes !== null && w.memory_limit_bytes !== null && w.rss_bytes > w.memory_limit_bytes * 0.9;
  return (
    <Panel
      title="Analytics worker"
      description="The process that stores and searches analytics. Forge restarts it when it exits, stops answering or passes its memory limit."
      actions={<Badge tone={WORKER_TONE[w.status]}>{w.status.replace("_", " ")}</Badge>}
    >
      {w.last_error && w.status !== "running" && (
        <div className="flex items-start gap-2 border-b border-line bg-bad-soft px-4 py-2 text-xs text-bad">
          <AlertTriangle className="mt-0.5 size-3.5 shrink-0" />
          <span className="min-w-0 break-words select-text">{w.last_error}</span>
        </div>
      )}
      <div className="grid grid-cols-2 divide-line sm:grid-cols-4">
        <Cell label="Process" title={w.started_at ? `Started ${time(w.started_at)}` : undefined}>
          {w.pid === null ? "—" : `pid ${w.pid}`}
          {w.started_at !== null && <span className="text-muted"> · up {duration(Date.now() - w.started_at)}</span>}
        </Cell>
        <Cell label="Memory" title="Resident memory of the worker / worker_memory_limit_mb">
          <span className={overLimit ? "text-warn" : ""}>{bytes(w.rss_bytes)}</span>
          <span className="text-muted"> / {w.memory_limit_bytes === null ? "no limit" : bytes(w.memory_limit_bytes)}</span>
        </Cell>
        <Cell label="Restarts" title="Since forge-server started">
          {w.restarts}
        </Cell>
        <Cell label="Last restart" title={w.last_restart_at ? time(w.last_restart_at) : undefined}>
          {w.last_restart_at === null ? "—" : ago(w.last_restart_at)}
          {w.last_restart_reason && <span className="text-muted"> · {RESTART_REASON[w.last_restart_reason]}</span>}
        </Cell>
      </div>
    </Panel>
  );
}

// ------------------------------------------------------------------ restart

function RestartBanner({ s, canEdit }: { s: Settings; canEdit: boolean }) {
  const restart = useRestartAnalytics();
  const cancel = useCancelAnalyticsRestart();
  const toast = useToast();
  const [confirmNow, setConfirmNow] = useState(false);
  const [schedule, setSchedule] = useState(false);
  const why = !s.managed ? "Forge does not run this engine; restart it on its host." : !canEdit ? "Needs the analytics.configure permission." : undefined;
  return (
    <div className="flex flex-wrap items-center gap-x-3 gap-y-2 rounded-lg border border-warn/40 bg-warn-soft px-4 py-2 text-[13px]">
      <RotateCw className="size-3.5 shrink-0 text-warn" />
      <div className="min-w-0 flex-1">
        <span className="font-medium">{s.pending_restart ? "Pending analytics restart" : "Analytics restart scheduled"}</span>
        <span className="text-muted">
          {s.restart_at ? (
            <>
              {" "}
              · scheduled <span title={time(s.restart_at)}>{ago(s.restart_at)}</span> ({time(s.restart_at)})
            </>
          ) : (
            " · some changes take effect after the engine restarts"
          )}
          . Forge stays available; ingest is queued and history is briefly unavailable.
        </span>
      </div>
      {canEdit && s.managed ? (
        <div className="flex shrink-0 gap-2">
          {s.restart_at && (
            <Button
              onClick={() =>
                cancel.mutate(undefined, {
                  onSuccess: () => toast("Scheduled restart cancelled"),
                  onError: (e) => toast(errorText(e), "error"),
                })
              }
              disabled={cancel.isPending}
            >
              {cancel.isPending && <Loader2 className="size-4 animate-spin" />}
              Cancel
            </Button>
          )}
          <Button onClick={() => setSchedule(true)}>Schedule…</Button>
          <Button variant="primary" onClick={() => setConfirmNow(true)}>
            Restart now
          </Button>
        </div>
      ) : (
        <span className="text-xs text-muted">{why}</span>
      )}
      {confirmNow && (
        <Confirm
          open
          onOpenChange={setConfirmNow}
          title="Restart the analytics engine now?"
          body={
            <ul className="list-disc space-y-1 pl-5 text-[13px]">
              <li>Forge stops and starts its analytics engine and applies the pending settings.</li>
              <li>Node history, log search and charts are unavailable for a short time.</li>
              <li>Incoming metrics and logs are queued and written after the restart.</li>
              <li>The rest of Forge stays available.</li>
            </ul>
          }
          confirmLabel="Restart now"
          busy={restart.isPending}
          onConfirm={() =>
            restart.mutate(undefined, {
              onSuccess: () => {
                setConfirmNow(false);
                toast("Analytics restart started");
              },
              onError: (e) => toast(errorText(e), "error"),
            })
          }
        />
      )}
      {schedule && <ScheduleRestart onClose={() => setSchedule(false)} />}
    </div>
  );
}

const WEEK = 7 * 24 * 3600 * 1000;

/** `datetime-local` value in local time. */
function localInput(ms: number): string {
  const d = new Date(ms);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}T${p(d.getHours())}:${p(d.getMinutes())}`;
}

function ScheduleRestart({ onClose }: { onClose: () => void }) {
  const restart = useRestartAnalytics();
  const toast = useToast();
  const [now] = useState(() => Date.now());
  const [value, setValue] = useState(() => localInput(now + 3600_000));
  const at = value ? new Date(value).getTime() : NaN;
  const error = !Number.isFinite(at) ? "Pick a date and time" : at <= now ? "Pick a time in the future" : at > now + WEEK ? "At most 7 days ahead" : undefined;
  return (
    <Modal open onOpenChange={(o) => !o && onClose()} title="Schedule the analytics restart" description="Pick a quiet time. Forge stays available; history and log search pause briefly.">
      <form
        className="space-y-3"
        onSubmit={(e) => {
          e.preventDefault();
          if (error) return;
          restart.mutate(at, {
            onSuccess: (r) => {
              toast(`Analytics restart scheduled for ${time(r?.at ?? at)}`);
              onClose();
            },
            onError: (e) => toast(errorText(e), "error"),
          });
        }}
      >
        <label className="block text-xs text-muted">
          Restart at (local time)
          <Input type="datetime-local" className="mt-1" value={value} min={localInput(now)} max={localInput(now + WEEK)} onChange={(e) => setValue(e.target.value)} />
        </label>
        {error && <p className="text-xs text-bad">{error}</p>}
        <div className="flex justify-end gap-2">
          <Button type="button" onClick={onClose}>
            Cancel
          </Button>
          <Button type="submit" variant="primary" disabled={!!error || restart.isPending}>
            {restart.isPending && <Loader2 className="size-4 animate-spin" />}
            Schedule
          </Button>
        </div>
      </form>
    </Modal>
  );
}

// ------------------------------------------------------------------ settings form

function SettingsForm({ s, canEdit }: { s: Settings; canEdit: boolean }) {
  const [draft, setDraft] = useState<Draft>({});
  const [advanced, setAdvanced] = useState(false);
  const [pendingConfirm, setPendingConfirm] = useState<{ body: AnalyticsSettingsUpdate; asks: AnalyticsConfirmation[] } | null>(null);
  const [report, setReport] = useState<ApplyReport | null>(null);
  const update = useUpdateAnalyticsSettings();
  const toast = useToast();

  const byName = new Map(s.settings.map((x) => [x.setting, x]));
  const parsed = Object.entries(draft).flatMap(([name, raw]) => {
    const st = byName.get(name);
    return st ? [{ st, raw, r: parseDraft(st, raw) }] : [];
  });
  const invalid = parsed.some((p) => "error" in p.r);
  const changes: AnalyticsSettingsUpdate["settings"] = {};
  for (const p of parsed) if ("value" in p.r) changes[p.st.setting] = p.r.value;
  const count = Object.keys(changes).length;

  const send = (body: AnalyticsSettingsUpdate) =>
    update.mutate(body, {
      onSuccess: (r) => {
        setPendingConfirm(null);
        setDraft({});
        setReport(r);
        const failed = r.results.filter((x) => x.status === "failed").length;
        if (failed) toast(`Saved with ${failed} failed setting(s); see the result below`, "error");
        else toast("Analytics settings saved");
      },
      onError: (e) => {
        const asks = confirmationsOf(e);
        if (asks.length) setPendingConfirm({ body, asks });
        else {
          setPendingConfirm(null);
          toast(errorText(e), "error");
        }
      },
    });

  const shownReport = report && (!s.last_apply || report.at >= s.last_apply.at) ? report : s.last_apply;
  const unknown = s.settings.filter((x) => !KNOWN.has(x.setting));
  const sectionRows = (names: readonly string[]) => names.flatMap((n) => (byName.has(n) ? [byName.get(n) as AnalyticsSetting] : []));
  const rowProps = (st: AnalyticsSetting) => ({
    s: st,
    canEdit,
    raw: st.setting in draft ? draft[st.setting] : undefined,
    onChange: (v: string | null | undefined) =>
      setDraft((d) => {
        const next = { ...d };
        if (v === undefined) delete next[st.setting];
        else next[st.setting] = v;
        return next;
      }),
  });

  return (
    <form
      className="space-y-4"
      onSubmit={(e) => {
        e.preventDefault();
        if (!invalid && count) send({ settings: changes });
      }}
    >
      {!canEdit && <p className="text-xs text-muted">Read-only. Changing analytics settings needs the analytics.configure permission.</p>}

      <Section title="Retention" description="How long history is kept. Shortening it deletes older data.">
        {sectionRows(SECTIONS[0].names).map((st) => (
          <SettingRow key={st.setting} {...rowProps(st)} />
        ))}
      </Section>

      {s.managed && (
        <Section title="CPU and memory" description="Limits of the analytics engine Forge runs. They never apply to Forge itself.">
          {s.process_limits_unavailable && (
            <div className="flex items-start gap-2 border-b border-line bg-warn-soft px-4 py-2 text-xs text-warn">
              <AlertTriangle className="mt-0.5 size-3.5 shrink-0" />
              <span className="min-w-0">The CPU cap is not in effect: {s.process_limits_unavailable}</span>
            </div>
          )}
          {sectionRows(SECTIONS[1].names).map((st) => (
            <SettingRow key={st.setting} {...rowProps(st)} />
          ))}
        </Section>
      )}

      <section className="rounded-lg border border-line bg-panel">
        <button
          type="button"
          onClick={() => setAdvanced((a) => !a)}
          aria-expanded={advanced}
          className="flex w-full items-center gap-2 rounded-lg px-4 py-2.5 text-left font-semibold focus-visible:outline-2 focus-visible:outline-accent"
        >
          {advanced ? <ChevronDown className="size-3.5" /> : <ChevronRight className="size-3.5" />}
          Advanced
          <span className="text-xs font-normal text-muted">Search, ingestion and disk protection</span>
        </button>
        {advanced && (
          <div className="border-t border-line">
            {SECTIONS.slice(2).map((sec) => (
              <div key={sec.id}>
                <h3 className="border-b border-line bg-panel-2 px-4 py-1 text-[11px] font-medium tracking-wide text-muted uppercase">{sec.title}</h3>
                {sectionRows(sec.names).map((st) => (
                  <SettingRow key={st.setting} {...rowProps(st)} />
                ))}
              </div>
            ))}
            {unknown.length > 0 && (
              <div>
                <h3 className="border-b border-line bg-panel-2 px-4 py-1 text-[11px] font-medium tracking-wide text-muted uppercase">Other</h3>
                {unknown.map((st) => (
                  <SettingRow key={st.setting} {...rowProps(st)} />
                ))}
              </div>
            )}
          </div>
        )}
      </section>

      {canEdit && (
        <UnsavedBar
          dirty={Object.keys(draft).length > 0}
          summary={invalid ? "Fix the highlighted values to save." : count ? <span className="text-muted">{count} change(s), applied together in one save.</span> : null}
          onDiscard={() => setDraft({})}
          saving={update.isPending}
          saveDisabled={!count || invalid}
        />
      )}

      {shownReport && <ReportPanel report={shownReport} />}

      {pendingConfirm && (
        <Confirm
          open
          onOpenChange={(o) => !o && setPendingConfirm(null)}
          title="Confirm the change"
          body={
            <ul className="list-disc space-y-1 pl-5 text-[13px]">
              {pendingConfirm.asks.map((a) => (
                <li key={a.confirm}>{a.message}</li>
              ))}
            </ul>
          }
          confirmLabel="Save anyway"
          busy={update.isPending}
          onConfirm={() =>
            send({
              ...pendingConfirm.body,
              confirm: { ...pendingConfirm.body.confirm, ...Object.fromEntries(pendingConfirm.asks.map((a) => [a.confirm, true])) },
            })
          }
        />
      )}
    </form>
  );
}

function Section({ title, description, children }: { title: string; description: string; children: ReactNode }) {
  return (
    <Panel title={title} description={description}>
      <div className="divide-y divide-line">{children}</div>
    </Panel>
  );
}

function SettingRow({ s, canEdit, raw, onChange }: { s: AnalyticsSetting; canEdit: boolean; raw: string | null | undefined; onChange: (v: string | null | undefined) => void }) {
  const m = metaOf(s.setting);
  const overridden = s.override_value !== null;
  const editable = canEdit && s.applies;
  const resetting = raw === null;
  const r = raw === undefined ? undefined : parseDraft(s, raw);
  const err = r && "error" in r ? r.error : undefined;
  const next = r && "value" in r ? r.value : undefined;
  const shortens = m.unit === "days" && s.group === "retention" && typeof next === "number" && next < s.effective;
  const resetShortens = resetting && s.group === "retention" && s.configured < s.effective;
  const id = `an-${s.setting}`;
  return (
    <div className="grid grid-cols-1 gap-x-4 gap-y-1 px-4 py-2 text-[13px] sm:grid-cols-[minmax(0,1fr)_auto]">
      <div className="min-w-0">
        <div className="flex flex-wrap items-center gap-x-2 gap-y-0.5">
          <label htmlFor={editable ? id : undefined} className="font-medium">
            {m.label}
          </label>
          <span className="font-mono text-[12px] tabular-nums">{fmtValue(s.setting, s.effective)}</span>
          {overridden && <Badge tone="accent">overridden</Badge>}
          {resetting && <Badge tone="warn">reset on save</Badge>}
        </div>
        <div className="flex flex-wrap gap-x-2 text-[11px] text-muted">
          <span className="font-mono tabular-nums" title="Value from forge.toml">
            forge.toml: {fmtValue(s.setting, s.configured)}
          </span>
          {m.hint && <span>{m.hint}</span>}
        </div>
        {err && <p className="text-[11px] text-bad">{err}</p>}
        {(shortens || resetShortens) && <p className="text-[11px] text-warn">Shorter than now: older data is deleted after saving.</p>}
      </div>
      {editable && (
        <div className="flex items-center gap-1.5 sm:justify-end">
          <Input
            id={id}
            type="number"
            inputMode={m.fractional ? "decimal" : "numeric"}
            step={m.fractional ? 0.1 : 1}
            min={s.min}
            max={s.max}
            aria-invalid={!!err}
            className={`w-28 font-mono text-[12px] tabular-nums ${err ? "border-bad" : ""}`}
            value={resetting ? String(s.configured) : (raw ?? String(s.effective))}
            disabled={resetting}
            onChange={(e) => onChange(e.target.value === String(s.effective) && !overridden ? undefined : e.target.value)}
          />
          <span className="w-9 text-[11px] text-muted">{UNIT_LABEL[m.unit]}</span>
          {raw !== undefined ? (
            <Button type="button" variant="ghost" className="px-1.5" title="Undo this edit" aria-label={`Undo ${m.label} edit`} onClick={() => onChange(undefined)}>
              <RotateCcw className="size-4" />
            </Button>
          ) : overridden ? (
            <Button type="button" variant="ghost" className="px-1.5 text-xs" title={`Reset to forge.toml (${fmtValue(s.setting, s.configured)})`} onClick={() => onChange(null)}>
              Reset
            </Button>
          ) : (
            <span className="w-[42px]" aria-hidden />
          )}
        </div>
      )}
    </div>
  );
}

// ------------------------------------------------------------------ apply report

const STATUS_TONE: Record<ApplyStatus, "ok" | "accent" | "warn" | "bad" | "muted"> = {
  applied: "ok",
  pending: "accent",
  pending_restart: "warn",
  failed: "bad",
};
const STATUS_LABEL: Record<ApplyStatus, string> = {
  applied: "applied",
  pending: "pending",
  pending_restart: "pending restart",
  failed: "failed",
};

function ReportPanel({ report }: { report: ApplyReport }) {
  const failed = report.results.filter((r) => r.status === "failed").length;
  return (
    <Panel
      title="Last apply"
      description={
        <span title={time(report.at)}>
          {ago(report.at)}
          {failed > 0 && ` · ${failed} of ${report.results.length} failed`}
        </span>
      }
    >
      {report.results.length === 0 ? (
        <p className="px-4 py-2 text-xs text-muted">No settings changed.</p>
      ) : (
        <ul className="divide-y divide-line">
          {report.results.map((r) => (
            <li key={r.setting} className="flex flex-wrap items-center gap-x-3 gap-y-0.5 px-4 py-1.5 text-[13px]">
              <span className="min-w-0 truncate" title={r.setting}>
                {metaOf(r.setting).label}
              </span>
              <Badge tone={STATUS_TONE[r.status]}>{STATUS_LABEL[r.status]}</Badge>
              <span className="font-mono text-[12px] tabular-nums text-muted">{fmtValue(r.setting, r.effective)}</span>
              {r.message && <span className={`min-w-0 basis-full text-xs break-words select-text sm:basis-auto ${r.status === "failed" ? "text-bad" : "text-muted"}`}>{r.message}</span>}
            </li>
          ))}
        </ul>
      )}
    </Panel>
  );
}
