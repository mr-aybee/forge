import { AlertTriangle, BellRing, Hash, Loader2, Mail, Pencil, Plus, Power, Trash2, Users, Webhook } from "lucide-react";
import { type ReactNode, useState } from "react";

import { type RuleScope, type Severity, useAlertRules } from "../../api/alerts";
import { useServices, useServers, useUsers } from "../../api/hooks";
import { type Channel, type ChannelKind, EMAIL_ROLES, NOTIFY_LIMITS, type Policy, type PolicyBody, type PolicyEvent, useChannels, useDeletePolicy, usePolicies, useSavePolicy } from "../../api/notifications";
import { type Role, type User, nodeName } from "../../api/types";
import { useToast } from "../../app/toast";
import { Badge, Button, Confirm, Empty, ErrorNote, Field, Input, Modal, Panel, RowMenu, Select, Skeleton, Switch, Table, Td, short } from "../ui";
import { FormError, Group, POLICY_ROLE_HINT, PickList, SEVERITIES, SEV_TONE, actionError, useNotificationRights } from "./parts";

type Unit = "minutes" | "hours" | "days";
const UNIT_SECS: Record<Unit, number> = { minutes: 60, hours: 3600, days: 86_400 };

/** 900 -> "every 15 min", 7200 -> "every 2 h"; `undefined` when reminders are off. */
function interval(secs: number | undefined): string | undefined {
  if (!secs) return undefined;
  if (secs % 86_400 === 0) return `every ${secs / 86_400} d`;
  if (secs % 3600 === 0) return `every ${secs / 3600} h`;
  return `every ${Math.round(secs / 60)} min`;
}

const EVENT_LABEL: Record<PolicyEvent, string> = { fired: "Fired", resolved: "Resolved" };

function scopeText(s: RuleScope | undefined): string {
  if (!s) return "all resources";
  const what = s.kind === "servers" ? "nodes" : "services";
  const n = s.ids?.length ?? 0;
  return n ? `${n} ${n === 1 ? what.slice(0, -1) : what}` : `all ${what}`;
}

function rulesText(p: Policy): string {
  const n = p.rules.length;
  return n ? `${n} ${n === 1 ? "rule" : "rules"}` : "every rule";
}

const ROLE_LABEL: Record<Role, string> = {
  platform_admin: "Platform admin",
  workspace_admin: "Workspace admin",
  operator: "Operator",
  developer: "Developer",
  dba: "DBA",
  auditor: "Auditor",
  customer: "Customer",
};

/** "critical only", "major and worse", "any severity". */
function severityText(s: Severity): string {
  if (s === "critical") return "critical only";
  if (s === "info") return "any severity";
  return `${s} and worse`;
}

const KIND_ICON: Record<ChannelKind, typeof Webhook> = { webhook: Webhook, slack: Hash, email: Mail };
const KIND_LABEL: Record<ChannelKind, string> = { webhook: "Webhook", slack: "Slack", email: "Email" };

/** The request body that saves `p` unchanged except for `patch`. */
function bodyOf(p: Policy, patch: Partial<PolicyBody>): PolicyBody {
  return {
    name: p.name,
    enabled: p.enabled,
    min_severity: p.min_severity,
    rules: p.rules,
    scope: p.scope,
    events: p.events,
    repeat_interval_secs: p.repeat_interval_secs,
    channels: p.channels,
    email_channel: p.email_channel,
    email_users: p.email_users,
    email_roles: p.email_roles,
    ...patch,
  };
}

const chip = "inline-flex max-w-[200px] min-w-0 items-center gap-1 rounded border px-1.5 py-px text-xs";

function Pill({ children, title }: { children: ReactNode; title?: string }) {
  return (
    <span title={title} className="inline-flex items-center rounded border border-line px-1.5 py-px text-[11px] whitespace-nowrap text-muted">
      {children}
    </span>
  );
}

