import { Link } from "@tanstack/react-router";
import { Check, Layers, Loader2, Settings2, X, RefreshCw, RotateCcw, RotateCw, Rocket, Scale, Trash2 } from "lucide-react";
import { useState } from "react";

import type { Service } from "../../api/types";
import { ReplicaBar } from "./ReplicaBar";
import { isRunning, runningNodes } from "../swarm/TaskUsage";
import { type ServiceSwarmAction, type ServiceSwarmView, type SwarmTask, type SwarmTaskStat, swarmServiceBusy, useSwarmTaskStats, useServiceSwarm, useServiceSwarmAction, useServiceSwarmLogs, useSwarm } from "../../api/swarm";
import { useServers } from "../../api/hooks";
import { errorText, useToast } from "../../app/toast";
import { Badge, Button, Confirm, CopyIcon, Empty, ErrorNote, Input, Modal, Panel, Select, Skeleton, Table, Td, ago, bytes, time } from "../ui";

type Tone = "ok" | "accent" | "warn" | "bad" | "muted";
const taskTone = (s: string): Tone =>
  s === "running" ? "ok" : s === "failed" || s === "rejected" || s === "orphaned" ? "bad" : s === "shutdown" || s === "complete" ? "muted" : "accent";
const updateTone = (s: string): Tone =>
  s === "completed" || s === "rollback_completed" ? "ok" : s === "paused" || s === "rollback_paused" ? "warn" : s.startsWith("rollback") ? "warn" : "accent";
const taskMs = (t: SwarmTask) => (typeof t.updated_at === "number" ? t.updated_at : t.updated_at ? Date.parse(t.updated_at) || undefined : undefined);

const LABEL: Record<ServiceSwarmAction["kind"], string> = { deploy: "Deploy", scale: "Scale", restart: "Restart", rollback: "Roll back", remove: "Remove" };

