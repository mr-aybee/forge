// The audit log: filtered, cursor-paged entries, a histogram by outcome,
// the distinct action types and chain verification.

import { useInfiniteQuery, useMutation, useQuery } from "@tanstack/react-query";

import { ApiError, get } from "./client";

export const AUDIT_OUTCOMES = ["success", "denied", "failure"] as const;
export type AuditOutcome = (typeof AUDIT_OUTCOMES)[number];

export interface AuditEntry {
  seq: number;
  id: string;
  at: number;
  /** `user:<id>`, `agent:<server id>`, `token:<id>`, `system:forge`… */
  actor: string;
  action: string;
  /** `<kind>:<id>`, such as `server:<id>` or `platform:root`. */
  resource: string;
  workspace_id: string | null;
  outcome: AuditOutcome;
  details: Record<string, unknown>;
  prev_hash: string;
  hash: string;
  /** Ed25519 signature over `hash`; empty when an older server leaves it out. */
  signature: string;
}

export interface AuditPage {
  entries: AuditEntry[];
  /** Pass as `before` for the next, older page; null on the last page. */
  next_cursor: number | null;
  /** Matching entries across all pages; first page only. */
  total?: number;
}

export interface AuditActionCount {
  action: string;
  count: number;
}

export interface AuditHistogramBucket {
  t: number;
  success: number;
  denied: number;
  failure: number;
}

export interface AuditHistogram {
  from: number;
  to: number;
  step_ms: number;
  buckets: AuditHistogramBucket[];
}

export interface VerifyReport {
  ok: boolean;
  entries_checked: number;
  head_hash: string | null;
  /** First failing sequence number and why. */
  first_invalid: [number, string] | null;
}

/** Relative windows; the start is computed when the request runs, so "24 h" always means the last 24 hours. */
export const AUDIT_RANGES = [
  { key: "1h", label: "Last hour", ms: 3_600_000 },
  { key: "24h", label: "24 h", ms: 86_400_000 },
  { key: "7d", label: "7 days", ms: 7 * 86_400_000 },
  { key: "30d", label: "30 days", ms: 30 * 86_400_000 },
] as const;

/** What the audit page filters on (also its URL search). */
export interface AuditFilter {
  /** Free text over action, actor, resource and details. */
  q?: string;
  /** Action prefixes; an entry matches any of them. */
  actions?: string[];
  /** Substring of the actor: `user:<id>`, or a type prefix such as `agent:`. */
  actor?: string;
  outcome?: AuditOutcome;
  /** A key of `AUDIT_RANGES`, or undefined for a custom window (`since`/`until`) or all time. */
  range?: string;
  since?: number;
  until?: number;
}

export const auditKeys = {
  all: ["audit"] as const,
  entries: (f: AuditFilter) => ["audit", "entries", ...filterKey(f)] as const,
  histogram: (f: AuditFilter, buckets: number) => ["audit", "histogram", buckets, ...filterKey(f)] as const,
  actions: ["audit", "actions"] as const,
};

function filterKey(f: AuditFilter) {
  return [f.q?.trim() ?? "", [...(f.actions ?? [])].sort().join(","), f.actor ?? "", f.outcome ?? "", f.range ?? "", f.since ?? 0, f.until ?? 0] as const;
}

/** Query string for the API; relative ranges resolve against the current time. */
function params(f: AuditFilter, extra: Record<string, string | number | undefined> = {}): string {
  const p = new URLSearchParams();
  const preset = AUDIT_RANGES.find((r) => r.key === f.range);
  if (preset) p.set("since", String(Date.now() - preset.ms));
  else {
    if (f.since) p.set("since", String(f.since));
    if (f.until) p.set("until", String(f.until));
  }
  if (f.q?.trim()) p.set("q", f.q.trim());
  if (f.actions?.length) p.set("action", f.actions.join(","));
  if (f.actor) p.set("actor", f.actor);
  if (f.outcome) p.set("outcome", f.outcome);
  for (const [k, v] of Object.entries(extra)) if (v !== undefined && v !== "") p.set(k, String(v));
  return p.toString();
}

const num = (v: unknown, d = 0) => (typeof v === "number" && Number.isFinite(v) ? v : d);
const str = (v: unknown) => (typeof v === "string" ? v : "");
const isOutcome = (v: unknown): v is AuditOutcome => typeof v === "string" && (AUDIT_OUTCOMES as readonly string[]).includes(v);

