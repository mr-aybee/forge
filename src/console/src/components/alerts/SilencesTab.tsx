import { Link } from "@tanstack/react-router";
import { BellOff, CalendarClock, ChevronRight, Clock, Info, Loader2, Plus, Square } from "lucide-react";
import { useState } from "react";

import { type Alarm, type Severity, useAlertRules } from "../../api/alerts";
import { useAllEnvironments, useServices, useClusters, useServers } from "../../api/hooks";
import {
  SILENCE_LIMIT,
  SILENCE_LIMITS,
  SILENCE_STATES,
  type Silence,
  type SilenceBody,
  type SilenceScope,
  type SilenceScopeKind,
  type SilenceState,
  useCreateSilence,
  useEndSilence,
  useExtendSilence,
  useSilence,
  useSilences,
} from "../../api/silences";
import { nodeName } from "../../api/types";
import { useToast } from "../../app/toast";
import { SearchSelect } from "../SearchSelect";
import { Badge, Button, Confirm, CopyIcon, Drawer, Empty, ErrorNote, Field, Input, Modal, Mono, Panel, RowMenu, Select, Skeleton, Table, Td, ago, short, time } from "../ui";
import { FormError, Group, POLICY_ROLE_HINT, PickList, SEVERITIES, actionError, useNotificationRights } from "./parts";

const STATE_TONE: Record<SilenceState, "ok" | "accent" | "muted"> = { active: "ok", scheduled: "accent", expired: "muted" };
const HOUR = 3_600_000;

export function SilenceStateBadge({ state }: { state: SilenceState }) {
  return <Badge tone={STATE_TONE[state] ?? "muted"}>{state === "scheduled" ? "scheduled" : state}</Badge>;
}

// ------------------------------------------------------------------ names and links

/** Name lookups for everything a silence can name; each list is shared through the query cache. */
function useNames(scope: SilenceScopeKind | null) {
  const clusters = useClusters();
  const environments = useAllEnvironments();
  const nodes = useServers();
  const apps = useServices(undefined, undefined, scope === null || scope === "services");
  const rules = useAlertRules();
  const list = <T,>(d: T[] | undefined) => (Array.isArray(d) ? d : []);
  const cluster = (id: string | null | undefined) => list(clusters.data).find((c) => c.id === id)?.name;
  return {
    cluster,
    env: (id: string) => {
      const n = list(environments.data).find((x) => x.id === id);
      if (!n) return undefined;
      const c = cluster(n.cluster_id);
      return c ? `${n.name} · ${c}` : n.name;
    },
    node: (id: string) => {
      const n = list(nodes.data).find((x) => x.id === id);
      return n ? nodeName(n) : undefined;
    },
    app: (id: string) => list(apps.data).find((a) => a.id === id)?.name,
    rule: (id: string) => list(rules.data).find((r) => r.id === id)?.name,
    queries: { clusters, environments, nodes, apps, rules },
  };
}

type Names = ReturnType<typeof useNames>;

const linkCls = "truncate rounded hover:text-accent hover:underline";

/** The scope in one line, every name a link: `Cluster prod`, `Nodes node-1, node-2`. */
function ScopeLinks({ scope, names, max = 3 }: { scope: SilenceScope; names: Names; max?: number }) {
  switch (scope.kind) {
    case "workspace":
      return <span>Whole workspace</span>;
    case "cluster":
      return (
        <span className="flex min-w-0 items-center gap-1">
          <span className="shrink-0 text-muted">Cluster</span>
          <Link to="/clusters/$clusterId" params={{ clusterId: scope.id }} className={linkCls} title={names.cluster(scope.id) ?? scope.id}>
            {names.cluster(scope.id) ?? short(scope.id)}
          </Link>
        </span>
      );
    case "environment":
      return (
        <span className="flex min-w-0 items-center gap-1">
          <span className="shrink-0 text-muted">Environment</span>
          <Link to="/projects" search={{ sel: `env:${scope.id}` }} className={linkCls} title={names.env(scope.id) ?? scope.id}>
            {names.env(scope.id) ?? short(scope.id)}
          </Link>
        </span>
      );
    case "servers":
    case "services": {
      const isNode = scope.kind === "servers";
      const shown = scope.ids.slice(0, max);
      const label = (id: string) => (isNode ? names.node(id) : names.app(id)) ?? short(id);
      return (
        <span className="flex min-w-0 items-center gap-1" title={scope.ids.map(label).join(", ")}>
          <span className="shrink-0 text-muted">{isNode ? (scope.ids.length === 1 ? "Node" : "Nodes") : scope.ids.length === 1 ? "Service" : "Services"}</span>
          <span className="min-w-0 truncate">
            {shown.map((id, i) => (
              <span key={id}>
                {i > 0 && ", "}
                {isNode ? (
                  <Link to="/nodes/$nodeId" params={{ nodeId: id }} className={linkCls}>
                    {label(id)}
                  </Link>
                ) : (
                  <Link to="/services/$serviceId" params={{ serviceId: id }} className={linkCls}>
                    {label(id)}
                  </Link>
                )}
              </span>
            ))}
            {scope.ids.length > max && <span className="text-muted"> +{scope.ids.length - max}</span>}
          </span>
        </span>
      );
    }
  }
}