/** The Swarm panel of a service in a Swarm cluster: status, actions, tasks and logs. */
export function ServiceSwarm({ a, onEditSettings }: { a: Service; onEditSettings?: () => void }) {
  const q = useServiceSwarm(a.id);
  const v = q.data;
  const cluster = useSwarm(v?.cluster_id ?? "", !!v?.cluster_id).data;
  const servers = useServers().data ?? [];
  const act = useServiceSwarmAction(a.id);
  const toast = useToast();
  const [dialog, setDialog] = useState<"scale" | "restart" | "rollback" | "remove" | null>(null);
  const [pending, setPending] = useState<ServiceSwarmAction["kind"] | null>(null);

  const run = (b: ServiceSwarmAction, done?: () => void) => {
    setPending(b.kind);
    act.mutate(b, {
      onSuccess: () => {
        toast(`${LABEL[b.kind]} started on the swarm`);
        done?.();
      },
      onError: (e) => toast(errorText(e), "error"),
      onSettled: () => setPending(null),
    });
  };

  if (q.isLoading) return <Panel title="Swarm"><Skeleton rows={4} /></Panel>;
  if (q.error || !v) return <Panel title="Swarm"><ErrorNote error={q.error ?? "No swarm status"} /></Panel>;

  const s = v.status;
  const exists = !!s?.exists;
  const replicated = v.settings.mode === "replicated";
  const denied = a.can_deploy === false;
  const deniedWhy = denied ? "Needs the deploy permission on this environment" : undefined;
  // Swarm node id → host name: the cluster's live node list, else the Forge member record.
  const hostOf = (nodeId?: string) => {
    if (!nodeId) return undefined;
    const n = cluster?.nodes?.find((x) => x.id === nodeId);
    const m = cluster?.members.find((x) => x.node_id === nodeId);
    const server = m ? servers.find((x) => x.id === m.server_id) : undefined;
    return { name: n?.hostname ?? m?.hostname ?? server?.hostname, server: m?.server_id };
  };
  const btn = (kind: ServiceSwarmAction["kind"], icon: React.ReactNode, onClick: () => void, why?: string, danger = false) => (
    <span title={why ?? deniedWhy} className="inline-flex">
      <Button variant={danger ? "danger" : "default"} disabled={!!why || denied || pending !== null} onClick={onClick}>
        {pending === kind ? <Loader2 className="size-4 animate-spin" /> : icon} {LABEL[kind]}
      </Button>
    </span>
  );
  const noService = exists ? undefined : "Not deployed to the swarm yet";

  return (
    <Panel
      title="Swarm"
      actions={
        <span className="flex flex-wrap justify-end gap-1.5">
          <span title={deniedWhy} className="inline-flex">
            <Button variant="primary" disabled={denied || pending !== null} onClick={() => run({ kind: "deploy" })}>
              {pending === "deploy" ? <Loader2 className="size-4 animate-spin" /> : <Rocket className="size-4" />} Deploy
            </Button>
          </span>
          {btn("scale", <Scale className="size-4" />, () => setDialog("scale"), noService ?? (replicated ? undefined : "Global services run one task per matching node"))}
          {btn("restart", <RotateCw className="size-4" />, () => setDialog("restart"), noService)}
          {btn("rollback", <RotateCcw className="size-4" />, () => setDialog("rollback"), noService)}
          {btn("remove", <Trash2 className="size-4" />, () => setDialog("remove"), noService, true)}
        </span>
      }
    >
      <StatusLine v={v} serviceId={a.id} hostOf={(id) => hostOf(id)?.name} canScale={!denied} />
      <HaReadiness v={v} onEdit={onEditSettings} />
      {v.status_error && <ErrorNote error={`Swarm status unavailable: ${v.status_error}`} />}
      {s && !exists ? (
        <Empty icon={<Layers className="size-5" />} title="Not in the swarm yet" action={<Button variant="primary" disabled={denied || pending !== null} title={deniedWhy} onClick={() => run({ kind: "deploy" })}><Rocket className="size-4" /> Deploy</Button>}>
          Deploy creates the Docker service <span className="font-mono text-xs">{v.name || a.name}</span> on a manager; the swarm then places its tasks.
        </Empty>
      ) : s ? (
        <Tasks clusterId={v.cluster_id} tasks={s.tasks} hostOf={hostOf} />
      ) : null}
      {exists && <Logs id={a.id} />}

      {dialog === "scale" && <ScaleDialog current={s?.desired || v.settings.replicas} busy={pending === "scale"} onClose={() => setDialog(null)} onScale={(replicas) => run({ kind: "scale", replicas }, () => setDialog(null))} />}
      {dialog === "restart" && (
        <Confirm
          open
          onOpenChange={(o) => !o && setDialog(null)}
          title={`Restart ${a.name}?`}
          confirmLabel="Restart"
          busy={pending === "restart"}
          body={`Replaces every task with a fresh one, following the rollout settings (${v.settings.update.parallelism || "all"} at a time, ${v.settings.update.order}). With one replica or stop-first, expect a short outage.`}
          onConfirm={() => run({ kind: "restart" }, () => setDialog(null))}
        />
      )}
      {dialog === "rollback" && (
        <Confirm
          open
          onOpenChange={(o) => !o && setDialog(null)}
          title={`Roll back ${a.name}?`}
          confirmLabel="Roll back"
          busy={pending === "rollback"}
          body="Returns the Docker service to its previous spec (image, settings) and replaces the tasks. Forge's saved definition does not change; the next Deploy applies it again."
          onConfirm={() => run({ kind: "rollback" }, () => setDialog(null))}
        />
      )}
      {dialog === "remove" && (
        <Confirm
          open
          onOpenChange={(o) => !o && setDialog(null)}
          title={`Remove ${a.name} from the swarm?`}
          confirmLabel="Remove"
          busy={pending === "remove"}
          body={
            <div className="space-y-2 text-[13px]">
              <p>
                Stops all {s?.running ?? 0} running task{s?.running === 1 ? "" : "s"} and deletes the Docker service <span className="font-mono text-xs">{v.name}</span>. The service is down until the next Deploy.
              </p>
              <ul className="list-disc space-y-0.5 pl-4 text-xs">
                <li>Ingress endpoints pointing at it stop answering.</li>
                <li>The definition, secrets and volumes on the nodes stay; delete the service afterwards if you no longer need it.</li>
              </ul>
            </div>
          }
          onConfirm={() => run({ kind: "remove" }, () => setDialog(null))}
        />
      )}
    </Panel>
  );
}

