// Alarm notifications (#11, D-110): channels, policies and the delivery history.
import { useQuery, type QueryKey } from "@tanstack/react-query";

import type { RuleScope, Severity } from "./alerts";
import { ApiError, api, get, post } from "./client";
import { useAction } from "./hooks";
import type { Role } from "./types";

export type ChannelKind = "webhook" | "slack" | "email";

/** How an email channel protects its SMTP connection. */
export type SmtpTls = "starttls" | "tls" | "none";

/** An email channel's SMTP settings as the server returns them. The password is write-only. */
export interface EmailSettings {
  smtp_host: string;
  smtp_port: number;
  tls: SmtpTls;
  username?: string;
  /** Whether a password is stored. */
  password_set: boolean;
  from: string;
  to: string[];
  subject_prefix?: string;
}

/** An email channel's settings as written. Omit `password` on update to keep the stored one. */
export interface EmailBody {
  smtp_host: string;
  /** Default: 587 (starttls), 465 (tls), 25 (none). */
  smtp_port?: number;
  tls: SmtpTls;
  username?: string;
  password?: string;
  from: string;
  to: string[];
  subject_prefix?: string;
}

/** A channel as the server returns it. The URL is write-only: only its `target` (scheme://host[:port]) comes back. */
export interface Channel {
  id: string;
  workspace_id: string;
  name: string;
  kind: ChannelKind;
  enabled: boolean;
  target: string;
  /** Vault item whose password signs webhook requests; absent when unsigned. */
  secret_item?: string;
  /** SMTP settings of an email channel. */
  email?: EmailSettings;
  created_by: string;
  created_at: number;
  updated_at: number;
}

export interface ChannelBody {
  name: string;
  kind: ChannelKind;
  enabled: boolean;
  /** Required on create; omit on update to keep the stored URL. */
  url?: string;
  secret_item?: string;
  /** Required for, and only for, an email channel. */
  email?: EmailBody;
}

export type PolicyEvent = "fired" | "resolved";

export interface Policy {
  id: string;
  workspace_id: string;
  name: string;
  enabled: boolean;
  min_severity: Severity;
  /** Alert rule ids; empty = every rule. */
  rules: string[];
  /** Absent = every resource. */
  scope?: RuleScope;
  events: PolicyEvent[];
  /** Absent = no reminders. */
  repeat_interval_secs?: number;
  channels: string[];
  /** The email channel that sends to `email_users` and `email_roles` (D-117). */
  email_channel?: string;
  /** Users emailed by name, when they may see the alarm. */
  email_users: string[];
  /** Roles whose holders on the alarm's resource (or above it) are emailed. */
  email_roles: Role[];
  /** Named users without an email address; they are skipped. */
  warnings: { users_without_email: string[] };
  created_by: string;
  created_at: number;
  updated_at: number;
}

export type PolicyBody = Pick<
  Policy,
  | "name"
  | "enabled"
  | "min_severity"
  | "rules"
  | "scope"
  | "events"
  | "repeat_interval_secs"
  | "channels"
  | "email_channel"
  | "email_users"
  | "email_roles"
>;

/** The roles a policy can email, in the order the editor lists them. */
export const EMAIL_ROLES: Role[] = ["platform_admin", "workspace_admin", "operator", "developer", "dba", "auditor", "customer"];

/** Why a person a policy names was not emailed. */
export type RecipientSkip = "no_email" | "no_access" | "disabled" | "too_many";

/** Whom a delivery to people went to. `to` is empty unless the caller manages notification channels. */
export interface DeliveryRecipients {
  count: number;
  to: string[];
  skipped: { user_id: string; reason: RecipientSkip }[];
}

/** `suppressed`: held back by a silence (`silence_id`). */
export type DeliveryStatus =
  "pending" | "sending" | "sent" | "failed" | "cancelled" | "suppressed";
export type DeliveryEvent =
  "fired" | "resolved" | "ended" | "reminder" | "test";
export const DELIVERY_STATUSES: DeliveryStatus[] = [
  "pending",
  "sending",
  "sent",
  "failed",
  "cancelled",
  "suppressed",
];