function severityText(s: Pick<Silence, "min_severity" | "max_severity">): string {
  if (s.min_severity && s.max_severity) return s.min_severity === s.max_severity ? s.min_severity : `${s.min_severity} – ${s.max_severity}`;
  if (s.min_severity) return `${s.min_severity} and worse`;
  if (s.max_severity) return `${s.max_severity} and milder`;
  return "any severity";
}

const rulesText = (rules: string[], names: Names) =>
  rules.length === 0 ? "every rule" : rules.length === 1 ? (names.rule(rules[0] ?? "") ?? `rule ${short(rules[0])}`) : `${rules.length} rules`;

/** `user:<id>` → short id; the full value in the tooltip. */
const who = (p: string | undefined) => (p ? short(p.includes(":") ? p.slice(p.indexOf(":") + 1) : p) : "—");

// ------------------------------------------------------------------ tab

/**
 * Silences: time-bounded holds on notifications. A maintenance window is a
 * silence that starts later. Alarms keep firing and show here as silenced;
 * only their messages wait.
 */
export function SilencesTab({ openId, onOpen, onOpenAlarms }: { openId?: string; onOpen: (id: string | undefined) => void; onOpenAlarms: () => void }) {
  const [state, setState] = useState<SilenceState | "">("");
  const [creating, setCreating] = useState(false);
  const [action, setAction] = useState<{ kind: "extend" | "end"; silence: Silence } | null>(null);
  const q = useSilences(state);
  const names = useNames(null);
  const can = useNotificationRights().policies;
  const list = q.data ?? [];

  const create = (
    <span title={can ? undefined : POLICY_ROLE_HINT}>
      <Button variant="primary" disabled={!can} onClick={() => setCreating(true)}>
        <Plus className="size-4" /> New silence
      </Button>
    </span>
  );

  return (
    <Panel
      title="Silences"
      description="A silence holds back the notifications of matching alarms for a set time. Alarms are still evaluated and listed. A silence that starts later is a maintenance window."
      actions={
        <div className="flex flex-wrap items-center gap-2">
          {q.isFetching && !q.isLoading && <Loader2 className="size-3.5 animate-spin text-faint" aria-label="Refreshing" />}
          <Select aria-label="State" className="w-36" value={state} onChange={(e) => setState(e.target.value as SilenceState | "")}>
            <option value="">All states</option>
            {SILENCE_STATES.map((s) => (
              <option key={s} value={s}>
                {s}
              </option>
            ))}
          </Select>
          {create}
        </div>
      }
    >
      {q.isLoading ? (
        <Skeleton rows={3} />
      ) : q.isError ? (
        <ErrorNote error={q.error} />
      ) : list.length === 0 ? (
        <Empty icon={<BellOff className="size-5" />} title={state ? `No ${state} silences` : "No silences yet"} action={state ? undefined : create}>
          {state
            ? "Change the state filter to see more."
            : "Silence an alarm from the Alarms tab, or plan a maintenance window here so planned work does not page anyone."}
        </Empty>
      ) : (
        <>
          <Table head={["State", "Scope", "Matches", "Starts", "Ends", "Reason", ""]}>
            {list.map((s) => (
              <tr key={s.id} className="cursor-pointer hover:bg-panel-2" onClick={() => onOpen(s.id)}>
                <Td>
                  <SilenceStateBadge state={s.state} />
                </Td>
                <Td className="max-w-[260px]" onClick={(e) => e.stopPropagation()}>
                  <ScopeLinks scope={s.scope} names={names} />
                </Td>
                <Td className="max-w-[200px] truncate text-xs text-muted" title={`${rulesText(s.rules, names)} · ${severityText(s)}`}>
                  {rulesText(s.rules, names)} · {severityText(s)}
                </Td>
                <Td className="text-xs whitespace-nowrap text-muted" title={time(s.starts_at)}>
                  {ago(s.starts_at)}
                </Td>
                <Td className="text-xs whitespace-nowrap text-muted" title={time(s.ends_at)}>
                  {ago(s.ends_at)}
                </Td>
                <Td className="max-w-[260px] truncate text-[13px]" title={s.reason}>
                  {s.reason || "—"}
                </Td>
                <Td className="text-right whitespace-nowrap" onClick={(e) => e.stopPropagation()}>
                  <SilenceMenu silence={s} can={can} onAction={setAction} onOpen={() => onOpen(s.id)} />
                </Td>
              </tr>
            ))}
          </Table>
          {list.length >= SILENCE_LIMIT && <p className="border-t border-line px-4 py-2 text-xs text-faint">Showing the {SILENCE_LIMIT} latest. Filter by state to find others.</p>}
        </>
      )}
      {creating && <SilenceDialog onClose={() => setCreating(false)} onCreated={(id) => onOpen(id)} />}
      {action?.kind === "extend" && <ExtendDialog key={action.silence.id} silence={action.silence} onClose={() => setAction(null)} />}
      {action?.kind === "end" && <EndConfirm key={action.silence.id} silence={action.silence} names={names} onClose={() => setAction(null)} />}
      {openId && <SilenceDrawer key={openId} id={openId} names={names} can={can} onAction={setAction} onClose={() => onOpen(undefined)} onOpenAlarms={onOpenAlarms} />}
    </Panel>
  );
}