function StatusLine({ v, serviceId, hostOf, canScale }: { v: ServiceSwarmView; serviceId: string; hostOf: (nodeId?: string) => string | undefined; canScale: boolean }) {
  const s = v.status;
  return (
    <div className="flex flex-wrap items-center gap-x-4 gap-y-1.5 border-b border-line px-3 py-2 text-xs">
      <span className="flex items-center gap-1.5">
        <span className="text-muted">Tasks</span>
        {s?.exists ? (
          <ReplicaBar v={v} serviceId={serviceId} hostOf={hostOf} canScale={canScale} />
        ) : (
          <span className="text-faint">—</span>
        )}
        {swarmServiceBusy(v) && <Loader2 className="size-3.5 animate-spin text-accent" aria-label="Converging" />}
      </span>
      <span className="flex items-center gap-1.5">
        <span className="text-muted">Mode</span>
        <span>{v.settings.mode}</span>
      </span>
      {s?.update_state && (
        <span className="flex min-w-0 items-center gap-1.5">
          <span className="text-muted">Update</span>
          <Badge tone={updateTone(s.update_state)}>{s.update_state.replace(/_/g, " ")}</Badge>
          {s.update_message && (
            <span className="max-w-80 truncate text-muted" title={s.update_message}>
              {s.update_message}
            </span>
          )}
        </span>
      )}
      <span className="flex min-w-0 flex-1 items-center gap-1.5">
        <span className="text-muted">Image</span>
        <span className="min-w-0 truncate font-mono text-[11px] select-all" title={s?.image}>
          {s?.image || "—"}
        </span>
        {s?.image && <CopyIcon text={s.image} label="Copy image" />}
      </span>
      {v.name && (
        <span className="flex min-w-0 items-center gap-1.5">
          <span className="text-muted">Docker service</span>
          <span className="max-w-48 truncate font-mono text-[11px] select-all" title={v.name}>
            {v.name}
          </span>
        </span>
      )}
    </div>
  );
}

const haTone = (n: number): Tone => (n >= 80 ? "ok" : n >= 50 ? "warn" : "bad");

/** HA readiness: the server's score and checklist; hidden when an older server sends no checks. */
function HaReadiness({ v, onEdit }: { v: ServiceSwarmView; onEdit?: () => void }) {
  const { score, checks } = v.ha;
  if (checks.length === 0) return null;
  const failing = checks.filter((c) => !c.ok).length;
  return (
    <div className="border-b border-line px-3 py-2 text-xs">
      <div className="flex flex-wrap items-center gap-2">
        <span className="font-medium text-muted">HA readiness</span>
        <Badge tone={haTone(score)}>
          <span className="font-mono tabular-nums">{score}</span>/100
        </Badge>
        <span className="text-faint">{failing === 0 ? "all checks pass" : `${failing} of ${checks.length} checks fail`}</span>
        {onEdit && (
          <Button variant="ghost" className="ml-auto" onClick={onEdit} title="Configuration → Swarm">
            <Settings2 className="size-4" /> Edit Swarm settings
          </Button>
        )}
      </div>
      <ul className="mt-1.5 grid gap-x-6 gap-y-0.5 sm:grid-cols-2">
        {checks.map((c) => (
          <li key={c.name} className="flex min-w-0 items-start gap-1.5">
            {c.ok ? <Check className="mt-px size-3.5 shrink-0 text-ok" aria-label="Passes" /> : <X className="mt-px size-3.5 shrink-0 text-bad" aria-label="Fails" />}
            <span className="min-w-0">
              <span className={c.ok ? "text-muted" : "text-fg"}>{c.name}</span>
              {!c.ok && c.hint && <span className="block text-faint">{c.hint}</span>}
            </span>
          </li>
        ))}
      </ul>
    </div>
  );
}

function Tasks({ clusterId, tasks, hostOf }: { clusterId: string; tasks: SwarmTask[]; hostOf: (id?: string) => { name?: string; server?: string } | undefined }) {
  const stats = useSwarmTaskStats(clusterId, runningNodes(tasks)).data;
  if (tasks.length === 0) return <p className="px-3 py-3 text-xs text-muted">No tasks yet. They appear once the swarm schedules the service.</p>;
  // Newest first within a slot, so the current task sits on top.
  const rows = [...tasks].sort((x, y) => (x.slot ?? 0) - (y.slot ?? 0) || (taskMs(y) ?? 0) - (taskMs(x) ?? 0));
  return (
    <Table head={["Slot", "Node", "State", "Desired", "CPU", "Memory", "Error", "Updated"]}>
      {rows.map((t) => {
        const h = hostOf(t.node_id);
        const ms = taskMs(t);
        return (
          <tr key={t.id}>
            <Td>
              <span className="font-mono text-[11px] tabular-nums" title={`Task ${t.id}`}>
                {t.slot ?? "—"}
              </span>
            </Td>
            <Td className="max-w-[180px]">
              {h?.server ? (
                <Link to="/nodes/$nodeId" params={{ nodeId: h.server }} className="block truncate hover:text-accent" title={t.node_id}>
                  {h.name ?? t.node_id?.slice(0, 12)}
                </Link>
              ) : (
                <span className="block truncate font-mono text-[11px] text-muted" title={t.node_id}>
                  {h?.name ?? t.node_id?.slice(0, 12) ?? "—"}
                </span>
              )}
            </Td>
            <Td>
              <Badge tone={taskTone(t.state)}>{t.state}</Badge>
            </Td>
            <Td className="text-xs text-muted">{t.desired_state ?? "—"}</Td>
            <Td className="font-mono text-[11px] whitespace-nowrap tabular-nums">{fmtCpu(isRunning(t) ? stats?.get(t.id) : undefined)}</Td>
            <Td className="font-mono text-[11px] whitespace-nowrap tabular-nums">{fmtMem(isRunning(t) ? stats?.get(t.id) : undefined)}</Td>
            <Td className="max-w-[260px]">
              {t.error ? (
                <span className="block truncate text-xs text-bad select-all" title={t.error}>
                  {t.error}
                </span>
              ) : (
                <span className="text-faint">—</span>
              )}
            </Td>
            <Td className="text-xs whitespace-nowrap text-muted">
              <span title={ms ? time(ms) : undefined}>{ms ? ago(ms) : "—"}</span>
            </Td>
          </tr>
        );
      })}
    </Table>
  );
}

