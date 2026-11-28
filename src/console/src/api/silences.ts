// Silences and maintenance windows (#11, D-112). A silence holds back the
// notifications of the alarms it covers for a bounded time; a maintenance
// window is a silence that starts later. Alarms are still evaluated.
import { type QueryKey, useQuery } from "@tanstack/react-query";

import type { Severity } from "./alerts";
import { ApiError, get, post } from "./client";
import { useAction } from "./hooks";
import { notificationKeys } from "./notifications";

export type SilenceState = "scheduled" | "active" | "expired";
export const SILENCE_STATES: SilenceState[] = ["active", "scheduled", "expired"];

/** Which alarms a silence covers, by where their resource is. Services are matched by id. */
export type SilenceScope =
  | { kind: "workspace" }
  | { kind: "cluster"; id: string }
  | { kind: "environment"; id: string }
  | { kind: "servers"; ids: string[] }
  | { kind: "services"; ids: string[] };
export type SilenceScopeKind = SilenceScope["kind"];

export interface Silence {
  id: string;
  workspace_id: string;
  scope: SilenceScope;
  /** Alert rule ids; empty = every rule. */
  rules: string[];
  min_severity?: Severity;
  max_severity?: Severity;
  starts_at: number;
  /** Exclusive end (epoch ms). */
  ends_at: number;
  reason: string;
  /** `user:<id>` or the raw id; empty when unknown. */
  created_by: string;
  created_at: number;
  updated_at: number;
  ended_by?: string;
  /** By the server's clock. */
  state: SilenceState;
}

/** What an alarm view carries while an active silence covers it. */
export interface SilenceMark {
  silence_id: string;
  until: number;
  reason: string;
}

export interface SilenceBody {
  scope: SilenceScope;
  rules: string[];
  min_severity?: Severity;
  max_severity?: Severity;
  /** Omitted = now. */
  starts_at?: number;
  ends_at?: number;
  duration_secs?: number;
  reason: string;
}

/** Limits the server enforces; the dialog checks them before sending. */
export const SILENCE_LIMITS = { minSecs: 60, maxSecs: 2_592_000, aheadMs: 365 * 86_400_000, reason: 500, rules: 100, scopeIds: 100 } as const;
export const SILENCE_LIMIT = 200;

const enc = encodeURIComponent;

export const silenceKeys = {
  all: ["silences"] as const,
  list: (state: SilenceState | "") => ["silences", "list", state] as const,
  one: (id: string | null) => ["silences", "one", id] as const,
};

// ------------------------------------------------------------------ normalizing

const strings = (v: unknown): string[] => (Array.isArray(v) ? v.filter((x): x is string => typeof x === "string") : []);
const num = (v: unknown) => (typeof v === "number" && Number.isFinite(v) ? v : 0);
const SEVS: Severity[] = ["info", "warning", "major", "critical"];
const sev = (v: unknown) => (SEVS.includes(v as Severity) ? (v as Severity) : undefined);

function principal(v: unknown): string {
  if (typeof v === "string") return v;
  if (v && typeof v === "object") {
    const o = v as Record<string, unknown>;
    if (typeof o.id === "string") return typeof o.type === "string" ? `${o.type}:${o.id}` : o.id;
  }
  return "";
}

function normScope(v: unknown): SilenceScope {
  const o = (v && typeof v === "object" ? v : {}) as Record<string, unknown>;
  const id = typeof o.id === "string" ? o.id : "";
  switch (o.kind) {
    case "cluster":
      return { kind: "cluster", id };
    case "environment":
      return { kind: "environment", id };
    case "servers":
      return { kind: "servers", ids: strings(o.ids) };
    case "services":
      return { kind: "services", ids: strings(o.ids) };
    default:
      return { kind: "workspace" };
  }
}

export function normSilence(v: unknown): Silence {
  const o = (v && typeof v === "object" ? v : {}) as Record<string, unknown>;
  const starts = num(o.starts_at);
  const ends = num(o.ends_at);
  const now = Date.now();
  const state: SilenceState =
    o.state === "scheduled" || o.state === "active" || o.state === "expired" ? o.state : now >= ends ? "expired" : now < starts ? "scheduled" : "active";
  return {
    id: typeof o.id === "string" ? o.id : "",
    workspace_id: typeof o.workspace_id === "string" ? o.workspace_id : "",
    scope: normScope(o.scope),
    rules: strings(o.rules),
    min_severity: sev(o.min_severity),
    max_severity: sev(o.max_severity),
    starts_at: starts,
    ends_at: ends,
    reason: typeof o.reason === "string" ? o.reason : "",
    created_by: principal(o.created_by),
    created_at: num(o.created_at),
    updated_at: num(o.updated_at),
    ended_by: principal(o.ended_by) || undefined,
    state,
  };
}

/** A silence mark from an alarm answer; `undefined` when absent or unusable. */
export function normMark(v: unknown): SilenceMark | undefined {
  if (!v || typeof v !== "object") return undefined;
  const o = v as Record<string, unknown>;
  if (typeof o.silence_id !== "string" || typeof o.until !== "number") return undefined;
  return { silence_id: o.silence_id, until: o.until, reason: typeof o.reason === "string" ? o.reason : "" };
}

// ------------------------------------------------------------------ queries

/** Client errors (refused, missing endpoint on an older server) are not retried. */
const retry = (n: number, e: unknown) => !(e instanceof ApiError && e.status < 500) && n < 2;

/** Latest start first. Refreshes every 30 s so states move from scheduled to active to expired. */
export const useSilences = (state: SilenceState | "") =>
  useQuery({
    queryKey: silenceKeys.list(state),
    queryFn: () => {
      const p = new URLSearchParams({ limit: String(SILENCE_LIMIT) });
      if (state) p.set("state", state);
      return get<unknown>(`/api/v1/silences?${p}`).then((l) => (Array.isArray(l) ? l.map(normSilence).filter((s) => s.id) : []));
    },
    retry,
    refetchInterval: 30_000,
  });

export const useSilence = (id: string | null) =>
  useQuery({
    queryKey: silenceKeys.one(id),
    queryFn: () => get<unknown>(`/api/v1/silences/${enc(id ?? "")}`).then(normSilence),
    enabled: !!id,
    retry,
  });

// ------------------------------------------------------------------ mutations

// A silence changes which alarms show as silenced and which deliveries are held back.
const silenceInvalidates: QueryKey[] = [silenceKeys.all, ["alarms"], ["alarm"], notificationKeys.deliveries];

/** Needs `alert.manage` on everything the scope names. */
export const useCreateSilence = () => useAction((b: SilenceBody) => post<unknown>("/api/v1/silences", b).then(normSilence), silenceInvalidates);

export const useExtendSilence = () =>
  useAction(
    ({ id, ...b }: { id: string; ends_at?: number; extend_secs?: number }) => post<unknown>(`/api/v1/silences/${enc(id)}/extend`, b).then(normSilence),
    silenceInvalidates,
  );

/** Ends it now; held-back notifications for still-firing alarms are sent. */
export const useEndSilence = () => useAction((id: string) => post<unknown>(`/api/v1/silences/${enc(id)}/end`, {}).then(normSilence), silenceInvalidates);