function SilenceMenu({ silence: s, can, onAction, onOpen }: { silence: Silence; can: boolean; onAction: (a: { kind: "extend" | "end"; silence: Silence }) => void; onOpen?: () => void }) {
  const over = s.state === "expired";
  const why = !can ? POLICY_ROLE_HINT : over ? "This silence is over." : undefined;
  return (
    <RowMenu
      label={`Actions for silence ${short(s.id)}`}
      actions={[
        { label: "Details", icon: ChevronRight, onSelect: () => onOpen?.(), hidden: !onOpen },
        { label: "Extend…", icon: Clock, onSelect: () => onAction({ kind: "extend", silence: s }), disabled: !!why, hint: why },
        { label: s.state === "scheduled" ? "Cancel window…" : "End now…", icon: Square, onSelect: () => onAction({ kind: "end", silence: s }), disabled: !!why, hint: why, danger: true },
      ]}
    />
  );
}

// ------------------------------------------------------------------ drawer

function SilenceDrawer({
  id,
  names,
  can,
  onAction,
  onClose,
  onOpenAlarms,
}: {
  id: string;
  names: Names;
  can: boolean;
  onAction: (a: { kind: "extend" | "end"; silence: Silence }) => void;
  onClose: () => void;
  onOpenAlarms: () => void;
}) {
  const q = useSilence(id);
  const s = q.data;
  return (
    <Drawer open onOpenChange={(o) => !o && onClose()} title={s?.state === "scheduled" ? "Maintenance window" : "Silence"}>
      {q.isLoading ? (
        <Skeleton rows={4} />
      ) : q.isError || !s ? (
        <ErrorNote error={q.error ?? "The silence could not be loaded."} />
      ) : (
        <div className="space-y-4">
          <div className="flex flex-wrap items-center gap-2">
            <SilenceStateBadge state={s.state} />
            <span className="text-xs text-muted">
              {s.state === "active" ? `Holds notifications back until ${time(s.ends_at)}` : s.state === "scheduled" ? `Starts ${ago(s.starts_at)}` : `Ended ${ago(s.ends_at)}`}
            </span>
            <span className="ml-auto">
              <SilenceMenu silence={s} can={can} onAction={onAction} />
            </span>
          </div>
          <dl className="grid grid-cols-[110px_minmax(0,1fr)] gap-y-1.5 text-[13px]">
            <dt className="text-muted">Scope</dt>
            <dd className="min-w-0">
              <ScopeLinks scope={s.scope} names={names} max={20} />
            </dd>
            <dt className="text-muted">Rules</dt>
            <dd className="min-w-0 break-words">{s.rules.length === 0 ? "Every rule" : s.rules.map((r) => names.rule(r) ?? short(r)).join(", ")}</dd>
            <dt className="text-muted">Severity</dt>
            <dd>{severityText(s)}</dd>
            <dt className="text-muted">Starts</dt>
            <dd title={ago(s.starts_at)}>{time(s.starts_at)}</dd>
            <dt className="text-muted">Ends</dt>
            <dd title={ago(s.ends_at)}>{time(s.ends_at)}</dd>
            <dt className="text-muted">Reason</dt>
            <dd className="min-w-0 break-words whitespace-pre-wrap">{s.reason || "—"}</dd>
            <dt className="text-muted">Created</dt>
            <dd title={s.created_by}>
              {ago(s.created_at)} by <Mono>{who(s.created_by)}</Mono>
            </dd>
            {s.ended_by && (
              <>
                <dt className="text-muted">Ended early by</dt>
                <dd title={s.ended_by}>
                  <Mono>{who(s.ended_by)}</Mono>
                </dd>
              </>
            )}
            <dt className="text-muted">Silence id</dt>
            <dd className="flex min-w-0 items-center gap-1">
              <span className="min-w-0 truncate font-mono text-xs select-all">{s.id}</span>
              <CopyIcon text={s.id} label="Copy silence id" />
            </dd>
          </dl>
          <p className="flex gap-2 rounded-md bg-panel-2 px-3 py-2 text-xs text-muted">
            <Info className="mt-0.5 size-3.5 shrink-0" />
            <span>
              Matching alarms still fire and show on the{" "}
              <button type="button" className="rounded text-accent hover:underline" onClick={onOpenAlarms}>
                Alarms
              </button>{" "}
              tab marked as silenced. Their messages show as <b>suppressed</b> in the delivery history. When the silence ends, alarms that still fire notify within seconds.
            </span>
          </p>
        </div>
      )}
    </Drawer>
  );
}