const fmtCpu = (s?: SwarmTaskStat) => (s?.cpu_percent === undefined ? "—" : `${s.cpu_percent.toFixed(1)}%`);
const fmtMem = (s?: SwarmTaskStat) => (s?.memory_bytes === undefined ? "—" : `${bytes(s.memory_bytes)}${s.memory_limit ? ` / ${bytes(s.memory_limit)}` : ""}`);

const TAILS = [100, 200, 500, 1000] as const;

function Logs({ id }: { id: string }) {
  const [tail, setTail] = useState<number>(200);
  const q = useServiceSwarmLogs(id, tail);
  return (
    <div className="border-t border-line">
      <div className="flex flex-wrap items-center gap-2 px-3 py-1.5">
        <span className="text-xs font-medium text-muted">Logs</span>
        <span className="text-[11px] text-faint">all tasks</span>
        <span className="ml-auto flex items-center gap-1.5">
          <Select aria-label="Lines" className="h-7 w-28 py-0 text-xs" value={tail} onChange={(e) => setTail(Number(e.target.value))}>
            {TAILS.map((n) => (
              <option key={n} value={n}>
                Last {n}
              </option>
            ))}
          </Select>
          {q.data && <CopyIcon text={q.data} label="Copy logs" />}
          <Button onClick={() => void q.refetch()} disabled={q.isFetching} aria-label="Refresh logs">
            {q.isFetching ? <Loader2 className="size-4 animate-spin" /> : <RefreshCw className="size-4" />} Refresh
          </Button>
        </span>
      </div>
      {q.error ? (
        <ErrorNote error={q.error} />
      ) : q.isLoading ? (
        <Skeleton rows={3} />
      ) : (
        <pre className="log-console mx-3 mb-3 max-h-[50vh] min-h-24 overflow-auto rounded-lg bg-term p-3 font-mono text-[11.5px] leading-relaxed whitespace-pre-wrap break-all text-term-fg select-text">
          {q.data || "No output yet."}
        </pre>
      )}
    </div>
  );
}

function ScaleDialog({ current, busy, onClose, onScale }: { current: number; busy: boolean; onClose: () => void; onScale: (n: number) => void }) {
  const [n, setN] = useState(current || 1);
  const bad = !Number.isInteger(n) || n < 1 || n > 1000;
  return (
    <Modal open onOpenChange={(o) => !o && onClose()} title="Scale">
      <form
        className="space-y-3"
        onSubmit={(e) => {
          e.preventDefault();
          if (!bad) onScale(n);
        }}
      >
        <label className="block space-y-1">
          <span className="text-xs font-medium text-muted">Replicas</span>
          <Input type="number" min={1} max={1000} autoFocus className={`w-28 font-mono tabular-nums ${bad ? "border-bad" : ""}`} value={n} onChange={(e) => setN(Number(e.target.value))} />
          <span className="block text-xs text-faint">
            Now <span className="font-mono tabular-nums">{current}</span>. 1 to 1000. Only the task count changes; the next Deploy uses the replicas saved in Configuration.
          </span>
        </label>
        {n < current && <p className="text-xs text-warn">Stops {current - n} task{current - n === 1 ? "" : "s"}; their connections drop.</p>}
        <div className="flex justify-end gap-2">
          <Button type="button" onClick={onClose}>
            Cancel
          </Button>
          <Button type="submit" variant="primary" disabled={bad || busy || n === current}>
            {busy ? <Loader2 className="size-4 animate-spin" /> : <Scale className="size-4" />} Scale
          </Button>
        </div>
      </form>
    </Modal>
  );
}
