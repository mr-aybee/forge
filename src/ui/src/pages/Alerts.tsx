import { useNavigate, useSearch } from "@tanstack/react-router";
import { BellOff, Check, Pencil, Plus, Siren, Trash2 } from "lucide-react";
import { useState } from "react";

import {
  type Alarm,
  type AlertRule,
  useAcknowledge,
  useAddNote,
  useAlarm,
  useAlarms,
  useAlertRules,
  useDeleteRule,
} from "../api/alerts";
import { useServers } from "../api/hooks";
import { useDeliveries } from "../api/notifications";
import { nodeName } from "../api/types";
import type { AlertTab } from "../app/router";
import { errorText, useToast } from "../app/toast";
import { ChannelsTab } from "../components/alerts/ChannelsTab";
import { DeliveriesTab } from "../components/alerts/DeliveriesTab";
import { PoliciesTab } from "../components/alerts/PoliciesTab";
import { RuleDialog } from "../components/alerts/RuleDialog";
import { SilenceDialog, type SilencePrefill, SilencedMarker, SilencesTab, prefillFromAlarm } from "../components/alerts/SilencesTab";
import { DeliveryBadge, POLICY_ROLE_HINT, SEV_TONE, eventLabel, lastResult, useNotificationRights } from "../components/alerts/parts";
import { Badge, Button, Drawer, Empty, ErrorNote, FadeSwitch, Field, IconButton, Mono, PageHeader, Panel, Skeleton, Switch, Table, Td, TextArea, ago, short, time } from "../components/ui";

const STATE_TONE: Record<string, string> = { firing: "bad", resolved: "ok", ended: "muted" };
const value = (a: Alarm) => (a.fired_value ? String(a.fired_value.value) : "—");

const TABS: { id: AlertTab; label: string }[] = [
  { id: "alarms", label: "Alarms" },
  { id: "rules", label: "Rules" },
  { id: "policies", label: "Policies" },
  { id: "channels", label: "Channels" },
  { id: "deliveries", label: "Delivery history" },
  { id: "silences", label: "Silences" },
];

/**
 * Alerts (#11): alarms opened by rules over observations, and the
 * notifications they send (policies, channels, delivery history). An alarm
 * stays firing until recovery evidence holds; acknowledging only says someone
 * is on it. Missing data never resolves an alarm.
 *
 * The tab and the open alarm live in the URL: `?alarm=<id>` is the link in
 * notification messages.
 */
export function Alerts() {
  const { tab: tabParam, alarm, silence } = useSearch({ from: "/alerts" });
  const navigate = useNavigate();
  const tab = tabParam ?? "alarms";
  const go = (next: { tab: AlertTab; alarm?: string; silence?: string }) =>
    void navigate({ to: "/alerts", search: { tab: next.tab === "alarms" ? undefined : next.tab, alarm: next.alarm, silence: next.silence }, replace: true });
  const openAlarm = (id: string) => go({ tab, alarm: id });
  const openSilence = (id: string) => go({ tab: "silences", silence: id });
  // Mounted only while open, so its form starts from the prefill each time.
  const [silencing, setSilencing] = useState<SilencePrefill | null>(null);
  const silenceAlarm = (a: Alarm) => setSilencing(prefillFromAlarm(a));
  return (
    <>
      <PageHeader eyebrow="Operations" title="Alerts" description="Alarms opened by your rules, the policies that decide which alarms send a message, and the channels the messages go to." />
      <div className="mb-4 flex flex-wrap gap-1">
        {TABS.map((t) => (
          <Button key={t.id} aria-pressed={tab === t.id} variant={tab === t.id ? "primary" : "ghost"} onClick={() => go({ tab: t.id })}>
            {t.label}
          </Button>
        ))}
      </div>
      <FadeSwitch k={tab}>
      {tab === "alarms" ? (
        <AlarmsTab onOpen={openAlarm} onSilence={silenceAlarm} onOpenSilence={openSilence} />
      ) : tab === "rules" ? (
        <RulesTab />
      ) : tab === "policies" ? (
        <PoliciesTab onOpenChannels={() => go({ tab: "channels" })} />
      ) : tab === "channels" ? (
        <ChannelsTab />
      ) : tab === "silences" ? (
        <SilencesTab openId={silence} onOpen={(id) => go({ tab: "silences", silence: id })} onOpenAlarms={() => go({ tab: "alarms" })} />
      ) : (
        <DeliveriesTab onOpenAlarm={openAlarm} onOpenSilence={openSilence} />
      )}
      </FadeSwitch>
      {alarm && <AlarmDrawer key={alarm} id={alarm} onClose={() => go({ tab, silence })} onSilence={silenceAlarm} onOpenSilence={openSilence} />}
      {silencing && <SilenceDialog prefill={silencing} onClose={() => setSilencing(null)} onCreated={openSilence} />}
    </>
  );
}