export interface Delivery {
  id: string;
  workspace_id: string;
  /** Absent for a test message. */
  alarm_id?: string;
  event: DeliveryEvent;
  occurrence: number;
  /** Absent for a test message. */
  policy_id?: string;
  channel_id: string;
  status: DeliveryStatus;
  /** Number of attempts made so far. */
  attempts: number;
  next_attempt_at?: number;
  /** Error class of the last attempt: timeout, connect, tls, blocked_target, http_status, secret, config, smtp_reply. */
  last_error?: string;
  /** HTTP status, or the SMTP reply code when `last_error` is `smtp_reply` or the email was sent. */
  last_http_status?: number;
  /** Why it was cancelled or suppressed. */
  detail?: string;
  /** The silence that held it back (`suppressed`), or the one this message follows (`after_silence`). */
  silence_id?: string;
  /** Sent because a silence ended; stands for the messages it held back. */
  after_silence: boolean;
  created_at: number;
  finished_at?: number;
  channel_name?: string;
  policy_name?: string;
  alarm_title?: string;
  resource?: { kind: string; id: string };
  /** Set on a policy's delivery to people (D-117). */
  recipients?: DeliveryRecipients;
}

export interface Attempt {
  delivery_id: string;
  number: number;
  at: number;
  ok: boolean;
  http_status?: number;
  error?: string;
  duration_ms?: number;
}

/** A delivery with its attempts (`attempts` is the list here, not the count). */
export type DeliveryView = Omit<Delivery, "attempts"> & { attempts: Attempt[] };

export interface DeliveryFilter {
  alarm?: string;
  channel?: string;
  status?: DeliveryStatus;
}

/** Same page size as the alarm list. */
export const DELIVERY_LIMIT = 200;

/** Limits the server enforces; the dialogs check them before sending. */
export const NOTIFY_LIMITS = {
  name: 80,
  url: 2000,
  recipients: 20,
  subjectPrefix: 40,
  smtpHost: 253,
  policyChannels: 10,
  policyEmailUsers: 100,
  policyEmailRoles: 20,
  policyRules: 100,
  scopeIds: 500,
  repeatMin: 300,
  repeatMax: 604_800,
} as const;

/** `pending` and `sending` still change; the others are final. */
export const deliveryOpen = (d: { status?: string } | null | undefined) =>
  d?.status === "pending" || d?.status === "sending";

const enc = encodeURIComponent;

export const notificationKeys = {
  channels: ["notification-channels"] as const,
  policies: ["notification-policies"] as const,
  deliveries: ["notification-deliveries"] as const,
  deliveryList: (f: DeliveryFilter) =>
    [
      "notification-deliveries",
      "list",
      f.alarm ?? "",
      f.channel ?? "",
      f.status ?? "",
    ] as const,
  delivery: (id: string | null) =>
    ["notification-deliveries", "one", id] as const,
};

// ------------------------------------------------------------------ normalizing

const strings = (v: unknown): string[] =>
  Array.isArray(v) ? v.filter((x): x is string => typeof x === "string") : [];
const optStr = (v: unknown) => (typeof v === "string" && v ? v : undefined);
const optNum = (v: unknown) =>
  typeof v === "number" && Number.isFinite(v) ? v : undefined;

function normChannel(c: Partial<Channel> | null | undefined): Channel {
  const o = c ?? {};
  return {
    id: o.id ?? "",
    workspace_id: o.workspace_id ?? "",
    name: o.name ?? "unnamed",
    kind: o.kind === "slack" || o.kind === "email" ? o.kind : "webhook",
    enabled: o.enabled !== false,
    target: o.target ?? "",
    secret_item: optStr(o.secret_item),
    email: o.email ?? undefined,
    created_by: o.created_by ?? "",
    created_at: o.created_at ?? 0,
    updated_at: o.updated_at ?? 0,
  };
}

function normScope(
  s: Partial<RuleScope> | null | undefined,
): RuleScope | undefined {
  if (s?.kind !== "servers" && s?.kind !== "services") return undefined;
  return { kind: s.kind, ids: strings(s.ids) };
}