// ------------------------------------------------------------------ end and extend

function EndConfirm({ silence: s, names, onClose }: { silence: Silence; names: Names; onClose: () => void }) {
  const end = useEndSilence();
  const toast = useToast();
  const scheduled = s.state === "scheduled";
  return (
    <Confirm
      open
      onOpenChange={(o) => !o && onClose()}
      title={scheduled ? "Cancel this maintenance window?" : "End this silence now?"}
      confirmLabel={scheduled ? "Cancel window" : "End silence"}
      cancelLabel={scheduled ? "Keep window" : "Keep silence"}
      busy={end.isPending}
      onConfirm={() =>
        end.mutate(s.id, {
          onSuccess: () => {
            toast(scheduled ? "Maintenance window cancelled" : "Silence ended");
            onClose();
          },
          onError: (e) => toast(actionError(e), "error"),
        })
      }
      body={
        <div className="space-y-2 text-[13px]">
          <div className="flex min-w-0 items-center gap-1 text-fg">
            <ScopeLinks scope={s.scope} names={names} />
          </div>
          {scheduled ? (
            <p>The window never starts. Alarms in its scope notify as usual.</p>
          ) : (
            <p>Notifications resume now. Alarms in this scope that are still firing send their messages within seconds, so expect a burst on your channels.</p>
          )}
          <p className="text-xs">The silence stays in the list as history.</p>
        </div>
      }
    />
  );
}

const EXTEND_STEPS = [
  { label: "+1 hour", secs: 3600 },
  { label: "+4 hours", secs: 4 * 3600 },
  { label: "+1 day", secs: 86_400 },
];