function AlarmsTab({ onOpen, onSilence, onOpenSilence }: { onOpen: (id: string) => void; onSilence: (a: Alarm) => void; onOpenSilence: (id: string) => void }) {
  const can = useNotificationRights().policies;
  const [active, setActive] = useState(true);
  const q = useAlarms(active);
  const nodes = useServers().data ?? [];
  const name = (a: Alarm) => {
    const n = nodes.find((s) => s.id === (a.server ?? a.resource.id));
    const host = n ? nodeName(n) : (a.server ?? a.resource.id).slice(0, 8);
    // Installations and certificates (D-118) say what they are; a package is shown by its node.
    if (a.resource.kind === "installation") return `package on ${host}`;
    if (a.resource.kind === "certificate") return `certificate ${a.resource.id.slice(0, 8)}`;
    return host;
  };
  return (
    <Panel title="Alarms" actions={<Switch checked={active} onChange={setActive} label="Firing only" />}>
      {q.isLoading ? (
        <Skeleton rows={3} />
      ) : q.isError ? (
        <ErrorNote error={q.error} />
      ) : (q.data ?? []).length === 0 ? (
        <Empty icon={<Siren className="size-5" />} title={active ? "Nothing is firing" : "No alarms yet"}>
          Alarms appear here when a rule's condition holds.
        </Empty>
      ) : (
        <Table head={["State", "Severity", "Alarm", "Resource", "Value", "Fired", ""]}>
          {(q.data ?? []).map((a) => (
            <tr key={a.id} className="cursor-pointer hover:bg-panel-2" onClick={() => onOpen(a.id)}>
              <Td>
                <Badge tone={STATE_TONE[a.state] as never}>{a.state}</Badge>
                {a.acknowledged_at && a.state === "firing" && <Badge tone={"muted" as never}>acknowledged</Badge>}
                {a.silence && <SilencedMarker until={a.silence.until} reason={a.silence.reason} onOpen={() => onOpenSilence(a.silence?.silence_id ?? "")} />}
              </Td>
              <Td>
                <Badge tone={SEV_TONE[a.severity] as never}>{a.severity}</Badge>
              </Td>
              <Td className="font-medium">
                {/* The row opens on click; this button makes it reachable by keyboard. */}
                <button type="button" className="rounded text-left hover:underline" onClick={() => onOpen(a.id)}>
                  {a.title}
                </button>
              </Td>
              <Td>{name(a)}</Td>
              <Td>
                <Mono>{value(a)}</Mono>
              </Td>
              <Td className="text-xs text-muted" title={time(a.fired_at)}>
                {ago(a.fired_at)}
              </Td>
              <Td className="text-right" onClick={(e) => e.stopPropagation()}>
                {a.state === "firing" && !a.silence && (
                  <IconButton label={can ? `Silence ${a.title}` : POLICY_ROLE_HINT} title={can ? "Silence: hold back its notifications" : POLICY_ROLE_HINT} disabled={!can} onClick={() => onSilence(a)}>
                    <BellOff className="size-4" />
                  </IconButton>
                )}
              </Td>
            </tr>
          ))}
        </Table>
      )}
    </Panel>
  );
}