function normPolicy(p: Partial<Policy> | null | undefined): Policy {
  const o = p ?? {};
  return {
    id: o.id ?? "",
    workspace_id: o.workspace_id ?? "",
    name: o.name ?? "unnamed",
    enabled: o.enabled !== false,
    min_severity: o.min_severity ?? "warning",
    rules: strings(o.rules),
    scope: normScope(o.scope),
    events: strings(o.events).filter(
      (e): e is PolicyEvent => e === "fired" || e === "resolved",
    ),
    repeat_interval_secs: optNum(o.repeat_interval_secs),
    channels: strings(o.channels),
    email_channel: optStr(o.email_channel),
    email_users: strings(o.email_users),
    email_roles: strings(o.email_roles).filter((r): r is Role => (EMAIL_ROLES as string[]).includes(r)),
    warnings: { users_without_email: strings(o.warnings?.users_without_email) },
    created_by: o.created_by ?? "",
    created_at: o.created_at ?? 0,
    updated_at: o.updated_at ?? 0,
  };
}

function normRecipients(r: Partial<DeliveryRecipients> | null | undefined): DeliveryRecipients | undefined {
  if (!r) return undefined;
  const skipped = Array.isArray(r.skipped) ? r.skipped.filter((x) => typeof x?.user_id === "string") : [];
  return { count: optNum(r.count) ?? 0, to: strings(r.to), skipped };
}

type RawDelivery = Partial<Omit<Delivery, "attempts">> & {
  attempt_count?: unknown;
  attempts?: unknown;
};

function normDeliveryBase(o: RawDelivery): Omit<Delivery, "attempts"> {
  return {
    id: o.id ?? "",
    workspace_id: o.workspace_id ?? "",
    alarm_id: optStr(o.alarm_id),
    event: o.event ?? "fired",
    occurrence: optNum(o.occurrence) ?? 0,
    policy_id: optStr(o.policy_id),
    channel_id: o.channel_id ?? "",
    status: o.status ?? "pending",
    next_attempt_at: optNum(o.next_attempt_at),
    last_error: optStr(o.last_error),
    last_http_status: optNum(o.last_http_status),
    detail: optStr(o.detail),
    silence_id: optStr(o.silence_id),
    after_silence: o.after_silence === true,
    created_at: o.created_at ?? 0,
    finished_at: optNum(o.finished_at),
    channel_name: optStr(o.channel_name),
    policy_name: optStr(o.policy_name),
    alarm_title: optStr(o.alarm_title),
    resource:
      o.resource && typeof o.resource.id === "string"
        ? { kind: o.resource.kind ?? "", id: o.resource.id }
        : undefined,
    recipients: normRecipients(o.recipients),
  };
}

function normDelivery(d: RawDelivery | null | undefined): Delivery {
  const o = d ?? {};
  // The server names the count `attempt_count`; `attempts` is the list in the detail answer.
  return {
    ...normDeliveryBase(o),
    attempts:
      optNum(o.attempt_count) ??
      (Array.isArray(o.attempts) ? o.attempts.length : 0),
  };
}

function normAttempt(a: Partial<Attempt> | null | undefined): Attempt {
  const o = a ?? {};
  return {
    delivery_id: o.delivery_id ?? "",
    number: optNum(o.number) ?? 0,
    at: o.at ?? 0,
    ok: o.ok === true,
    http_status: optNum(o.http_status),
    error: optStr(o.error),
    duration_ms: optNum(o.duration_ms),
  };
}

function normDeliveryView(d: RawDelivery | null | undefined): DeliveryView {
  const o = d ?? {};
  // `attempts` is the list in this answer (the count is `attempt_count`).
  const list: unknown[] = Array.isArray(o.attempts) ? o.attempts : [];
  return {
    ...normDeliveryBase(o),
    attempts: list
      .map((a) => normAttempt(a as Partial<Attempt>))
      .sort((a, b) => a.number - b.number),
  };
}

// ------------------------------------------------------------------ queries

/** Client errors (refused, missing endpoint on an older server) are not retried. */
const retry = (n: number, e: unknown) =>
  !(e instanceof ApiError && e.status < 500) && n < 2;