/** One chip per channel, one for the people the policy emails, and a warning chip for named users without an address. */
function Destinations({ p, channels, users }: { p: Policy; channels: Channel[] | undefined; users: User[] | undefined }) {
  const userName = (id: string) => {
    const u = users?.find((x) => x.id === id);
    return u ? u.display_name || u.username || u.email || short(id) : short(id);
  };
  const nUsers = p.email_users.length;
  const people = [nUsers ? `${nUsers} ${nUsers === 1 ? "person" : "people"}` : "", ...p.email_roles.map((r) => (ROLE_LABEL[r] ?? r).toLowerCase())].filter(Boolean);
  const via = p.email_channel ? (channels?.find((c) => c.id === p.email_channel)?.name ?? short(p.email_channel)) : undefined;
  const peopleTitle = [
    via ? `Emailed through ${via}` : "",
    nUsers ? `Users: ${p.email_users.map(userName).join(", ")}` : "",
    p.email_roles.length ? `Roles: ${p.email_roles.map((r) => ROLE_LABEL[r] ?? r).join(", ")}` : "",
  ]
    .filter(Boolean)
    .join("\n");
  const missing = p.warnings.users_without_email;
  if (p.channels.length === 0 && people.length === 0) return <span className="text-faint">—</span>;
  return (
    <div className="flex min-w-0 flex-wrap gap-1">
      {p.channels.map((id) => {
        const c = channels?.find((x) => x.id === id);
        const Icon = c ? KIND_ICON[c.kind] : AlertTriangle;
        return (
          <span
            key={id}
            className={`${chip} ${c ? (c.enabled ? "border-line text-fg" : "border-line text-faint") : "border-warn text-warn"}`}
            title={c ? `${KIND_LABEL[c.kind]} channel ${c.name}${c.enabled ? "" : " (turned off: sends nothing)"}${c.target ? ` → ${c.target}` : ""}` : `Channel ${id} not found`}
          >
            <Icon aria-hidden className="size-3.5 shrink-0 text-muted" />
            <span className="truncate">{c?.name ?? short(id)}</span>
            {c && !c.enabled && <span className="text-[11px]">(off)</span>}
          </span>
        );
      })}
      {people.length > 0 && (
        <span className={`${chip} border-line text-fg`} title={peopleTitle}>
          <Users aria-hidden className="size-3.5 shrink-0 text-muted" />
          <span className="truncate">{people.join(" · ")}</span>
        </span>
      )}
      {missing.length > 0 && (
        <span className={`${chip} border-transparent bg-warn-soft text-warn`} title={`No email address, skipped: ${missing.map(userName).join(", ")}`}>
          <AlertTriangle aria-hidden className="size-3.5 shrink-0" />
          <span className="truncate">{missing.length} without email</span>
        </span>
      )}
    </div>
  );
}

