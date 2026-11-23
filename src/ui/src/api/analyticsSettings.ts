// Analytics engine settings (issue #86): retention, the embedded engine's CPU cap and memory budget,
// search, ingestion and disk protection, with forge.toml as the fallback.
// Read: platform `view`; change: `analytics.configure` (may need step-up).
import { useQuery } from "@tanstack/react-query";

import { ApiError, api, get, post } from "./client";
import { useAction } from "./hooks";

export type AnalyticsEngine = "off" | "embedded" | "clickhouse" | "duckdb" | "postgres" | "memory";
export type AnalyticsSettingGroup = "retention" | "process" | "engine_memory" | "forge";

/** Every setting the server knows, in display order. */
export const ANALYTICS_SETTING_NAMES = [
  "node_metrics_days",
  "top_processes_days",
  "node_rollup_days",
  "app_logs_days",
  "cpu_quota_vcpu",
  "memory_budget_mb",
  "search_max_concurrent",
  "search_timeout_secs",
  "search_max_lookback_days",
  "search_threads",
  "search_memory_mb",
  "ingest_max_queued_mb",
  "ingest_max_batch_rows",
  "ingest_flush_interval_ms",
  "min_free_disk_mb",
] as const;
/** A newer server may send names this console does not know; they are shown generically. */
export type AnalyticsSettingName = (typeof ANALYTICS_SETTING_NAMES)[number] | (string & {});

export interface AnalyticsSetting {
  setting: AnalyticsSettingName;
  group: AnalyticsSettingGroup;
  /** Value from forge.toml. */
  configured: number;
  /** Runtime override; null when forge.toml applies. */
  override_value: number | null;
  effective: number;
  min: number;
  max: number;
  /** False: CPU and memory of an engine Forge does not run; the server refuses changes. */
  applies: boolean;
}

export interface AnalyticsUsage {
  cpu_vcpu: number | null;
  process_memory_bytes: number | null;
  engine_memory_bytes: number | null;
  /** 0 = derived from the host. */
  server_memory_limit_bytes: number | null;
  per_query_memory_bytes: number | null;
  /** CPUs the embedded engine may run on, read back from the kernel. */
  allowed_cpus: number | null;
  free_disk_bytes: number | null;
}

export interface AnalyticsQueue {
  queued_bytes: number;
  queued_requests: number;
  oldest_ms: number | null;
}

export type WorkerStatus = "off" | "starting" | "running" | "restarting" | "incompatible" | "stopped";
export type WorkerRestartReason = "exited" | "unresponsive" | "memory_limit" | "start_failed";

/** The analytics worker process that forge-server supervises. */
export interface AnalyticsWorker {
  status: WorkerStatus;
  pid: number | null;
  rss_bytes: number | null;
  /** worker_memory_limit_mb in bytes; null: no limit. */
  memory_limit_bytes: number | null;
  restarts: number;
  started_at: number | null;
  last_restart_at: number | null;
  last_restart_reason: WorkerRestartReason | null;
  last_error: string | null;
}

export type ApplyStatus = "applied" | "pending" | "pending_restart" | "failed";

export interface ApplyResult {
  setting: AnalyticsSettingName;
  status: ApplyStatus;
  effective: number | null;
  message: string | null;
}

export interface ApplyReport {
  at: number;
  results: ApplyResult[];
}

export interface AnalyticsSettings {
  engine: AnalyticsEngine;
  /** The engine is run by Forge. */
  managed: boolean;
  ready: boolean;
  last_error: string | null;
  settings: AnalyticsSetting[];
  usage: AnalyticsUsage;
  queue: AnalyticsQueue;
  /** Why the CPU cap is not in effect; null when it is or none is set. */
  process_limits_unavailable: string | null;
  pending_restart: boolean;
  restart_at: number | null;
  last_apply: ApplyReport | null;
  worker: AnalyticsWorker;
  /** Scheduled store jobs (DuckDB: rollups, retention, compaction); empty for other engines. */
  jobs: AnalyticsJob[];
}

/** The last run of one scheduled analytics job. */
export interface AnalyticsJob {
  name: string;
  last_run: number;
  duration_ms: number;
  error: string | null;
  done: number;
}

export type AnalyticsConfirmFlag = "shorten_retention";

export interface AnalyticsConfirmation {
  confirm: AnalyticsConfirmFlag;
  message: string;
}

export interface AnalyticsSettingsUpdate {
  settings: Partial<Record<AnalyticsSettingName, number | null>>;
  confirm?: Partial<Record<AnalyticsConfirmFlag, boolean>>;
}

export const analyticsSettingsKeys = {
  all: ["analytics-settings"] as const,
  settings: () => [...analyticsSettingsKeys.all, "settings"] as const,
};

const num = (v: unknown): number | null => (typeof v === "number" && Number.isFinite(v) ? v : null);
const str = (v: unknown): string | null => (typeof v === "string" && v ? v : null);
const obj = (v: unknown): Record<string, unknown> => (v && typeof v === "object" ? (v as Record<string, unknown>) : {});

const STATUSES: ApplyStatus[] = ["applied", "pending", "pending_restart", "failed"];

function toReport(x: unknown): ApplyReport | null {
  const o = obj(x);
  const at = num(o.at);
  if (at === null) return null;
  const results = Array.isArray(o.results)
    ? o.results.flatMap((r): ApplyResult[] => {
        const ro = obj(r);
        const setting = str(ro.setting);
        if (!setting) return [];
        const status = STATUSES.includes(ro.status as ApplyStatus) ? (ro.status as ApplyStatus) : "failed";
        return [{ setting, status, effective: num(ro.effective), message: str(ro.message) }];
      })
    : [];
  return { at, results };
}

