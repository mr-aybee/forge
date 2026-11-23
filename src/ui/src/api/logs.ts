// Log events from the analytics database: service events (Seq/CLEF ingest)
// and container stdout/stderr collected by the nodes.

import { type InfiniteData, useInfiniteQuery, useQuery, useQueryClient } from "@tanstack/react-query";

import { analyticsUnavailable } from "./analytics";
import { api, get, post } from "./client";
import { useAction } from "./hooks";

const enc = encodeURIComponent;

export const LOG_LEVELS = ["Verbose", "Debug", "Information", "Warning", "Error", "Fatal"] as const;
export type LogLevel = (typeof LOG_LEVELS)[number];

export const LOG_SOURCES = ["ingest", "container"] as const;
/** `ingest`: sent by a service; `container`: collected from Docker on the node. */
export type LogSource = (typeof LOG_SOURCES)[number];
export type LogStream = "stdout" | "stderr";

export interface LogEvent {
  id: string;
  /** `null` for a container line that belongs to no service. */
  service_id: string | null;
  /** Service name; `null` with `service_id`. */
  service: string | null;
  source: LogSource;
  /** Container lines: short id, name, stack and stream; `null` otherwise. */
  container_id: string | null;
  container_name: string | null;
  stack: string | null;
  stream: LogStream | null;
  ts: number;
  node_ts: number | null;
  ingest_ts: number;
  level: LogLevel;
  message: string;
  message_template: string;
  exception: string | null;
  trace_id: string | null;
  span_id: string | null;
  /** 8 hex chars. */
  event_type: string;
  properties: Record<string, unknown>;
  /** Node that sent the event; null when Forge could not tell (older events, or no MachineName/Node/Host property). */
  server_id: string | null;
  /** That node's display name; null when unknown. */
  node: string | null;
}

export interface LogPage {
  events: LogEvent[];
  /** Cursor for older events (`before`); null at the end of the log. */
  next: string | null;
  /** Cursor of the newest event in this page (`after` for the live tail). */
  newest: string | null;
  /** Set on pages prepended by the live tail: when they arrived (client clock). */
  arrived?: number;
}

export interface LogHistogram {
  from: number;
  to: number;
  step_ms: number;
  buckets: { t: number; counts: Partial<Record<LogLevel, number>> }[];
}

export interface LogKey {
  id: string;
  service_id: string;
  name: string;
  prefix: string;
  created_at: number;
  last_used_at: number | null;
}

export interface CreatedLogKey {
  key: LogKey;
  /** Shown once; never stored by the console. */
  token: string;
}

/** What the log console filters on (also the `/logs` URL search). */
export interface LogFilter {
  /** Whole services (every node, including events with no known node). */
  services?: string[];
  /** Single nodes of services, as `serviceId:serverId`. OR'ed with `services`. */
  instances?: string[];
  /** Only events from these nodes. */
  nodes?: string[];
  /** A container name, or a prefix of its id. */
  container?: string;
  /** Stack name (`<environment>/<stack>`, or a compose project). */
  stack?: string;
  source?: LogSource;
  levels?: LogLevel[];
  q?: string;
}

/** Order-independent key parts, so the same selection in another order hits the same cache. */
function filterKey(f: LogFilter) {
  const sorted = (v?: string[]) => [...(v ?? [])].sort().join(",");
  return [sorted(f.services), sorted(f.instances), sorted(f.nodes), f.container ?? "", f.stack ?? "", f.source ?? "", (f.levels ?? []).join(","), f.q ?? ""] as const;
}

export const logKeys = {
  all: ["logs"] as const,
  events: (f: LogFilter) => ["logs", "events", ...filterKey(f)] as const,
  tail: (f: LogFilter) => ["logs", "tail", ...filterKey(f)] as const,
  histogram: (f: LogFilter) => ["logs", "histogram", ...filterKey(f)] as const,
  ingestKeys: (app: string) => ["logs", "keys", app] as const,
};

function params(f: LogFilter, extra: Record<string, string | number | undefined> = {}): string {
  const p = new URLSearchParams();
  if (f.services?.length) p.set("services", f.services.join(","));
  if (f.instances?.length) p.set("instances", f.instances.join(","));
  if (f.nodes?.length) p.set("node", f.nodes.join(","));
  if (f.container) p.set("container", f.container);
  if (f.stack) p.set("stack", f.stack);
  if (f.source) p.set("source", f.source);
  if (f.levels?.length) p.set("levels", f.levels.join(","));
  if (f.q?.trim()) p.set("q", f.q.trim());
  for (const [k, v] of Object.entries(extra)) if (v !== undefined && v !== "") p.set(k, String(v));
  const s = p.toString();
  return s ? `?${s}` : "";
}

const optStr = (v: unknown): string | null => (typeof v === "string" && v ? v : null);
const isLevel = (v: unknown): v is LogLevel => typeof v === "string" && (LOG_LEVELS as readonly string[]).includes(v);