/** Policies: which alarms notify which channels. */
export function PoliciesTab({ onOpenChannels }: { onOpenChannels: () => void }) {
  const q = usePolicies();
  const channels = useChannels();
  const users = useUsers();
  const can = useNotificationRights().policies;
  const del = useDeletePolicy();
  const toggle = useSavePolicy();
  const toast = useToast();
  const [edit, setEdit] = useState<Policy | "new" | null>(null);
  const [deleting, setDeleting] = useState<Policy | null>(null);
  const list = q.data ?? [];
  const channelName = (id: string) => channels.data?.find((c) => c.id === id)?.name ?? short(id);
  const hint = can ? undefined : POLICY_ROLE_HINT;
  const setEnabled = (p: Policy, enabled: boolean) =>
    toggle.mutate(
      { id: p.id, body: bodyOf(p, { enabled }) },
      {
        onSuccess: () => toast(`Policy "${p.name}" turned ${enabled ? "on" : "off"}`),
        onError: (e) => toast(actionError(e), "error"),
      },
    );

  const create = (
    <Button variant="primary" disabled={!can} title={hint} onClick={() => setEdit("new")}>
      <Plus className="size-4" /> New policy
    </Button>
  );

  return (
    <Panel title="Policies" description="A policy sends alarms of a minimum severity to one or more channels, when they fire, when they resolve, or both." actions={create}>
      {q.isLoading ? (
        <Skeleton rows={3} />
      ) : q.isError ? (
        <ErrorNote error={q.error} />
      ) : list.length === 0 ? (
        <Empty icon={<BellRing className="size-5" />} title="No policies yet" action={can ? create : undefined}>
          A policy decides which alarms send a message to which channels. Without one, alarms only show in the console. {can ? "" : POLICY_ROLE_HINT}
        </Empty>
      ) : (
        <Table head={["Name", "Severity", "Events", "Destinations", "Enabled", ""]}>
          {list.map((p) => {
            const remind = interval(p.repeat_interval_secs);
            const busy = toggle.isPending && toggle.variables?.id === p.id;
            const matches = `${rulesText(p)} · ${scopeText(p.scope)}`;
            return (
              <tr key={p.id} className={can ? "cursor-pointer hover:bg-panel-2" : undefined} onClick={can ? () => setEdit(p) : undefined}>
                <Td className="max-w-[240px] min-w-[140px]">
                  <div className={`truncate text-[13px] font-medium ${p.enabled ? "" : "text-muted"}`} title={p.name}>
                    {p.name}
                  </div>
                  <div className="truncate text-xs text-muted" title={matches}>
                    {matches}
                  </div>
                </Td>
                <Td className="whitespace-nowrap">
                  <span title={`Alarms of severity ${p.min_severity} and worse`}>
                    <Badge tone={SEV_TONE[p.min_severity] ?? "muted"}>{severityText(p.min_severity)}</Badge>
                  </span>
                </Td>
                <Td className="min-w-[110px]">
                  <div className="flex flex-wrap items-center gap-1">
                    {p.events.length === 0 && <span className="text-faint">—</span>}
                    {p.events.map((e) => (
                      <Pill key={e}>{EVENT_LABEL[e] ?? e}</Pill>
                    ))}
                    {remind ? (
                      <Pill title="Repeats the message while the alarm fires, until it is acknowledged or resolved">Reminder {remind}</Pill>
                    ) : (
                      <span className="text-[11px] whitespace-nowrap text-faint">no reminder</span>
                    )}
                  </div>
                </Td>
                <Td className="max-w-[360px] min-w-[160px]">
                  <Destinations p={p} channels={channels.data} users={users.data} />
                </Td>
                <Td className="whitespace-nowrap" onClick={(e) => e.stopPropagation()}>
                  <span className="inline-flex items-center gap-1.5">
                    <Switch
                      checked={p.enabled}
                      disabled={!can || busy}
                      title={hint ?? (p.enabled ? "Turn the policy off" : "Turn the policy on")}
                      onChange={(on) => setEnabled(p, on)}
                      label={<span className={`text-xs ${p.enabled ? "text-ok" : "text-muted"}`}>{p.enabled ? "On" : "Off"}</span>}
                    />
                    {busy && <Loader2 aria-hidden className="size-3.5 animate-spin text-muted" />}
                  </span>
                </Td>
                <Td className="w-10 text-right whitespace-nowrap" onClick={(e) => e.stopPropagation()}>
                  <RowMenu
                    label={`Actions for policy ${p.name}`}
                    actions={[
                      { label: "Edit…", icon: Pencil, onSelect: () => setEdit(p), disabled: !can, hint },
                      { label: p.enabled ? "Turn off" : "Turn on", icon: Power, onSelect: () => setEnabled(p, !p.enabled), disabled: !can || busy, hint },
                      {
                        label: "Delete…",
                        icon: Trash2,
                        danger: true,
                        disabled: !can,
                        hint,
                        onSelect: () => {
                          del.reset();
                          setDeleting(p);
                        },
                      },
                    ]}
                  />
                </Td>
              </tr>
            );
          })}
        </Table>
      )}
      {edit && (
        <PolicyDialog
          policy={edit === "new" ? null : edit}
          onClose={() => setEdit(null)}
          onOpenChannels={() => {
            setEdit(null);
            onOpenChannels();
          }}
        />
      )}
      {deleting && (
        <Confirm
          open
          onOpenChange={(o) => !o && setDeleting(null)}
          title={`Delete the policy "${deleting.name}"?`}
          confirmLabel="Delete policy"
          busy={del.isPending}
          onConfirm={() =>
            del.mutate(deleting.id, {
              onSuccess: () => {
                toast("Policy deleted");
                setDeleting(null);
              },
            })
          }
          body={
            <div className="space-y-2">
              <p>
                Alarms it matched no longer send messages to {deleting.channels.map(channelName).join(", ") || "its channels"} through this policy. Alarms, rules and the delivery history stay.
              </p>
              {del.isError && <FormError>{actionError(del.error)}</FormError>}
            </div>
          }
        />
      )}
    </Panel>
  );
}

interface PolicyForm {
  name: string;
  enabled: boolean;
  min_severity: Severity;
  events: PolicyEvent[];
  channels: string[];
  rules: string[];
  scope: "all" | RuleScope["kind"];
  scopeIds: string[];
  remind: boolean;
  every: string;
  unit: Unit;
  emailChannel: string;
  emailUsers: string[];
  emailRoles: Role[];
}