const WORKER_STATUSES: WorkerStatus[] = ["off", "starting", "running", "restarting", "incompatible", "stopped"];
const RESTART_REASONS: WorkerRestartReason[] = ["exited", "unresponsive", "memory_limit", "start_failed"];

function toWorker(x: unknown): AnalyticsWorker {
  const o = obj(x);
  return {
    status: WORKER_STATUSES.includes(o.status as WorkerStatus) ? (o.status as WorkerStatus) : "off",
    pid: num(o.pid),
    rss_bytes: num(o.rss_bytes),
    memory_limit_bytes: num(o.memory_limit_bytes),
    restarts: num(o.restarts) ?? 0,
    started_at: num(o.started_at),
    last_restart_at: num(o.last_restart_at),
    last_restart_reason: RESTART_REASONS.includes(o.last_restart_reason as WorkerRestartReason)
      ? (o.last_restart_reason as WorkerRestartReason)
      : null,
    last_error: str(o.last_error),
  };
}

/** Defaults every field so an older server or a partial answer never breaks the page. */
function toSettings(x: unknown): AnalyticsSettings {
  const o = obj(x);
  const u = obj(o.usage);
  const q = obj(o.queue);
  const engines: AnalyticsEngine[] = ["off", "embedded", "clickhouse", "duckdb", "postgres", "memory"];
  const groups: AnalyticsSettingGroup[] = ["retention", "process", "engine_memory", "forge"];
  const settings = Array.isArray(o.settings)
    ? o.settings.flatMap((s): AnalyticsSetting[] => {
        const so = obj(s);
        const setting = str(so.setting);
        const effective = num(so.effective);
        if (!setting || effective === null) return [];
        return [
          {
            setting,
            group: groups.includes(so.group as AnalyticsSettingGroup) ? (so.group as AnalyticsSettingGroup) : "forge",
            configured: num(so.configured) ?? effective,
            override_value: num(so.override_value),
            effective,
            min: num(so.min) ?? 0,
            max: num(so.max) ?? Number.MAX_SAFE_INTEGER,
            applies: so.applies !== false,
          },
        ];
      })
    : [];
  return {
    engine: engines.includes(o.engine as AnalyticsEngine) ? (o.engine as AnalyticsEngine) : "off",
    managed: o.managed === true,
    ready: o.ready === true,
    last_error: str(o.last_error),
    settings,
    usage: {
      cpu_vcpu: num(u.cpu_vcpu),
      process_memory_bytes: num(u.process_memory_bytes),
      engine_memory_bytes: num(u.engine_memory_bytes),
      server_memory_limit_bytes: num(u.server_memory_limit_bytes),
      per_query_memory_bytes: num(u.per_query_memory_bytes),
      allowed_cpus: num(u.allowed_cpus),
      free_disk_bytes: num(u.free_disk_bytes),
    },
    queue: { queued_bytes: num(q.queued_bytes) ?? 0, queued_requests: num(q.queued_requests) ?? 0, oldest_ms: num(q.oldest_ms) },
    process_limits_unavailable: str(o.process_limits_unavailable),
    pending_restart: o.pending_restart === true,
    restart_at: num(o.restart_at),
    last_apply: toReport(o.last_apply),
    worker: toWorker(o.worker),
    jobs: Array.isArray(o.jobs)
      ? o.jobs.flatMap((j): AnalyticsJob[] => {
          const jo = obj(j);
          const name = str(jo.name);
          const lastRun = num(jo.last_run);
          if (!name || lastRun === null) return [];
          return [{ name, last_run: lastRun, duration_ms: num(jo.duration_ms) ?? 0, error: str(jo.error), done: num(jo.done) ?? 0 }];
        })
      : [],
  };
}

/** The confirmations a 409 `confirmation_required` asks for; empty for any other error. */
export function confirmationsOf(e: unknown): AnalyticsConfirmation[] {
  if (!(e instanceof ApiError) || e.status !== 409) return [];
  const list = obj(e.body).confirm;
  if (!Array.isArray(list)) return [];
  return list.flatMap((c): AnalyticsConfirmation[] => {
    const co = obj(c);
    const flag = co.confirm;
    if (flag !== "shorten_retention") return [];
    return [{ confirm: flag, message: str(co.message) ?? flag }];
  });
}

/** Settings, usage and the last apply. Usage is sampled every 10 s server-side. */
export const useAnalyticsSettings = () =>
  useQuery({
    queryKey: analyticsSettingsKeys.settings(),
    queryFn: async () => toSettings(await get<unknown>("/api/v1/analytics/settings")),
    retry: (n, e) => !(e instanceof ApiError && (e.status === 403 || e.status === 404)) && n < 2,
    refetchInterval: 10_000,
    staleTime: 5_000,
  });

/** One PUT with the changed fields; null resets a field to forge.toml. 409 asks for confirmations. */
export const useUpdateAnalyticsSettings = () =>
  useAction(
    async (body: AnalyticsSettingsUpdate) => toReport(await api<unknown>("PUT", "/api/v1/analytics/settings", body)) ?? { at: Date.now(), results: [] },
    [analyticsSettingsKeys.all],
  );

/** Restart the managed engine now (no `at`) or at an epoch-ms time up to 7 days ahead. */
export const useRestartAnalytics = () =>
  useAction((at: number | undefined) => post<{ at: number }>("/api/v1/analytics/restart", at === undefined ? {} : { at }), [analyticsSettingsKeys.all]);

/** Cancel a scheduled restart (404 when none). */
export const useCancelAnalyticsRestart = () => useAction(() => api<void>("DELETE", "/api/v1/analytics/restart"), [analyticsSettingsKeys.all]);