/** Messages sent for one alarm: event, channel and outcome, newest first. */
function AlarmNotifications({ alarmId }: { alarmId: string }) {
  const q = useDeliveries({ alarm: alarmId });
  const list = q.data ?? [];
  return (
    <div>
      <h3 className="mb-2 text-xs font-semibold text-muted uppercase">Notifications</h3>
      {q.isLoading ? (
        <Skeleton rows={2} />
      ) : q.isError ? (
        <ErrorNote error={q.error} />
      ) : list.length === 0 ? (
        <p className="text-[13px] text-muted">No message was sent for this alarm. A policy that matches an alarm sends one to its channels.</p>
      ) : (
        <ul className="divide-y divide-line rounded-md border border-line text-[13px]">
          {list.map((d) => (
            <li key={d.id} className="flex min-w-0 items-center gap-2 px-3 py-1.5">
              <DeliveryBadge status={d.status} />
              <span className="shrink-0">{eventLabel(d)}</span>
              <span className="min-w-0 flex-1 truncate text-muted" title={`${d.channel_name ?? d.channel_id}${d.policy_name ? ` · ${d.policy_name}` : ""}`}>
                → {d.channel_name ?? short(d.channel_id)}
                {d.status === "cancelled" && d.detail ? ` · ${d.detail}` : d.status === "failed" ? ` · ${lastResult(d)}` : ""}
              </span>
              <span className="shrink-0 text-xs text-muted" title={time(d.created_at)}>
                {ago(d.created_at)}
              </span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

function AlarmDrawer({ id, onClose, onSilence, onOpenSilence }: { id: string; onClose: () => void; onSilence: (a: Alarm) => void; onOpenSilence: (id: string) => void }) {
  const can = useNotificationRights().policies;
  const q = useAlarm(id);
  const ack = useAcknowledge();
  const note = useAddNote();
  const toast = useToast();
  const [text, setText] = useState("");
  const a = q.data;
  const run = (p: Promise<unknown>, done: string) =>
    p.then(() => {
      toast(done);
      setText("");
    }, (e) => toast(errorText(e), "error"));
  return (
    <Drawer open onOpenChange={(o) => !o && onClose()} title={a?.title ?? "Alarm"}>
      {q.isError ? (
        <ErrorNote error={q.error} />
      ) : !a ? (
        <Skeleton rows={4} />
      ) : (
        <div className="space-y-4 p-4">
          <div className="flex flex-wrap gap-2">
            <Badge tone={STATE_TONE[a.state] as never}>{a.state}</Badge>
            <Badge tone={SEV_TONE[a.severity] as never}>{a.severity}</Badge>
            {a.acknowledged_at && <Badge tone={"muted" as never}>acknowledged {ago(a.acknowledged_at)}</Badge>}
            {a.silence && <SilencedMarker until={a.silence.until} reason={a.silence.reason} onOpen={() => onOpenSilence(a.silence?.silence_id ?? "")} />}
          </div>
          {a.silence?.reason && (
            <p className="rounded-md bg-panel-2 px-3 py-2 text-xs text-muted">
              Notifications are held back until {time(a.silence.until)}: <span className="text-fg">{a.silence.reason}</span>
            </p>
          )}
          <dl className="grid grid-cols-[120px_1fr] gap-y-1 text-[13px]">
            <dt className="text-muted">Subject</dt>
            <dd>
              <Mono>{a.subject}</Mono>
            </dd>
            <dt className="text-muted">Value</dt>
            <dd>
              <Mono>{value(a)}</Mono>
            </dd>
            <dt className="text-muted">Fired</dt>
            <dd>{new Date(a.fired_at).toLocaleString()}</dd>
            {a.resolved_at && (
              <>
                <dt className="text-muted">{a.state === "ended" ? "Ended" : "Resolved"}</dt>
                <dd>
                  {new Date(a.resolved_at).toLocaleString()}
                  {a.end_reason && ` (${a.end_reason})`}
                </dd>
              </>
            )}
          </dl>
          {a.state === "firing" && (
            <div className="flex gap-2">
              {a.acknowledged_at ? (
                <Button variant="ghost" disabled={ack.isPending} onClick={() => run(ack.mutateAsync({ id: a.id, note: "", undo: true }), "Acknowledgement removed")}>
                  <BellOff className="size-4" /> Unacknowledge
                </Button>
              ) : (
                <Button variant="primary" disabled={ack.isPending} onClick={() => run(ack.mutateAsync({ id: a.id, note: text }), "Acknowledged")}>
                  <Check className="size-4" /> Acknowledge
                </Button>
              )}
              {!a.silence && (
                <span title={can ? "Hold back this alarm's notifications for a while" : POLICY_ROLE_HINT}>
                  <Button disabled={!can} onClick={() => onSilence(a)}>
                    <BellOff className="size-4" /> Silence
                  </Button>
                </span>
              )}
            </div>
          )}
          <Field label="Note">
            <TextArea rows={3} maxLength={2000} value={text} onChange={(e) => setText(e.target.value)} />
          </Field>
          <Button disabled={!text.trim() || note.isPending} onClick={() => run(note.mutateAsync({ id: a.id, note: text }), "Note added")}>
            Add note
          </Button>
          <div>
            <h3 className="mb-2 text-xs font-semibold text-muted uppercase">Timeline</h3>
            <ol className="space-y-2 text-[13px]">
              {(a.events ?? []).map((e, i) => (
                <li key={i} className="border-l-2 border-line pl-3">
                  <span className="font-medium">{e.kind}</span> <span className="text-xs text-muted">{new Date(e.at).toLocaleString()}</span>
                  {e.actor && <span className="text-xs text-muted"> · {e.actor}</span>}
                  {e.detail?.note && <p className="whitespace-pre-wrap">{e.detail.note}</p>}
                  {e.detail?.reason && <p className="text-muted">{e.detail.reason}</p>}
                </li>
              ))}
            </ol>
          </div>
          <AlarmNotifications alarmId={a.id} />
        </div>
      )}
    </Drawer>
  );
}

function RulesTab() {
  const q = useAlertRules();
  const del = useDeleteRule();
  const toast = useToast();
  const [edit, setEdit] = useState<AlertRule | "new" | null>(null);
  return (
    <Panel
      title="Rules"
      description="A rule watches one subject on the nodes, services, databases or certificates in its scope and opens an alarm while its condition holds. Start from a template with New rule."
      actions={
        <Button variant="primary" onClick={() => setEdit("new")}>
          <Plus className="size-4" /> New rule
        </Button>
      }
    >
      {q.isLoading ? (
        <Skeleton rows={3} />
      ) : q.isError ? (
        <ErrorNote error={q.error} />
      ) : (q.data ?? []).length === 0 ? (
        <Empty icon={<Siren className="size-5" />} title="No rules yet">
          For example: alert when a node goes offline.
        </Empty>
      ) : (
        <Table head={["Name", "Severity", "Subject", "Enabled", "Revision", ""]}>
          {(q.data ?? []).map((r) => (
            <tr key={r.id}>
              <Td className="font-medium">{r.name}</Td>
              <Td>
                <Badge tone={SEV_TONE[r.severity] as never}>{r.severity}</Badge>
              </Td>
              <Td>
                <Mono>{r.subject}</Mono>
              </Td>
              <Td>{r.enabled ? "yes" : "no"}</Td>
              <Td>{r.revision}</Td>
              <Td className="text-right whitespace-nowrap">
                <IconButton label="Edit" onClick={() => setEdit(r)}>
                  <Pencil className="size-4" />
                </IconButton>
                <IconButton
                  label="Delete"
                  onClick={() => {
                    if (confirm(`Delete the rule "${r.name}"? Its alarms stay as history.`))
                      del.mutateAsync(r.id).then(() => toast("Rule deleted"), (e) => toast(errorText(e), "error"));
                  }}
                >
                  <Trash2 className="size-4" />
                </IconButton>
              </Td>
            </tr>
          ))}
        </Table>
      )}
      {edit && <RuleDialog rule={edit === "new" ? null : edit} onClose={() => setEdit(null)} />}
    </Panel>
  );
}