function initial(p: Policy | null): PolicyForm {
  const secs = p?.repeat_interval_secs;
  const unit: Unit = !secs ? "minutes" : secs % 86_400 === 0 ? "days" : secs % 3600 === 0 ? "hours" : "minutes";
  return {
    name: p?.name ?? "",
    enabled: p?.enabled ?? true,
    min_severity: p?.min_severity ?? "critical",
    events: p?.events ?? ["fired", "resolved"],
    channels: p?.channels ?? [],
    rules: p?.rules ?? [],
    scope: p?.scope?.kind ?? "all",
    scopeIds: p?.scope?.ids ?? [],
    remind: !!secs,
    every: secs ? String(Math.round((secs / UNIT_SECS[unit]) * 100) / 100) : "30",
    unit,
    emailChannel: p?.email_channel ?? "",
    emailUsers: p?.email_users ?? [],
    emailRoles: p?.email_roles ?? [],
  };
}

/** The request body, or the first thing wrong with the form. */
function toBody(f: PolicyForm): PolicyBody | string {
  if (!f.name.trim()) return "Name the policy.";
  if (f.events.length === 0) return "Choose at least one event.";
  const people = f.emailUsers.length > 0 || f.emailRoles.length > 0;
  if (f.channels.length === 0 && !people) return "Choose at least one channel, or people to email.";
  if (people && !f.emailChannel) return "Choose the email channel that sends to the people.";
  if (f.emailUsers.length > NOTIFY_LIMITS.policyEmailUsers) return `A policy emails at most ${NOTIFY_LIMITS.policyEmailUsers} people by name.`;
  if (f.emailRoles.length > NOTIFY_LIMITS.policyEmailRoles) return `A policy emails at most ${NOTIFY_LIMITS.policyEmailRoles} roles.`;
  if (f.channels.length > NOTIFY_LIMITS.policyChannels) return `A policy has at most ${NOTIFY_LIMITS.policyChannels} channels.`;
  if (f.rules.length > NOTIFY_LIMITS.policyRules) return `A policy filters on at most ${NOTIFY_LIMITS.policyRules} rules.`;
  if (f.scope !== "all" && f.scopeIds.length > NOTIFY_LIMITS.scopeIds) return `A scope lists at most ${NOTIFY_LIMITS.scopeIds} resources.`;
  let repeat: number | undefined;
  if (f.remind) {
    if (!f.events.includes("fired")) return "Reminders need the Fired event.";
    repeat = Math.round(Number(f.every) * UNIT_SECS[f.unit]);
    if (!Number.isFinite(repeat) || repeat < NOTIFY_LIMITS.repeatMin || repeat > NOTIFY_LIMITS.repeatMax) return "The reminder interval is between 5 minutes and 7 days.";
  }
  return {
    name: f.name.trim(),
    enabled: f.enabled,
    min_severity: f.min_severity,
    rules: f.rules,
    scope: f.scope === "all" ? undefined : { kind: f.scope, ids: f.scopeIds },
    events: f.events,
    repeat_interval_secs: repeat,
    channels: f.channels,
    email_channel: people ? f.emailChannel : undefined,
    email_users: people ? f.emailUsers : [],
    email_roles: people ? f.emailRoles : [],
  };
}