/** Defaults every field so a partial or older response cannot break the page. */
function normEntry(e: Partial<AuditEntry> | null | undefined): AuditEntry {
  return {
    seq: num(e?.seq),
    id: str(e?.id),
    at: num(e?.at),
    actor: str(e?.actor),
    action: str(e?.action),
    resource: str(e?.resource),
    workspace_id: typeof e?.workspace_id === "string" ? e.workspace_id : null,
    outcome: isOutcome(e?.outcome) ? e.outcome : "success",
    details: e?.details && typeof e.details === "object" && !Array.isArray(e.details) ? e.details : {},
    prev_hash: str(e?.prev_hash),
    hash: str(e?.hash),
    signature: str(e?.signature),
  };
}

/** Most entries per request; the server caps `/audit/page` at 200. */
export const AUDIT_PAGE_SIZE = 100;

/** Matching entries, newest first; `fetchNextPage` loads older ones. `live` polls every 5 s. */
export function useAuditEntries(f: AuditFilter, live = false) {
  return useInfiniteQuery({
    queryKey: auditKeys.entries(f),
    initialPageParam: undefined as number | undefined,
    queryFn: async ({ pageParam }): Promise<AuditPage> => {
      const r = await get<Partial<AuditPage>>(`/api/v1/audit/page?${params(f, { limit: AUDIT_PAGE_SIZE, before: pageParam, total: pageParam === undefined ? "true" : undefined })}`);
      return {
        entries: Array.isArray(r?.entries) ? r.entries.map(normEntry) : [],
        next_cursor: typeof r?.next_cursor === "number" ? r.next_cursor : null,
        total: typeof r?.total === "number" ? r.total : undefined,
      };
    },
    getNextPageParam: (last) => last.next_cursor ?? undefined,
    refetchInterval: live ? 5_000 : false,
    staleTime: 10_000,
    retry: (n, e) => !(e instanceof ApiError && e.status < 500) && n < 2,
  });
}

/**
 * Entries over time by outcome, with the page's filters. `null` when the
 * server predates `/audit/histogram` (404), so the page can say so instead
 * of failing.
 */
export function useAuditHistogram(f: AuditFilter, buckets: number, live = false) {
  return useQuery({
    queryKey: auditKeys.histogram(f, buckets),
    refetchInterval: live ? 10_000 : 60_000,
    staleTime: 10_000,
    retry: (n, e) => !(e instanceof ApiError && e.status < 500) && n < 2,
    queryFn: async (): Promise<AuditHistogram | null> => {
      try {
        const r = await get<Partial<AuditHistogram>>(`/api/v1/audit/histogram?${params(f, { buckets })}`);
        return {
          from: num(r?.from),
          to: num(r?.to),
          step_ms: num(r?.step_ms),
          buckets: Array.isArray(r?.buckets)
            ? r.buckets.map((b) => ({ t: num(b?.t), success: num(b?.success), denied: num(b?.denied), failure: num(b?.failure) }))
            : [],
        };
      } catch (e) {
        if (e instanceof ApiError && e.status === 404) return null;
        throw e;
      }
    },
  });
}

/** Distinct actions with counts, for the type filter. */
export const useAuditActions = () =>
  useQuery({
    queryKey: auditKeys.actions,
    queryFn: async () => {
      const r = await get<unknown>("/api/v1/audit/actions");
      return Array.isArray(r)
        ? r.flatMap((a: Partial<AuditActionCount> | null) => (a && typeof a.action === "string" ? [{ action: a.action, count: num(a.count) }] : []))
        : [];
    },
    retry: false,
    staleTime: 60_000,
  });

/** Walks the whole chain on the server; the result stays on the mutation. */
export const useVerifyAudit = () =>
  useMutation({
    mutationFn: async (): Promise<VerifyReport & { at: number }> => {
      const r = await get<Partial<VerifyReport>>("/api/v1/audit/verify");
      return {
        ok: r?.ok === true,
        entries_checked: num(r?.entries_checked),
        head_hash: typeof r?.head_hash === "string" ? r.head_hash : null,
        first_invalid: Array.isArray(r?.first_invalid) ? [num(r.first_invalid[0]), str(r.first_invalid[1])] : null,
        at: Date.now(),
      };
    },
  });

/** Parses the `/audit` URL search into a filter; unknown values are dropped. */
export function parseAuditSearch(s: { q?: string; action?: string; actor?: string; outcome?: string; range?: string; since?: number; until?: number }): AuditFilter {
  const range = s.range === "all" || AUDIT_RANGES.some((r) => r.key === s.range) ? s.range : undefined;
  const custom = !range;
  return {
    q: s.q || undefined,
    actions: s.action ? s.action.split(",").filter(Boolean) : undefined,
    actor: s.actor || undefined,
    outcome: isOutcome(s.outcome) ? s.outcome : undefined,
    // No range and no custom window: the last 24 hours.
    range: range ?? (s.since || s.until ? undefined : "24h"),
    since: custom ? s.since : undefined,
    until: custom ? s.until : undefined,
  };
}
