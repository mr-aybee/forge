import { BellOff, ChevronRight, Loader2, Send } from "lucide-react";
import { useState } from "react";

import { DELIVERY_LIMIT, DELIVERY_STATUSES, type Delivery, type DeliveryFilter, type DeliveryStatus, type RecipientSkip, deliveryOpen, useChannels, useDeliveries, useDelivery } from "../../api/notifications";
import { Badge, CopyIcon, Drawer, Empty, ErrorNote, IconButton, Mono, Panel, Select, Skeleton, Table, Td, ago, duration, short, time } from "../ui";
import { DeliveryBadge, attemptText, errorHelp, eventLabel, lastResult } from "./parts";

const recipientCount = (n: number) => `${n} ${n === 1 ? "recipient" : "recipients"}`;
const SKIP_LABEL: Record<RecipientSkip, string> = { no_email: "no email", no_access: "no access", disabled: "disabled", too_many: "over the limit" };

const alarmText = (d: Pick<Delivery, "event" | "alarm_id" | "alarm_title">) => (d.event === "test" ? "Test message" : (d.alarm_title ?? (d.alarm_id ? `Alarm ${short(d.alarm_id)}` : "—")));

/** Delivery history: every message a policy sent (or tried to send) to a channel, newest first. */
export function DeliveriesTab({ onOpenAlarm, onOpenSilence }: { onOpenAlarm: (id: string) => void; onOpenSilence: (id: string) => void }) {
  const [filter, setFilter] = useState<DeliveryFilter>({});
  const [open, setOpen] = useState<string | null>(null);
  const q = useDeliveries(filter);
  const channels = useChannels();
  const list = q.data ?? [];
  const filtered = !!filter.status || !!filter.channel;

  return (
    <Panel
      title="Delivery history"
      description="Each message to a channel with its attempts. Final deliveries are kept for 30 days."
      actions={
        <div className="flex flex-wrap items-center gap-2">
          {q.isFetching && !q.isLoading && <Loader2 className="size-3.5 animate-spin text-faint" aria-label="Refreshing" />}
          <Select aria-label="Status" className="w-36" value={filter.status ?? ""} onChange={(e) => setFilter((f) => ({ ...f, status: (e.target.value || undefined) as DeliveryStatus | undefined }))}>
            <option value="">All statuses</option>
            {DELIVERY_STATUSES.map((s) => (
              <option key={s} value={s}>
                {s}
              </option>
            ))}
          </Select>
          <Select aria-label="Channel" className="w-44" value={filter.channel ?? ""} onChange={(e) => setFilter((f) => ({ ...f, channel: e.target.value || undefined }))}>
            <option value="">All channels</option>
            {(channels.data ?? []).map((c) => (
              <option key={c.id} value={c.id}>
                {c.name}
              </option>
            ))}
          </Select>
        </div>
      }
    >
      {q.isLoading ? (
        <Skeleton rows={4} />
      ) : q.isError ? (
        <ErrorNote error={q.error} />
      ) : list.length === 0 ? (
        <Empty icon={<Send className="size-5" />} title={filtered ? "No deliveries match these filters" : "Nothing has been sent yet"}>
          {filtered ? "Change the status or channel filter to see more." : "When an alarm matches a policy, each message to a channel is recorded here with its attempts. Add a channel and a policy to start sending."}
        </Empty>
      ) : (
        <>
          <Table head={["Time", "Event", "Alarm", "Channel", "Policy", "Status", "Attempts", "Last error", ""]}>
            {list.map((d) => (
              <tr key={d.id} className="cursor-pointer hover:bg-panel-2" onClick={() => setOpen(d.id)}>
                <Td className="text-xs whitespace-nowrap text-muted" title={time(d.created_at)}>
                  {ago(d.created_at)}
                </Td>
                <Td className="whitespace-nowrap">{eventLabel(d)}</Td>
                <Td className="max-w-[240px] truncate" title={alarmText(d)}>
                  {d.alarm_id ? (
                    <button
                      type="button"
                      className="max-w-full truncate rounded font-medium hover:text-accent hover:underline"
                      onClick={(e) => {
                        e.stopPropagation();
                        onOpenAlarm(d.alarm_id ?? "");
                      }}
                    >
                      {alarmText(d)}
                    </button>
                  ) : (
                    <span className="text-muted">{alarmText(d)}</span>
                  )}
                </Td>
                <Td className="max-w-[180px] truncate" title={d.channel_name ?? d.channel_id}>
                  {d.channel_name ?? short(d.channel_id)}
                  {d.recipients && <span className="text-xs text-muted"> · {recipientCount(d.recipients.count)}</span>}
                </Td>
                <Td className="max-w-[180px] truncate" title={d.policy_name ?? d.policy_id}>
                  {d.policy_name ?? (d.policy_id ? short(d.policy_id) : "—")}
                </Td>
                <Td className="whitespace-nowrap">
                  <DeliveryBadge status={d.status} />
                  {d.silence_id && <SilenceLink id={d.silence_id} after={d.after_silence} onOpen={onOpenSilence} />}
                </Td>
                <Td className="font-mono text-[11px] tabular-nums">{d.attempts}</Td>
                <Td className="max-w-[200px] truncate font-mono text-[11px]" title={d.status === "cancelled" || d.status === "suppressed" ? d.detail : errorHelp(d.last_error)}>
                  {(d.status === "cancelled" || d.status === "suppressed") && d.detail ? <span className="font-sans text-xs text-muted">{d.detail}</span> : lastResult(d)}
                </Td>
                <Td className="text-right">
                  <IconButton
                    label="Details"
                    onClick={(e) => {
                      e.stopPropagation();
                      setOpen(d.id);
                    }}
                  >
                    <ChevronRight className="size-4" />
                  </IconButton>
                </Td>
              </tr>
            ))}
          </Table>
          {list.length >= DELIVERY_LIMIT && <p className="border-t border-line px-4 py-2 text-xs text-faint">Showing the newest {DELIVERY_LIMIT}. Filter by status or channel to find older ones.</p>}
        </>
      )}
      {open && (
        <DeliveryDrawer
          key={open}
          id={open}
          onClose={() => setOpen(null)}
          onOpenSilence={(id) => {
            setOpen(null);
            onOpenSilence(id);
          }}
          onOpenAlarm={(id) => {
            setOpen(null);
            onOpenAlarm(id);
          }}
        />
      )}
    </Panel>
  );
}