function ExtendDialog({ silence: s, onClose }: { silence: Silence; onClose: () => void }) {
  const ext = useExtendSilence();
  const toast = useToast();
  const [form, setForm] = useState<{ mode: "step" | "until"; secs: number; until: string }>({ mode: "step", secs: 3600, until: toLocal(s.ends_at + 4 * HOUR) });
  const newEnd = form.mode === "step" ? s.ends_at + form.secs * 1000 : fromLocal(form.until);
  const maxEnd = s.starts_at + SILENCE_LIMITS.maxSecs * 1000;
  const problem = !Number.isFinite(newEnd) ? "Choose a valid date and time." : newEnd <= s.ends_at ? "The new end is after the current one." : newEnd > maxEnd ? `A silence lasts at most 30 days from its start (until ${time(maxEnd)}).` : null;
  return (
    <Modal open onOpenChange={(o) => !o && onClose()} title="Extend silence" description={`It now ends ${time(s.ends_at)} (${ago(s.ends_at)}).`}>
      <form
        className="space-y-3"
        onSubmit={(e) => {
          e.preventDefault();
          if (problem) return;
          ext.mutate(form.mode === "step" ? { id: s.id, extend_secs: form.secs } : { id: s.id, ends_at: newEnd }, {
            onSuccess: (r) => {
              toast(`Silence extended to ${time(r.ends_at)}`);
              onClose();
            },
          });
        }}
      >
        <Group label="Extend by">
          <div className="flex flex-wrap gap-1">
            {EXTEND_STEPS.map((x) => (
              <Button key={x.secs} type="button" aria-pressed={form.mode === "step" && form.secs === x.secs} variant={form.mode === "step" && form.secs === x.secs ? "primary" : "ghost"} onClick={() => setForm((f) => ({ ...f, mode: "step", secs: x.secs }))}>
                {x.label}
              </Button>
            ))}
            <Button type="button" aria-pressed={form.mode === "until"} variant={form.mode === "until" ? "primary" : "ghost"} onClick={() => setForm((f) => ({ ...f, mode: "until" }))}>
              Until…
            </Button>
          </div>
        </Group>
        {form.mode === "until" && (
          <Field label="New end">
            <Input type="datetime-local" value={form.until} onChange={(e) => setForm((f) => ({ ...f, until: e.target.value }))} />
          </Field>
        )}
        <p className="text-xs text-muted">
          New end: <span className="text-fg">{Number.isFinite(newEnd) ? time(newEnd) : "—"}</span>
        </p>
        {problem && <p className="text-xs text-bad">{problem}</p>}
        {ext.isError && <FormError>{actionError(ext.error)}</FormError>}
        <div className="flex justify-end gap-2">
          <Button type="button" variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" disabled={!!problem || ext.isPending}>
            {ext.isPending && <Loader2 className="size-4 animate-spin" />}
            Extend
          </Button>
        </div>
      </form>
    </Modal>
  );
}

// ------------------------------------------------------------------ create