export const useChannels = (enabled = true) =>
  useQuery({
    queryKey: notificationKeys.channels,
    queryFn: () =>
      get<Partial<Channel>[]>("/api/v1/notification-channels").then((l) =>
        Array.isArray(l) ? l.map(normChannel).filter((c) => c.id) : [],
      ),
    enabled,
    retry,
    staleTime: 10_000,
  });

export const usePolicies = (enabled = true) =>
  useQuery({
    queryKey: notificationKeys.policies,
    queryFn: () =>
      get<Partial<Policy>[]>("/api/v1/notification-policies").then((l) =>
        Array.isArray(l) ? l.map(normPolicy).filter((p) => p.id) : [],
      ),
    enabled,
    retry,
    staleTime: 10_000,
  });

/**
 * Delivery history, newest first. The dispatch loop runs every 5 s, so the
 * list refreshes that fast while a delivery is not final, and at the alarm
 * list's pace (15 s) otherwise.
 */
export const useDeliveries = (filter: DeliveryFilter, enabled = true) =>
  useQuery({
    queryKey: notificationKeys.deliveryList(filter),
    queryFn: () => {
      const p = new URLSearchParams({ limit: String(DELIVERY_LIMIT) });
      if (filter.alarm) p.set("alarm", filter.alarm);
      if (filter.channel) p.set("channel", filter.channel);
      if (filter.status) p.set("status", filter.status);
      return get<RawDelivery[]>(`/api/v1/notification-deliveries?${p}`).then(
        (l) =>
          Array.isArray(l) ? l.map(normDelivery).filter((d) => d.id) : [],
      );
    },
    enabled,
    retry,
    refetchInterval: (q) =>
      (q.state.data ?? []).some(deliveryOpen) ? 5_000 : 15_000,
  });

export const useDelivery = (id: string | null) =>
  useQuery({
    queryKey: notificationKeys.delivery(id),
    queryFn: () =>
      get<RawDelivery>(`/api/v1/notification-deliveries/${enc(id ?? "")}`).then(
        normDeliveryView,
      ),
    enabled: !!id,
    retry,
    refetchInterval: (q) => (deliveryOpen(q.state.data) ? 5_000 : false),
  });

// ------------------------------------------------------------------ mutations

// Deliveries carry channel and policy names, so they change with both.
const channelKeys: QueryKey[] = [
  notificationKeys.channels,
  notificationKeys.deliveries,
];
const policyKeys: QueryKey[] = [
  notificationKeys.policies,
  notificationKeys.deliveries,
];

/** Needs `notification.manage` with an MFA-verified sign-in; the API client asks for the code and retries. */
export const useSaveChannel = () =>
  useAction(
    ({ id, body }: { id?: string; body: ChannelBody }) =>
      (id
        ? api<Partial<Channel>>(
            "PUT",
            `/api/v1/notification-channels/${enc(id)}`,
            body,
          )
        : post<Partial<Channel>>("/api/v1/notification-channels", body)
      ).then(normChannel),
    channelKeys,
  );

/** 409 while a policy still uses the channel. */
export const useDeleteChannel = () =>
  useAction(
    (id: string) =>
      api<void>("DELETE", `/api/v1/notification-channels/${enc(id)}`),
    channelKeys,
  );

/** Sends one test message now and answers with the delivery and its single attempt. 409 if the channel is turned off. */
export const useTestChannel = () =>
  useAction(
    (id: string) =>
      api<RawDelivery>(
        "POST",
        `/api/v1/notification-channels/${enc(id)}/test`,
      ).then(normDeliveryView),
    [notificationKeys.deliveries],
  );

export const useSavePolicy = () =>
  useAction(
    ({ id, body }: { id?: string; body: PolicyBody }) =>
      (id
        ? api<Partial<Policy>>(
            "PUT",
            `/api/v1/notification-policies/${enc(id)}`,
            body,
          )
        : post<Partial<Policy>>("/api/v1/notification-policies", body)
      ).then(normPolicy),
    policyKeys,
  );

export const useDeletePolicy = () =>
  useAction(
    (id: string) =>
      api<void>("DELETE", `/api/v1/notification-policies/${enc(id)}`),
    policyKeys,
  );