function DeliveryDrawer({ id, onClose, onOpenAlarm, onOpenSilence }: { id: string; onClose: () => void; onOpenAlarm: (id: string) => void; onOpenSilence: (id: string) => void }) {
  const q = useDelivery(id);
  const d = q.data;
  return (
    <Drawer open onOpenChange={(o) => !o && onClose()} title="Delivery" wide>
      {q.isLoading ? (
        <Skeleton rows={4} />
      ) : q.isError || !d ? (
        <ErrorNote error={q.error ?? "The delivery could not be loaded."} />
      ) : (
        <div className="space-y-4">
          <div className="flex flex-wrap items-center gap-2">
            <DeliveryBadge status={d.status} />
            <Badge>{eventLabel(d)}</Badge>
            {deliveryOpen(d) && <span className="text-xs text-muted">Not final yet; this view refreshes.</span>}
          </div>
          {d.status === "cancelled" && (
            <p className="rounded-md bg-panel-2 px-3 py-2 text-[13px]">
              <span className="font-medium">Cancelled:</span> {d.detail ?? "no reason recorded"}
            </p>
          )}
          {(d.status === "suppressed" || d.after_silence) && (
            <p className="rounded-md bg-panel-2 px-3 py-2 text-[13px]">
              {d.status === "suppressed" ? (
                <>
                  <span className="font-medium">Held back by a silence</span>
                  {d.detail ? `: ${d.detail}` : "."} When it ends, a message is sent if the alarm still fires.
                </>
              ) : (
                <span className="font-medium">Sent after a silence ended; it stands for the messages the silence held back.</span>
              )}{" "}
              {d.silence_id && (
                <button type="button" className="rounded text-accent hover:underline" onClick={() => onOpenSilence(d.silence_id ?? "")}>
                  Open silence
                </button>
              )}
            </p>
          )}
          <dl className="grid grid-cols-[120px_minmax(0,1fr)] gap-y-1 text-[13px]">
            <dt className="text-muted">Alarm</dt>
            <dd className="min-w-0 truncate">
              {d.alarm_id ? (
                <button type="button" className="max-w-full truncate rounded text-accent hover:underline" onClick={() => onOpenAlarm(d.alarm_id ?? "")}>
                  {alarmText(d)}
                </button>
              ) : (
                alarmText(d)
              )}
            </dd>
            {d.resource && (
              <>
                <dt className="text-muted">Resource</dt>
                <dd className="min-w-0 truncate">
                  {d.resource.kind} <Mono>{short(d.resource.id)}</Mono>
                </dd>
              </>
            )}
            <dt className="text-muted">Channel</dt>
            <dd className="min-w-0 truncate">{d.channel_name ?? short(d.channel_id)}</dd>
            <dt className="text-muted">Policy</dt>
            <dd className="min-w-0 truncate">{d.policy_name ?? (d.policy_id ? short(d.policy_id) : "—")}</dd>
            {d.recipients && (
              <>
                <dt className="text-muted">Recipients</dt>
                <dd className="min-w-0">
                  {recipientCount(d.recipients.count)}
                  {d.recipients.to.length > 0 && <div className="break-all font-mono text-[11px]">{d.recipients.to.join(", ")}</div>}
                  {d.recipients.skipped.length > 0 && (
                    <div className="text-xs text-muted">
                      Skipped:{" "}
                      {d.recipients.skipped
                        .map((s) => `${short(s.user_id)} (${SKIP_LABEL[s.reason] ?? s.reason})`)
                        .join(", ")}
                    </div>
                  )}
                </dd>
              </>
            )}
            <dt className="text-muted">Created</dt>
            <dd>{time(d.created_at)}</dd>
            <dt className="text-muted">Finished</dt>
            <dd>{time(d.finished_at)}</dd>
            {deliveryOpen(d) && d.next_attempt_at !== undefined && (
              <>
                <dt className="text-muted">Next attempt</dt>
                <dd title={time(d.next_attempt_at)}>{ago(d.next_attempt_at)}</dd>
              </>
            )}
            {(d.last_error || d.last_http_status !== undefined) && (
              <>
                <dt className="text-muted">Last error</dt>
                <dd className="min-w-0 break-words">{attemptText({ http_status: d.last_http_status, error: d.last_error })}</dd>
              </>
            )}
            <dt className="text-muted">Delivery id</dt>
            <dd className="flex min-w-0 items-center gap-1">
              <span className="min-w-0 truncate font-mono text-xs select-all" title="Sent to the receiver as X-Forge-Delivery">
                {d.id}
              </span>
              <CopyIcon text={d.id} label="Copy delivery id" />
            </dd>
          </dl>
          <div>
            <h3 className="mb-2 text-xs font-semibold text-muted uppercase">Attempts</h3>
            {d.attempts.length === 0 ? (
              <p className="text-[13px] text-muted">{deliveryOpen(d) ? "No attempt yet; the first one is waiting to run." : "No attempt was made."}</p>
            ) : (
              <div className="rounded-md border border-line">
                <Table head={["#", "Time", "Result", "HTTP", "Error", "Duration"]}>
                  {d.attempts.map((a) => (
                    <tr key={a.number}>
                      <Td className="font-mono text-[11px] tabular-nums">{a.number}</Td>
                      <Td className="text-xs whitespace-nowrap" title={time(a.at)}>
                        {ago(a.at)}
                      </Td>
                      <Td>
                        <Badge tone={a.ok ? "ok" : "bad"}>{a.ok ? "sent" : "failed"}</Badge>
                      </Td>
                      <Td className="font-mono text-[11px] tabular-nums">{a.http_status ?? "—"}</Td>
                      <Td className="text-xs" title={errorHelp(a.error)}>
                        {a.error ? <Mono>{a.error}</Mono> : "—"}
                      </Td>
                      <Td className="font-mono text-[11px] whitespace-nowrap tabular-nums">{duration(a.duration_ms)}</Td>
                    </tr>
                  ))}
                </Table>
              </div>
            )}
          </div>
        </div>
      )}
    </Drawer>
  );
}

/** Link from a delivery to the silence that held it back, or that it follows. */
function SilenceLink({ id, after, onOpen }: { id: string; after: boolean; onOpen: (id: string) => void }) {
  return (
    <button
      type="button"
      className="ml-1 inline-flex items-center gap-0.5 rounded align-middle text-xs text-muted hover:text-accent hover:underline"
      title={after ? "Sent after this silence ended" : "Held back by this silence"}
      aria-label={after ? "Open the silence this message follows" : "Open the silence that held this back"}
      onClick={(e) => {
        e.stopPropagation();
        onOpen(id);
      }}
    >
      <BellOff className="size-3.5" />
      {after ? "after silence" : "silence"}
    </button>
  );
}
