import type { ReactNode } from "react";

import type { Severity } from "../../api/alerts";
import { ApiError } from "../../api/client";
import { useMe } from "../../api/hooks";
import type {
  Attempt,
  Delivery,
  DeliveryEvent,
  DeliveryStatus,
} from "../../api/notifications";
import type { Role } from "../../api/types";
import { errorText } from "../../app/toast";
import { Badge, Skeleton, Switch, short } from "../ui";

export const SEV_TONE: Record<Severity, string> = {
  critical: "bad",
  major: "warn",
  warning: "warn",
  info: "muted",
};
export const SEVERITIES: Severity[] = ["critical", "major", "warning", "info"];

/** Sent, failed, cancelled and not-yet-final deliveries each get their own tone. */
const DELIVERY_TONE: Record<DeliveryStatus, string> = {
  sent: "ok",
  failed: "bad",
  cancelled: "muted",
  pending: "warn",
  sending: "accent",
  suppressed: "muted",
};

export function DeliveryBadge({ status }: { status: DeliveryStatus }) {
  return <Badge tone={DELIVERY_TONE[status] ?? "muted"}>{status}</Badge>;
}

const EVENT_LABEL: Record<DeliveryEvent, string> = {
  fired: "Fired",
  resolved: "Resolved",
  ended: "Ended",
  reminder: "Reminder",
  test: "Test",
};
export const eventLabel = (d: { event: DeliveryEvent; occurrence?: number }) =>
  d.event === "reminder" && d.occurrence
    ? `Reminder ${d.occurrence}`
    : (EVENT_LABEL[d.event] ?? d.event);

/** What each error class of an attempt means. */
const ERROR_HELP: Record<string, string> = {
  timeout: "the receiver did not answer in time",
  connect: "the connection failed",
  tls: "TLS failed: the certificate was not accepted, or the mail server does not offer STARTTLS",
  blocked_target: "the address is not allowed as a target",
  http_status: "the receiver refused the message",
  secret: "the signing secret could not be read",
  config: "the stored settings are not usable",
  smtp_reply:
    "the mail server refused the message (4xx is retried, 5xx is final)",
};

/** `SMTP 451` for a mail server's reply code, `HTTP 503` for a web receiver's status. */
const codeText = (code: number, error: string | undefined) =>
  `${error === "smtp_reply" ? "SMTP" : "HTTP"} ${code}`;
export const errorHelp = (error: string | undefined) =>
  error ? (ERROR_HELP[error] ?? error) : undefined;

/** Last error of a delivery in one short cell: `HTTP 503`, `timeout`, or `—`. */
export function lastResult(
  d: Pick<Delivery, "last_error" | "last_http_status">,
): string {
  if (d.last_http_status !== undefined)
    return d.last_error &&
      d.last_error !== "http_status" &&
      d.last_error !== "smtp_reply"
      ? `${d.last_error} · ${codeText(d.last_http_status, d.last_error)}`
      : codeText(d.last_http_status, d.last_error);
  return d.last_error ?? "—";
}

/** One attempt in words: `HTTP 200`, `timeout: the receiver did not answer in time`. */
export function attemptText(a: Pick<Attempt, "http_status" | "error">): string {
  const parts: string[] = [];
  if (a.http_status !== undefined) parts.push(codeText(a.http_status, a.error));
  if (a.error && a.error !== "http_status" && a.error !== "smtp_reply")
    parts.push(`${a.error}: ${errorHelp(a.error)}`);
  else if (a.error && a.http_status === undefined)
    parts.push(errorHelp(a.error) ?? a.error);
  return parts.join(" · ") || "—";
}

/**
 * Error of a channel or policy request. A refused or cancelled MFA step-up
 * (the API client already asked for the code) is named as such.
 */
export function actionError(e: unknown): string {
  if (e instanceof ApiError && e.code === "step_up_required")
    return `Not verified: ${e.message}`;
  if (e instanceof ApiError && e.status === 403)
    return `Not allowed: ${e.message}`;
  return errorText(e);
}

export function FormError({ children }: { children: ReactNode }) {
  return (
    <p
      role="alert"
      className="rounded-md bg-bad-soft px-3 py-2 text-xs break-words text-bad"
    >
      {children}
    </p>
  );
}

// Mirrors the Cedar policies; the server enforces them, this only disables the buttons and says why.
const CHANNEL_MANAGERS: Role[] = ["platform_admin", "workspace_admin"];
const POLICY_MANAGERS: Role[] = [
  "platform_admin",
  "workspace_admin",
  "operator",
];
export const CHANNEL_ROLE_HINT =
  "Needs a workspace or platform administrator (notification.manage).";
export const POLICY_ROLE_HINT =
  "Needs an operator or administrator (alert.manage).";

/** Whether the signed-in person holds a role that grants `notification.manage` (channels) and `alert.manage` (policies). */
export function useNotificationRights() {
  const bindings = useMe().data?.bindings;
  const roles = (Array.isArray(bindings) ? bindings : []).map((b) => b?.role);
  return {
    channels: roles.some((r) => CHANNEL_MANAGERS.includes(r)),
    policies: roles.some((r) => POLICY_MANAGERS.includes(r)),
  };
}

/** Like `Field`, for a group of controls: a `<label>` around several switches would toggle the first one. */
export function Group({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: ReactNode;
  children: ReactNode;
}) {
  return (
    <fieldset className="min-w-0 space-y-1">
      <legend className="text-xs font-medium text-muted">{label}</legend>
      {children}
      {hint && <span className="block text-xs text-faint">{hint}</span>}
    </fieldset>
  );
}

export interface PickItem {
  id: string;
  label: string;
}

/**
 * Multi-select as a wrapping list of switches (nodes of a rule, channels and
 * rules of a policy). Selected ids that are no longer in `items` stay visible
 * by their short id so they can be removed. A failed list shows the error,
 * never "nothing to pick".
 */
export function PickList({
  items,
  ids,
  onChange,
  loading,
  error,
  empty,
}: {
  items: PickItem[];
  ids: string[];
  onChange: (ids: string[]) => void;
  loading?: boolean;
  error?: unknown;
  empty?: ReactNode;
}) {
  if (loading) return <Skeleton rows={1} />;
  const gone = ids
    .filter((id) => !items.some((i) => i.id === id))
    .map((id): PickItem => ({ id, label: `${short(id)} (not found)` }));
  const all = [...items, ...gone];
  return (
    <div className="space-y-1.5">
      {error !== undefined && error !== null && (
        <FormError>Could not load the list: {errorText(error)}</FormError>
      )}
      {all.length === 0 && !error ? (
        <p className="text-xs text-faint">
          {empty ?? "Nothing to choose from yet."}
        </p>
      ) : (
        <div className="flex max-h-28 flex-wrap gap-x-3 gap-y-2 overflow-auto">
          {all.map((i) => (
            <Switch
              key={i.id}
              checked={ids.includes(i.id)}
              onChange={(on) =>
                onChange(on ? [...ids, i.id] : ids.filter((x) => x !== i.id))
              }
              label={i.label}
            />
          ))}
        </div>
      )}
    </div>
  );
}