/** A searchable list of users to email by name; users without an address carry a warning chip. */
function UserPicker({ ids, onChange, missing }: { ids: string[]; onChange: (ids: string[]) => void; missing: string[] }) {
  const users = useUsers();
  const [search, setSearch] = useState("");
  if (users.isLoading) return <Skeleton rows={1} />;
  const all = users.data ?? [];
  const needle = search.trim().toLowerCase();
  const gone = ids.filter((id) => !all.some((u) => u.id === id));
  const shown = all.filter((u) => !u.disabled || ids.includes(u.id)).filter((u) => !needle || [u.display_name, u.username, u.email].some((x) => x?.toLowerCase().includes(needle)));
  const noEmail = (id: string) => missing.includes(id) || all.some((u) => u.id === id && !u.email);
  return (
    <div className="space-y-1.5">
      {users.isError && <FormError>Could not load the users: {actionError(users.error)}. Users chosen before stay.</FormError>}
      <Input aria-label="Search users" placeholder="Search by name, username or email" value={search} onChange={(e) => setSearch(e.target.value)} />
      <div className="flex max-h-32 flex-wrap gap-x-3 gap-y-2 overflow-auto">
        {[...shown.map((u) => ({ id: u.id, label: u.display_name || u.username || u.email || short(u.id) })), ...gone.map((id) => ({ id, label: `${short(id)} (not found)` }))].map((u) => (
          <Switch
            key={u.id}
            checked={ids.includes(u.id)}
            onChange={(on) => onChange(on ? [...ids, u.id] : ids.filter((x) => x !== u.id))}
            label={
              <span className="inline-flex items-center gap-1.5">
                {u.label}
                {noEmail(u.id) && <Badge tone="warn">no email</Badge>}
              </span>
            }
          />
        ))}
        {shown.length === 0 && gone.length === 0 && <p className="text-xs text-faint">{needle ? "No user matches." : "No users yet."}</p>}
      </div>
    </div>
  );
}