/** Defaults every field so an older or partial response cannot break the page. */
function normEvent(e: Partial<LogEvent>): LogEvent {
  return {
    id: String(e.id ?? ""),
    service_id: optStr(e.service_id),
    service: optStr(e.service),
    source: e.source === "container" ? "container" : "ingest",
    container_id: optStr(e.container_id),
    container_name: optStr(e.container_name),
    stack: optStr(e.stack),
    stream: e.stream === "stdout" || e.stream === "stderr" ? e.stream : null,
    ts: typeof e.ts === "number" ? e.ts : (e.ingest_ts ?? 0),
    node_ts: e.node_ts ?? null,
    ingest_ts: e.ingest_ts ?? 0,
    level: isLevel(e.level) ? e.level : "Information",
    message: e.message ?? e.message_template ?? "",
    message_template: e.message_template ?? "",
    exception: e.exception ?? null,
    trace_id: e.trace_id ?? null,
    span_id: e.span_id ?? null,
    event_type: e.event_type ?? "",
    properties: e.properties && typeof e.properties === "object" ? e.properties : {},
    server_id: typeof e.server_id === "string" && e.server_id ? e.server_id : null,
    node: typeof e.node === "string" && e.node ? e.node : null,
  };
}

async function fetchPage(f: LogFilter, cursor: { before?: string; after?: string }, limit = 100): Promise<LogPage> {
  const r = await get<Partial<LogPage>>(`/api/v1/logs${params(f, { ...cursor, limit })}`);
  return {
    events: Array.isArray(r?.events) ? r.events.map(normEvent) : [],
    next: r?.next ?? null,
    newest: r?.newest ?? null,
  };
}

const retry = (n: number, e: unknown) => !analyticsUnavailable(e) && n < 2;

/** The event stream, newest first; `fetchNextPage` loads older events. */
export function useLogEvents(f: LogFilter) {
  return useInfiniteQuery({
    queryKey: logKeys.events(f),
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam }) => fetchPage(f, { before: pageParam }),
    getNextPageParam: (last) => last.next ?? undefined,
    // The live tail prepends pages; a background refetch would reset them.
    staleTime: Infinity,
    refetchOnWindowFocus: false,
    retry,
  });
}

/**
 * Live tail: while `on`, polls every 2 s for events newer than the newest one
 * shown and prepends them to the stream's cache (newest stays on top). The
 * polling stops on unmount or when `on` turns false.
 */
export function useLogTail(f: LogFilter, on: boolean) {
  const qc = useQueryClient();
  return useQuery({
    queryKey: logKeys.tail(f),
    enabled: on,
    refetchInterval: on ? 2000 : false,
    refetchIntervalInBackground: false,
    gcTime: 0,
    retry,
    queryFn: async () => {
      const key = logKeys.events(f);
      const cur = qc.getQueryData<InfiniteData<LogPage, string | undefined>>(key);
      const newest = cur?.pages.find((p) => p.newest)?.newest ?? undefined;
      const page = await fetchPage(f, newest ? { after: newest } : {}, 500);
      if (!page.events.length) return Date.now();
      qc.setQueryData<InfiniteData<LogPage, string | undefined>>(key, (old) => {
        if (!old) return old;
        const seen = new Set(old.pages.flatMap((p) => p.events.map((e) => e.id)));
        const events = page.events.filter((e) => !seen.has(e.id));
        if (!events.length) return old;
        // `next: null` keeps paging driven by the last (oldest) page.
        return { pages: [{ events, next: null, newest: page.newest, arrived: Date.now() }, ...old.pages], pageParams: [undefined, ...old.pageParams] };
      });
      return Date.now();
    },
  });
}

/** Counts per level over the window (default the last 24 hours). */
export function useLogHistogram(f: LogFilter, live = false) {
  return useQuery({
    queryKey: logKeys.histogram(f),
    refetchInterval: live ? 10_000 : 60_000,
    retry,
    queryFn: async (): Promise<LogHistogram> => {
      const r = await get<Partial<LogHistogram>>(`/api/v1/logs/histogram${params(f)}`);
      return {
        from: r?.from ?? 0,
        to: r?.to ?? 0,
        step_ms: r?.step_ms ?? 0,
        buckets: Array.isArray(r?.buckets) ? r.buckets.map((b) => ({ t: b?.t ?? 0, counts: b?.counts ?? {} })) : [],
      };
    },
  });
}

export function useLogIngestKeys(app: string) {
  return useQuery({
    queryKey: logKeys.ingestKeys(app),
    retry,
    queryFn: async () => {
      const r = await get<LogKey[]>(`/api/v1/services/${enc(app)}/log-keys`);
      return Array.isArray(r) ? r : [];
    },
  });
}

export const useCreateLogIngestKey = (app: string) =>
  useAction((name: string) => post<CreatedLogKey>(`/api/v1/services/${enc(app)}/log-keys`, { name }), [logKeys.ingestKeys(app)]);

export const useRevokeLogIngestKey = (app: string) =>
  useAction((id: string) => api<void>("DELETE", `/api/v1/log-keys/${enc(id)}`), [logKeys.ingestKeys(app)]);

/** Parses the `levels` URL value; unknown names are dropped. */
export const parseLevels = (v: unknown): LogLevel[] | undefined => {
  if (typeof v !== "string" || !v) return undefined;
  const out = v.split(",").filter(isLevel);
  return out.length ? out : undefined;
};

/** Parses a comma list from the URL (`services`, `nodes`); empty items are dropped. */
export const parseList = (v: unknown): string[] | undefined => {
  if (typeof v !== "string" || !v) return undefined;
  const out = [...new Set(v.split(",").map((x) => x.trim()).filter(Boolean))];
  return out.length ? out : undefined;
};
