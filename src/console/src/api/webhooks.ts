// Deploy webhooks (D-159). Server: `src/forge/crates/apps/forge-server/src/api/webhooks.rs`.
// The list never returns the token or secret; create and rotate return them
// once. Mutations need `webhook.manage` and an MFA-verified sign-in (the client
// handles step-up).
import { useQuery } from "@tanstack/react-query";

import { api, get, post } from "./client";
import { useAction } from "./hooks";

export type WebhookTargetKind = "service" | "stack";
export type WebhookSignature = "none" | "github" | "gitlab" | "forge";
export const WEBHOOK_SIGNATURES: WebhookSignature[] = ["forge", "github", "gitlab", "none"];
export type DeliveryOutcome = "accepted" | "rejected" | "rate_limited" | "failed";

export interface WebhookTarget {
  kind: WebhookTargetKind;
  id: string;
}

export interface Webhook {
  id: string;
  workspace_id: string;
  name: string;
  target: WebhookTarget;
  token_hint: string;
  signature: WebhookSignature;
  tag_pattern: string | null;
  enabled: boolean;
  created_by: string;
  created_at?: number;
  updated_at?: number;
  last_delivery_at: number | null;
}

/** Create body. On update only `name`, `tag_pattern` and `enabled` change. */
export interface WebhookBody {
  name: string;
  target: WebhookTarget;
  signature: WebhookSignature;
  tag_pattern?: string | null;
  enabled: boolean;
}

/** Create and rotate: the token and secret are shown once. `secret` is null for `none`. */
export interface WebhookIssued {
  webhook: Webhook;
  token: string;
  secret: string | null;
}

export interface WebhookDelivery {
  id: string;
  at?: number;
  outcome: DeliveryOutcome;
  reason: string;
  job_id: string | null;
  tag: string | null;
  source_ip: string;
}

export const webhookKeys = {
  all: ["webhooks"] as const,
  deliveries: (id: string) => ["webhooks", "deliveries", id] as const,
};

type Obj = Record<string, unknown>;
const obj = (v: unknown): Obj => (v && typeof v === "object" ? (v as Obj) : {});
const str = (v: unknown) => (typeof v === "string" ? v : "");
const optStr = (v: unknown) => (typeof v === "string" && v ? v : null);
const num = (v: unknown) => (typeof v === "number" && Number.isFinite(v) ? v : undefined);
const oneOf = <T extends string>(v: unknown, all: readonly T[], d: T): T => (all.includes(v as T) ? (v as T) : d);

function normWebhook(v: unknown): Webhook {
  const o = obj(v);
  const t = obj(o.target);
  return {
    id: str(o.id),
    workspace_id: str(o.workspace_id),
    name: str(o.name),
    target: { kind: oneOf(t.kind, ["service", "stack"] as const, "service"), id: str(t.id) },
    token_hint: str(o.token_hint),
    signature: oneOf(o.signature, WEBHOOK_SIGNATURES, "none"),
    tag_pattern: optStr(o.tag_pattern),
    enabled: o.enabled !== false,
    created_by: str(o.created_by),
    created_at: num(o.created_at),
    updated_at: num(o.updated_at),
    last_delivery_at: num(o.last_delivery_at) ?? null,
  };
}

function normIssued(v: unknown): WebhookIssued {
  const o = obj(v);
  return { webhook: normWebhook(o.webhook), token: str(o.token), secret: optStr(o.secret) };
}

function normDelivery(v: unknown, i: number): WebhookDelivery {
  const o = obj(v);
  return {
    id: str(o.id) || `${num(o.at) ?? 0}-${i}`,
    at: num(o.at),
    outcome: oneOf(o.outcome, ["accepted", "rejected", "rate_limited", "failed"] as const, "failed"),
    reason: str(o.reason),
    job_id: optStr(o.job_id),
    tag: optStr(o.tag),
    source_ip: str(o.source_ip),
  };
}

const enc = encodeURIComponent;

/** The public URL a CI system calls. */
export const webhookUrl = (token: string) => `${window.location.origin}/hooks/${token}`;

/** All webhooks of the workspace; filter by target with `select`. */
export const useWebhooks = (target?: WebhookTarget) =>
  useQuery({
    queryKey: webhookKeys.all,
    queryFn: () => get<unknown>("/api/v1/webhooks").then((v) => (Array.isArray(v) ? v.map(normWebhook) : [])),
    select: target ? (rows: Webhook[]) => rows.filter((w) => w.target.kind === target.kind && w.target.id === target.id) : undefined,
    staleTime: 15_000,
    refetchInterval: 30_000,
  });

/** The newest deliveries; polls every 10 s while mounted. */
export const useWebhookDeliveries = (id: string) =>
  useQuery({
    queryKey: webhookKeys.deliveries(id),
    queryFn: () => get<unknown>(`/api/v1/webhooks/${enc(id)}/deliveries`).then((v) => (Array.isArray(v) ? v.map(normDelivery) : [])),
    refetchInterval: 10_000,
    staleTime: 5_000,
  });

export const useCreateWebhook = () => useAction((b: WebhookBody) => post<unknown>("/api/v1/webhooks", b).then(normIssued), [webhookKeys.all]);
export const useUpdateWebhook = () =>
  useAction(({ id, body }: { id: string; body: WebhookBody }) => api<unknown>("PUT", `/api/v1/webhooks/${enc(id)}`, body), [webhookKeys.all]);
export const useRotateWebhook = () => useAction((id: string) => post<unknown>(`/api/v1/webhooks/${enc(id)}/rotate`, {}).then(normIssued), [webhookKeys.all]);
export const useDeleteWebhook = () => useAction((id: string) => api<void>("DELETE", `/api/v1/webhooks/${enc(id)}`), [webhookKeys.all]);