function PolicyDialog({ policy, onClose, onOpenChannels }: { policy: Policy | null; onClose: () => void; onOpenChannels: () => void }) {
  const save = useSavePolicy();
  const toast = useToast();
  const channels = useChannels();
  const rules = useAlertRules();
  const [form, setForm] = useState<PolicyForm>(() => initial(policy));
  const set = (patch: Partial<PolicyForm>) => setForm((f) => ({ ...f, ...patch }));
  const nodes = useServers();
  const apps = useServices(undefined, undefined, form.scope === "services");
  const body = toBody(form);
  const noChannels = channels.isSuccess && (channels.data ?? []).length === 0;
  const toggleEvent = (e: PolicyEvent, on: boolean) => set({ events: on ? [...form.events.filter((x) => x !== e), e] : form.events.filter((x) => x !== e) });

  return (
    <Modal open onOpenChange={(o) => !o && onClose()} wide title={policy ? "Edit policy" : "New policy"} description="A channel reached by two matching policies gets one message per alarm event.">
      <form
        className="space-y-3"
        onSubmit={(e) => {
          e.preventDefault();
          if (typeof body === "string") return;
          save.mutate(
            { id: policy?.id, body },
            {
              onSuccess: () => {
                toast(policy ? "Policy saved" : "Policy created");
                onClose();
              },
            },
          );
        }}
      >
        <div className="grid gap-3 sm:grid-cols-2">
          <Field label="Name">
            <Input value={form.name} maxLength={NOTIFY_LIMITS.name} onChange={(e) => set({ name: e.target.value })} autoFocus placeholder="Critical to ops" />
          </Field>
          <Field label="Minimum severity" hint="Alarms of this severity and worse.">
            <Select value={form.min_severity} onChange={(e) => set({ min_severity: e.target.value as Severity })}>
              {SEVERITIES.map((s) => (
                <option key={s} value={s}>
                  {s}
                </option>
              ))}
            </Select>
          </Field>
        </div>
        <Group label="Events" hint="Resolved also covers alarms that ended without recovery (for example, after the rule was changed).">
          <div className="flex flex-wrap gap-x-4 gap-y-2">
            <Switch checked={form.events.includes("fired")} onChange={(on) => toggleEvent("fired", on)} label="Fired" />
            <Switch checked={form.events.includes("resolved")} onChange={(on) => toggleEvent("resolved", on)} label="Resolved" />
          </div>
        </Group>
        <Group label="Channels" hint={noChannels ? undefined : `Where the messages go. Up to ${NOTIFY_LIMITS.policyChannels}; none is fine when the policy emails people.`}>
          {noChannels && form.channels.length === 0 ? (
            <div className="flex flex-wrap items-center gap-2 rounded-md border border-line px-3 py-2 text-xs text-muted">
              <span className="min-w-0 flex-1">No channel exists yet. Create one in the Channels tab first, then come back to add the policy.</span>
              <Button type="button" onClick={onOpenChannels}>
                Open Channels
              </Button>
            </div>
          ) : (
            <PickList
              items={(channels.data ?? []).map((c) => ({ id: c.id, label: c.enabled ? c.name : `${c.name} (off)` }))}
              ids={form.channels}
              onChange={(ids) => set({ channels: ids })}
              loading={channels.isLoading}
              error={channels.error}
            />
          )}
        </Group>
        <Group
          label="Email people"
          hint="Optional. Named users are emailed when they may see the alarm. A role means the people responsible for the alarm's resource: everyone with that role on the resource, its environment, cluster or workspace. People without an email address, or without access, are skipped. The email channel's own recipients are not added."
        >
          <div className="space-y-2">
            <Field label="Sent through">
              <Select value={form.emailChannel} onChange={(e) => set({ emailChannel: e.target.value })}>
                <option value="">Choose an email channel</option>
                {(channels.data ?? [])
                  .filter((c) => c.kind === "email" || c.id === form.emailChannel)
                  .map((c) => (
                    <option key={c.id} value={c.id}>
                      {c.enabled ? c.name : `${c.name} (off)`}
                    </option>
                  ))}
              </Select>
            </Field>
            <Field label="Users">
              <UserPicker ids={form.emailUsers} onChange={(emailUsers) => set({ emailUsers })} missing={policy?.warnings.users_without_email ?? []} />
            </Field>
            <Field label="Roles">
              <PickList items={EMAIL_ROLES.map((r) => ({ id: r, label: ROLE_LABEL[r] }))} ids={form.emailRoles} onChange={(ids) => set({ emailRoles: EMAIL_ROLES.filter((r) => ids.includes(r)) })} />
            </Field>
          </div>
        </Group>
        <Group label="Rules" hint="None selected: every rule.">
          <PickList
            items={(rules.data ?? []).map((r) => ({ id: r.id, label: r.name }))}
            ids={form.rules}
            onChange={(ids) => set({ rules: ids })}
            loading={rules.isLoading}
            error={rules.error}
            empty="No alert rules yet; the policy applies to every rule added later."
          />
        </Group>
        <div className="grid gap-3 sm:grid-cols-[180px_1fr]">
          <Field label="Scope">
            <Select value={form.scope} onChange={(e) => set({ scope: e.target.value as PolicyForm["scope"], scopeIds: [] })}>
              <option value="all">All resources</option>
              <option value="servers">Nodes</option>
              <option value="services">Services</option>
            </Select>
          </Field>
          {form.scope === "servers" && (
            <Group label="Nodes" hint="None selected: every node of the workspace.">
              <PickList
                items={(nodes.data ?? []).map((n) => ({ id: n.id, label: nodeName(n) }))}
                ids={form.scopeIds}
                onChange={(ids) => set({ scopeIds: ids })}
                loading={nodes.isLoading}
                error={nodes.error}
              />
            </Group>
          )}
          {form.scope === "services" && (
            <Group label="Services" hint="None selected: every service of the workspace.">
              <PickList
                items={(apps.data ?? []).map((a) => ({ id: a.id, label: a.name }))}
                ids={form.scopeIds}
                onChange={(ids) => set({ scopeIds: ids })}
                loading={apps.isLoading}
                error={apps.error}
              />
            </Group>
          )}
        </div>
        <Group label="Reminders" hint="Repeats the message while the alarm is firing. Reminders stop when the alarm is acknowledged or resolved. Between 5 minutes and 7 days.">
          <div className="flex flex-wrap items-center gap-3">
            <Switch checked={form.remind} onChange={(remind) => set({ remind })} label={form.remind ? "Every" : "Off"} />
            {form.remind && (
              <>
                <div className="w-20">
                  <Input aria-label="Reminder interval" inputMode="decimal" value={form.every} onChange={(e) => set({ every: e.target.value })} />
                </div>
                <Select aria-label="Reminder interval unit" className="w-28" value={form.unit} onChange={(e) => set({ unit: e.target.value as Unit })}>
                  <option value="minutes">minutes</option>
                  <option value="hours">hours</option>
                  <option value="days">days</option>
                </Select>
              </>
            )}
          </div>
        </Group>
        <Switch checked={form.enabled} onChange={(enabled) => set({ enabled })} label="Enabled" />
        {typeof body === "string" && <p className="text-xs text-bad">{body}</p>}
        {save.isError && <FormError>{actionError(save.error)}</FormError>}
        <div className="flex justify-end gap-2">
          <Button type="button" variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" disabled={save.isPending || typeof body === "string"}>
            {save.isPending && <Loader2 className="size-4 animate-spin" />}
            Save
          </Button>
        </div>
      </form>
    </Modal>
  );
}
