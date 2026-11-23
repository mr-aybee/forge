import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { api, get, post } from "./client";
import { type SilenceMark, normMark } from "./silences";

/** Alerts (#11, D-087). */
export type Severity = "info" | "warning" | "major" | "critical";
export type AlarmState = "firing" | "resolved" | "ended";

export type Condition =
  | { kind: "threshold"; cmp: "gt" | "ge" | "lt" | "le" | "eq" | "ne"; value: number }
  | { kind: "state_in"; states: string[] }
  | { kind: "state_not_in"; states: string[] };

export type Mode =
  | { mode: "immediate" }
  | { mode: "sustained"; for_ms: number }
  | { mode: "window_average"; window_ms: number; min_coverage_pct: number }
  | { mode: "repeated"; count: number; window_ms: number; min_spacing_ms: number }
  | { mode: "any_in_window"; window_ms: number };

export interface RuleSpec {
  trigger: Condition;
  mode: Mode;
  recovery: { condition?: Condition | null; hold_ms: number };
}

/** What a rule watches: nodes, services, package installations (databases included) or ingress certificates (D-118). No ids: every one of that kind. */
export type RuleScopeKind = "servers" | "services" | "installations" | "certificates";
export type RuleScope = { kind: RuleScopeKind; ids?: string[] };

export interface AlertRule {
  id: string;
  workspace_id: string;
  name: string;
  description: string;
  enabled: boolean;
  severity: Severity;
  scope: RuleScope;
  subject: string;
  spec: RuleSpec;
  revision: number;
  created_at: number;
  updated_at: number;
  /** Plain description of the condition (`GET /alert-rules/{id}` only). */
  condition?: string | null;
}

export interface Alarm {
  id: string;
  rule_id: string;
  rule_revision: number;
  title: string;
  severity: Severity;
  resource: { kind: string; id: string };
  server?: string;
  subject: string;
  state: AlarmState;
  fired_value: { type: "num" | "state"; value: number | string } | null;
  fired_at: number;
  resolved_at?: number;
  end_reason?: string;
  acknowledged_by?: string;
  acknowledged_at?: number;
  previous?: string;
  /** Present while the alarm fires and an active silence holds its notifications back. */
  silence?: SilenceMark;
}

/** Keeps the alarm as the server sent it, with a usable `silence` or none (older servers send none). */
const withMark = <T extends Alarm>(a: T): T => ({ ...a, silence: normMark((a as { silence?: unknown }).silence) });

export interface AlarmEvent {
  at: number;
  kind: string;
  actor?: string;
  detail: { note?: string; reason?: string; value?: unknown };
}

export type RuleInput = Omit<AlertRule, "id" | "workspace_id" | "revision" | "created_at" | "updated_at" | "condition">;

export const useAlarms = (active: boolean, enabled = true) =>
  useQuery({
    queryKey: ["alarms", active],
    enabled,
    queryFn: () => get<Alarm[]>(`/api/v1/alarms?limit=200${active ? "&active=true" : ""}`).then((l) => (Array.isArray(l) ? l.map(withMark) : [])),
    refetchInterval: 15_000,
  });

export const useAlarm = (id: string | null) =>
  useQuery({
    queryKey: ["alarm", id],
    queryFn: () => get<Alarm & { events: AlarmEvent[] }>(`/api/v1/alarms/${id}`).then(withMark),
    enabled: !!id,
  });

export const useAlertRules = () => useQuery({ queryKey: ["alert-rules"], queryFn: () => get<AlertRule[]>(`/api/v1/alert-rules`) });

function useInvalidating<A, R>(fn: (a: A) => Promise<R>) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: fn,
    onSuccess: async () => {
      await Promise.all(["alarms", "alarm", "alert-rules"].map((k) => qc.invalidateQueries({ queryKey: [k] })));
    },
  });
}

export const useSaveRule = () =>
  useInvalidating((b: { id?: string; rule: RuleInput }) =>
    b.id ? api<AlertRule>("PUT", `/api/v1/alert-rules/${b.id}`, b.rule) : post<AlertRule>(`/api/v1/alert-rules`, b.rule),
  );
export const useDeleteRule = () => useInvalidating((id: string) => api<void>("DELETE", `/api/v1/alert-rules/${id}`));
export const useAcknowledge = () =>
  useInvalidating((b: { id: string; note: string; undo?: boolean }) =>
    b.undo ? post<Alarm>(`/api/v1/alarms/${b.id}/unacknowledge`, {}) : post<Alarm>(`/api/v1/alarms/${b.id}/acknowledge`, { note: b.note }),
  );
export const useAddNote = () => useInvalidating((b: { id: string; note: string }) => post<void>(`/api/v1/alarms/${b.id}/notes`, { note: b.note }));

// ------------------------------------------------------------- catalogue (D-118)

/** Query keys of the alert catalogue. */
export const alertKeys = {
  subjects: () => ["alert-subjects"] as const,
  templates: (place?: Place) => ["alert-rule-templates", place ?? "all"] as const,
};

/** One subject a rule can watch (`GET /api/v1/alert-subjects`). */
export interface SubjectInfo {
  /** The subject, or a pattern like `health.readiness.{check}`. */
  subject: string;
  pattern: boolean;
  kind: "number" | "state";
  states?: string[];
  /** `percent`, `ratio`, `seconds` or `days`. */
  unit?: string;
  scopes: RuleScopeKind[];
  source: "agent" | "probe" | "control_plane";
  description: string;
}

export interface SubjectList {
  subjects: SubjectInfo[];
  /** Concrete `health.*` subjects observed on the workspace's services. */
  seen_health: string[];
}

export type Place = "node" | "service" | "database" | "certificate" | "any";

/** A ready-made rule (`GET /api/v1/alert-rule-templates`). */
export interface AlertTemplate {
  id: string;
  place: Place;
  name: string;
  description: string;
  severity: Severity;
  /** May contain `{check}`: a health check name fills it. */
  subject: string;
  scope: RuleScopeKind;
  spec: RuleSpec;
}

export const useAlertSubjects = () =>
  useQuery({ queryKey: alertKeys.subjects(), queryFn: () => get<SubjectList>(`/api/v1/alert-subjects`), staleTime: 60_000 });

export const useAlertTemplates = (place?: Place) =>
  useQuery({
    queryKey: alertKeys.templates(place),
    queryFn: () => get<AlertTemplate[]>(`/api/v1/alert-rule-templates${place ? `?place=${place}` : ""}`),
    staleTime: 5 * 60_000,
  });

/** The catalogue entry of a concrete subject (`health.liveness.http` matches its pattern). */
export function subjectInfo(list: SubjectInfo[] | undefined, subject: string): SubjectInfo | undefined {
  const all = list ?? [];
  return (
    all.find((s) => s.subject === subject) ??
    all.find((s) => s.pattern && subject.startsWith(s.subject.replace("{check}", "")) && subject.length > s.subject.length - "{check}".length)
  );
}