/** `datetime-local` value in the browser's zone. */
function toLocal(ms: number): string {
  const d = new Date(ms);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}T${p(d.getHours())}:${p(d.getMinutes())}`;
}
const fromLocal = (v: string) => (v ? new Date(v).getTime() : Number.NaN);

type Unit = "minutes" | "hours" | "days";
const UNIT_SECS: Record<Unit, number> = { minutes: 60, hours: 3600, days: 86_400 };
const SEV_RANK: Record<Severity, number> = { info: 0, warning: 1, major: 2, critical: 3 };

interface SilenceForm {
  scope: SilenceScopeKind;
  cluster: string;
  environment: string;
  servers: string[];
  apps: string[];
  rules: string[];
  min: Severity | "";
  max: Severity | "";
  later: boolean;
  start: string;
  endBy: "duration" | "until";
  amount: string;
  unit: Unit;
  end: string;
  reason: string;
}

/** What the create dialog starts with: blank, or from an alarm. */
export interface SilencePrefill {
  scope?: SilenceScope;
  rules?: string[];
  reason?: string;
  /** Shown above the form: what was prefilled from. */
  from?: string;
}

/** Prefill for "Silence" on an alarm: its node or service and its rule. */
export function prefillFromAlarm(a: Alarm): SilencePrefill {
  const kind = a.resource?.kind;
  const scope: SilenceScope | undefined =
    kind === "service" ? { kind: "services", ids: [a.resource.id] } : a.server || kind === "server" ? { kind: "servers", ids: [a.server ?? a.resource.id] } : undefined;
  return { scope, rules: a.rule_id ? [a.rule_id] : [], from: a.title };
}

function initial(p: SilencePrefill | undefined): SilenceForm {
  const now = Date.now();
  const sc = p?.scope;
  return {
    scope: sc?.kind ?? "servers",
    cluster: sc?.kind === "cluster" ? sc.id : "",
    environment: sc?.kind === "environment" ? sc.id : "",
    servers: sc?.kind === "servers" ? sc.ids : [],
    apps: sc?.kind === "services" ? sc.ids : [],
    rules: p?.rules ?? [],
    min: "",
    max: "",
    later: false,
    // The next full hour, a common start for planned work.
    start: toLocal(Math.ceil((now + 5 * 60_000) / HOUR) * HOUR),
    endBy: "duration",
    amount: "1",
    unit: "hours",
    end: toLocal(Math.ceil(now / HOUR) * HOUR + 3 * HOUR),
    reason: p?.reason ?? "",
  };
}

function scopeOf(f: SilenceForm): SilenceScope | string {
  switch (f.scope) {
    case "workspace":
      return { kind: "workspace" };
    case "cluster":
      return f.cluster ? { kind: "cluster", id: f.cluster } : "Choose a cluster.";
    case "environment":
      return f.environment ? { kind: "environment", id: f.environment } : "Choose an environment.";
    case "servers":
      if (f.servers.length === 0) return "Choose at least one node.";
      return f.servers.length > SILENCE_LIMITS.scopeIds ? `At most ${SILENCE_LIMITS.scopeIds} nodes.` : { kind: "servers", ids: f.servers };
    case "services":
      if (f.apps.length === 0) return "Choose at least one service.";
      return f.apps.length > SILENCE_LIMITS.scopeIds ? `At most ${SILENCE_LIMITS.scopeIds} services.` : { kind: "services", ids: f.apps };
  }
}

/** The request body and its window, or the first thing wrong with the form. */
function toBody(f: SilenceForm, now: number): { body: SilenceBody; start: number; end: number } | string {
  const scope = scopeOf(f);
  if (typeof scope === "string") return scope;
  if (f.rules.length > SILENCE_LIMITS.rules) return `A silence names at most ${SILENCE_LIMITS.rules} rules.`;
  if (f.min && f.max && SEV_RANK[f.min] > SEV_RANK[f.max]) return "The lowest severity is milder than or equal to the highest.";
  const reason = f.reason.trim();
  if (!reason) return "Give a reason; it shows on silenced alarms and in the audit log.";
  if (reason.length > SILENCE_LIMITS.reason) return `The reason is at most ${SILENCE_LIMITS.reason} characters.`;
  let start = now;
  if (f.later) {
    start = fromLocal(f.start);
    if (!Number.isFinite(start)) return "Choose when the window starts.";
    if (start <= now) return "A maintenance window starts in the future. Choose Now to silence from now on.";
    if (start > now + SILENCE_LIMITS.aheadMs) return "A window starts at most a year ahead.";
  }
  const body: SilenceBody = { scope, rules: f.rules, reason, min_severity: f.min || undefined, max_severity: f.max || undefined, starts_at: f.later ? start : undefined };
  let end: number;
  if (f.endBy === "duration") {
    const secs = Math.round(Number(f.amount) * UNIT_SECS[f.unit]);
    if (!Number.isFinite(secs) || secs < SILENCE_LIMITS.minSecs || secs > SILENCE_LIMITS.maxSecs) return "The duration is between 1 minute and 30 days.";
    body.duration_secs = secs;
    end = start + secs * 1000;
  } else {
    end = fromLocal(f.end);
    if (!Number.isFinite(end)) return "Choose when it ends.";
    if (end - start < SILENCE_LIMITS.minSecs * 1000) return "It ends at least 1 minute after it starts.";
    if (end - start > SILENCE_LIMITS.maxSecs * 1000) return "A silence lasts at most 30 days.";
    body.ends_at = end;
  }
  return { body, start, end };
}

export function SilenceDialog({ prefill, onClose, onCreated }: { prefill?: SilencePrefill; onClose: () => void; onCreated?: (id: string) => void }) {
  const create = useCreateSilence();
  const toast = useToast();
  const [form, setForm] = useState<SilenceForm>(() => initial(prefill));
  const set = (patch: Partial<SilenceForm>) => setForm((f) => ({ ...f, ...patch }));
  const names = useNames(form.scope);
  const { clusters, environments, nodes, apps, rules } = names.queries;
  const out = toBody(form, Date.now());

  return (
    <Modal open onOpenChange={(o) => !o && onClose()} wide title={form.later ? "New maintenance window" : "New silence"} description="Holds back the notifications of matching alarms. Alarms are still evaluated and listed.">
      <form
        className="space-y-3"
        onSubmit={(e) => {
          e.preventDefault();
          // Validate again against the clock at submit time.
          const now = toBody(form, Date.now());
          if (typeof now === "string") return;
          create.mutate(now.body, {
            onSuccess: (s) => {
              toast(s.state === "scheduled" ? `Maintenance window planned for ${time(s.starts_at)}` : `Silenced until ${time(s.ends_at)}`);
              onClose();
              if (s.id) onCreated?.(s.id);
            },
          });
        }}
      >
        {prefill?.from && (
          <p className="truncate rounded-md bg-panel-2 px-3 py-1.5 text-xs text-muted" title={prefill.from}>
            From alarm <span className="text-fg">{prefill.from}</span>: its resource and rule are filled in.
          </p>
        )}
        <div className="grid gap-3 sm:grid-cols-[180px_minmax(0,1fr)]">
          <Field label="Scope">
            <Select value={form.scope} onChange={(e) => set({ scope: e.target.value as SilenceScopeKind })}>
              <option value="servers">Nodes</option>
              <option value="services">Services</option>
              <option value="environment">Environment</option>
              <option value="cluster">Cluster</option>
              <option value="workspace">Whole workspace</option>
            </Select>
          </Field>
          {form.scope === "cluster" && (
            <Field label="Cluster" hint="Alarms on its nodes and on services that run on them.">
              {clusters.isError ? (
                <FormError>Could not load clusters: {actionError(clusters.error)}</FormError>
              ) : (
                <SearchSelect aria-label="Cluster" value={form.cluster} onChange={(e) => set({ cluster: e.target.value })}>
                  <option value="">{clusters.isLoading ? "Loading…" : "Choose a cluster"}</option>
                  {(clusters.data ?? []).map((c) => (
                    <option key={c.id} value={c.id}>
                      {c.name}
                    </option>
                  ))}
                </SearchSelect>
              )}
            </Field>
          )}
          {form.scope === "environment" && (
            <Field label="Environment" hint="Alarms on its services and nodes.">
              {environments.isError ? (
                <FormError>Could not load environments: {actionError(environments.error)}</FormError>
              ) : (
                <SearchSelect aria-label="Environment" value={form.environment} onChange={(e) => set({ environment: e.target.value })}>
                  <option value="">{environments.isLoading ? "Loading…" : "Choose an environment"}</option>
                  {(environments.data ?? []).map((n) => (
                    <option key={n.id} value={n.id}>
                      {names.env(n.id) ?? n.name}
                    </option>
                  ))}
                </SearchSelect>
              )}
            </Field>
          )}
          {form.scope === "servers" && (
            <Group label="Nodes" hint="Alarms on these nodes and on services that run on them.">
              <PickList items={(nodes.data ?? []).map((n) => ({ id: n.id, label: nodeName(n) }))} ids={form.servers} onChange={(ids) => set({ servers: ids })} loading={nodes.isLoading} error={nodes.error} empty="No nodes yet." />
            </Group>
          )}
          {form.scope === "services" && (
            <Group label="Services" hint="Alarms on these services.">
              <PickList items={(apps.data ?? []).map((a) => ({ id: a.id, label: a.name }))} ids={form.apps} onChange={(ids) => set({ apps: ids })} loading={apps.isLoading} error={apps.error} empty="No services yet." />
            </Group>
          )}
          {form.scope === "workspace" && (
            <p className="self-end rounded-md bg-warn-soft px-3 py-2 text-xs text-warn">Every alarm of the workspace stays quiet. Needs alert.manage on the workspace.</p>
          )}
        </div>
        <Group label="Rules" hint="None selected: every rule.">
          <PickList items={(rules.data ?? []).map((r) => ({ id: r.id, label: r.name }))} ids={form.rules} onChange={(ids) => set({ rules: ids })} loading={rules.isLoading} error={rules.error} empty="No alert rules yet." />
        </Group>
        <div className="grid gap-3 sm:grid-cols-2">
          <Field label="Lowest severity" hint="Only alarms this severe or worse.">
            <Select value={form.min} onChange={(e) => set({ min: e.target.value as Severity | "" })}>
              <option value="">Any</option>
              {[...SEVERITIES].reverse().map((s) => (
                <option key={s} value={s}>
                  {s}
                </option>
              ))}
            </Select>
          </Field>
          <Field label="Highest severity" hint="Worse alarms still notify.">
            <Select value={form.max} onChange={(e) => set({ max: e.target.value as Severity | "" })}>
              <option value="">Any</option>
              {SEVERITIES.map((s) => (
                <option key={s} value={s}>
                  {s}
                </option>
              ))}
            </Select>
          </Field>
        </div>
        <div className="grid gap-3 sm:grid-cols-2">
          <Group label="Starts">
            <div className="flex flex-wrap items-center gap-1">
              <Button type="button" aria-pressed={!form.later} variant={!form.later ? "primary" : "ghost"} onClick={() => set({ later: false })}>
                Now
              </Button>
              <Button type="button" aria-pressed={form.later} variant={form.later ? "primary" : "ghost"} onClick={() => set({ later: true })}>
                <CalendarClock className="size-4" /> Later
              </Button>
              {form.later && (
                <div className="min-w-0 flex-1">
                  <Input aria-label="Start" type="datetime-local" value={form.start} onChange={(e) => set({ start: e.target.value })} />
                </div>
              )}
            </div>
          </Group>
          <Group label="Ends">
            <div className="flex flex-wrap items-center gap-1">
              <Select aria-label="End by" className="w-28" value={form.endBy} onChange={(e) => set({ endBy: e.target.value as SilenceForm["endBy"] })}>
                <option value="duration">After</option>
                <option value="until">At</option>
              </Select>
              {form.endBy === "duration" ? (
                <>
                  <div className="w-20">
                    <Input aria-label="Duration" inputMode="decimal" value={form.amount} onChange={(e) => set({ amount: e.target.value })} />
                  </div>
                  <Select aria-label="Duration unit" className="w-28" value={form.unit} onChange={(e) => set({ unit: e.target.value as Unit })}>
                    <option value="minutes">minutes</option>
                    <option value="hours">hours</option>
                    <option value="days">days</option>
                  </Select>
                </>
              ) : (
                <div className="min-w-0 flex-1">
                  <Input aria-label="End" type="datetime-local" value={form.end} onChange={(e) => set({ end: e.target.value })} />
                </div>
              )}
            </div>
          </Group>
        </div>
        <Field label="Reason" hint={`Shown on silenced alarms and kept in the audit log. ${form.reason.length}/${SILENCE_LIMITS.reason}`}>
          <Input value={form.reason} maxLength={SILENCE_LIMITS.reason} onChange={(e) => set({ reason: e.target.value })} placeholder="Kernel upgrade on node-1" autoFocus={!!prefill} />
        </Field>
        {typeof out !== "string" && (
          <div className="rounded-md border border-line px-3 py-2 text-xs text-muted">
            <p className="mb-1 font-medium text-fg">What happens</p>
            <ul className="list-disc space-y-0.5 pl-4">
              <li>
                {form.later ? `From ${time(out.start)}` : "From now"} until {time(out.end)}, matching alarms send no messages. Their deliveries show as <b>suppressed</b>.
              </li>
              <li>Alarms are still evaluated, recorded and listed, marked as silenced.</li>
              <li>When it ends (or you end it early), alarms that still fire notify within seconds.</li>
            </ul>
          </div>
        )}
        {typeof out === "string" && <p className="text-xs text-bad">{out}</p>}
        {create.isError && <FormError>{actionError(create.error)}</FormError>}
        <div className="flex justify-end gap-2">
          <Button type="button" variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" disabled={typeof out === "string" || create.isPending}>
            {create.isPending ? <Loader2 className="size-4 animate-spin" /> : <BellOff className="size-4" />}
            {form.later ? "Plan window" : "Silence"}
          </Button>
        </div>
      </form>
    </Modal>
  );
}

/** `14:30` today, `Nov 18, 14:30` on another day. */
function untilText(ms: number): string {
  const d = new Date(ms);
  const hm = d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  return d.toDateString() === new Date().toDateString() ? hm : `${d.toLocaleDateString([], { month: "short", day: "numeric" })}, ${hm}`;
}

/** "Silenced until …" chip for an alarm, linking to its silence. */
export function SilencedMarker({ until, reason, onOpen }: { until: number; reason: string; onOpen: () => void }) {
  return (
    <button
      type="button"
      className="inline-flex max-w-full items-center gap-1 rounded bg-panel-2 px-1.5 py-0.5 text-xs font-medium whitespace-nowrap text-muted hover:text-accent"
      title={`Silenced until ${time(until)}${reason ? ` · ${reason}` : ""}`}
      onClick={(e) => {
        e.stopPropagation();
        onOpen();
      }}
    >
      <BellOff className="size-3.5 shrink-0" />
      <span className="truncate">silenced until {untilText(until)}</span>
    </button>
  );
}
