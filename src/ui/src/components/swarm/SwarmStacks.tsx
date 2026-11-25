import { Link, useNavigate } from "@tanstack/react-router";
import { ChevronDown, ChevronRight, Layers, Loader2, Logs, Network, Plus, RefreshCw, RotateCcw, Rocket, Scale, Trash2 } from "lucide-react";
import { useState } from "react";

import {
  type SwarmStackRef,
  type SwarmStackService,
  type SwarmTaskStat,
  useDeploySwarmStack,
  useRemoveSwarmStack,
  useStackServiceAction,
  useStackServiceLogs,
  useStackSwarm,
  useSwarmStacks,
  useSwarmTaskStats,
} from "../../api/swarm";
import { errorText, useToast } from "../../app/toast";
import { Badge, Button, Confirm, CopyButton, Drawer, Empty, ErrorNote, Field, Input, Modal, Panel, RowMenu, Select, Skeleton, ago, time, toMs } from "../ui";
import { useJobFollow } from "./jobFollow";
import { TaskUsage, runningNodes } from "./TaskUsage";

/** Stacks deployed into the swarm, each with its live services. */
export function SwarmStacks({ clusterId, environmentId }: { clusterId: string; environmentId?: string | null }) {
  const q = useSwarmStacks(clusterId);
  const navigate = useNavigate();
  const newStack = (
    <Button variant="primary" onClick={() => void navigate({ to: "/stacks/new", search: { environment: environmentId ?? undefined } })}>
      <Plus className="size-4" /> Deploy stack
    </Button>
  );
  return (
    <Panel title={`Stacks${q.data ? ` · ${q.data.length}` : ""}`} description="Compose stacks in this cluster's environments run as swarm services." actions={newStack}>
      {q.error ? (
        <div className="p-3">
          <ErrorNote error={q.error} />
        </div>
      ) : q.isLoading ? (
        <div className="p-3">
          <Skeleton rows={3} />
        </div>
      ) : !q.data?.length ? (
        <Empty icon={<Layers className="size-5" />} title="No stacks in the swarm" action={newStack}>
          Create a compose stack in one of this cluster's environments; pick any manager node and Forge deploys it with <code className="font-mono">docker stack deploy</code>.
        </Empty>
      ) : (
        <ul className="divide-y divide-line">
          {q.data.map((s) => (
            <StackRow key={s.id} s={s} />
          ))}
        </ul>
      )}
    </Panel>
  );
}

function StackRow({ s }: { s: SwarmStackRef }) {
  const [open, setOpen] = useState(false);
  const [removing, setRemoving] = useState(false);
  const toast = useToast();
  const follow = useJobFollow(`Stack ${s.name}`);
  const deploy = useDeploySwarmStack(s.id);
  const remove = useRemoveSwarmStack(s.id);
  const deployed = s.deployed_revision > 0;
  const behind = deployed && s.deployed_revision < s.revision;

  const doDeploy = () =>
    deploy.mutate(undefined, {
      onSuccess: (j) => follow.started(j, `${deployed ? "Redeploying" : "Deploying"} ${s.name} (revision ${s.revision})`),
      onError: (e) => toast(errorText(e), "error"),
    });

  return (
    <li className="text-[13px]">
      <div className="flex min-w-0 flex-wrap items-center gap-2 px-3 py-2">
        <button type="button" aria-expanded={open} aria-label={open ? `Collapse ${s.name}` : `Expand ${s.name}`} onClick={() => setOpen(!open)} className="rounded p-0.5 text-muted hover:text-fg focus-visible:outline-2 focus-visible:outline-accent">
          {open ? <ChevronDown className="size-3.5" /> : <ChevronRight className="size-3.5" />}
        </button>
        <Link to="/stacks/$stackId" params={{ stackId: s.id }} className="max-w-60 truncate font-medium hover:text-accent" title={s.name}>
          {s.name}
        </Link>
        {s.environment && <span className="truncate text-xs text-faint">{s.environment}</span>}
        <span className="font-mono text-[11px] text-muted tabular-nums" title="Deployed revision / latest revision">
          r{deployed ? s.deployed_revision : "—"} / r{s.revision}
        </span>
        {!deployed ? <Badge tone="muted">not deployed</Badge> : behind ? <Badge tone="warn">changes not deployed</Badge> : <Badge tone="ok">deployed</Badge>}
        <span className="ml-auto flex gap-1.5">
          <Button onClick={doDeploy} disabled={deploy.isPending}>
            {deploy.isPending ? <Loader2 className="size-4 animate-spin" /> : <Rocket className="size-4" />} {deployed ? "Redeploy" : "Deploy"}
          </Button>
          <Button onClick={() => setRemoving(true)} disabled={!deployed} title={deployed ? "Remove the stack's services from the swarm" : "Not deployed"}>
            <Trash2 className="size-4" /> Remove
          </Button>
        </span>
      </div>
      {open && <StackStatus id={s.id} />}
      {removing && (
        <Confirm
          open
          onOpenChange={(o) => !o && setRemoving(false)}
          title={`Remove ${s.name} from the swarm?`}
          confirmLabel="Remove"
          busy={remove.isPending}
          onConfirm={() =>
            remove.mutate(undefined, {
              onSuccess: (j) => {
                follow.started(j, `Removing ${s.name} from the swarm`);
                setRemoving(false);
              },
              onError: (e) => toast(errorText(e), "error"),
            })
          }
          body={
            <p className="text-[13px]">
              Every service of {s.name} stops on every node: expect an outage for whatever routes to it. Named volumes, secrets and configs stay; the stack definition stays in Forge and can be deployed again.
            </p>
          }
        />
      )}
      {follow.view}
    </li>
  );
}

type Dialog = { kind: "scale"; svc: SwarmStackService } | { kind: "logs"; svc: SwarmStackService } | null;

function StackStatus({ id }: { id: string }) {
  const q = useStackSwarm(id);
  const act = useStackServiceAction(id);
  const toast = useToast();
  const [dialog, setDialog] = useState<Dialog>(null);
  const stats = useSwarmTaskStats(q.data?.cluster_id ?? "", (q.data?.status?.services ?? []).flatMap((s) => runningNodes(s.tasks)));

  if (q.error) return <Pad><ErrorNote error={q.error} /></Pad>;
  if (q.isLoading || !q.data) return <Pad><Skeleton rows={2} /></Pad>;
  const v = q.data;
  const services = v.status?.services ?? [];

  const run = (svc: SwarmStackService, kind: "restart" | "rollback") =>
    act.mutate(
      { service: svc.name, kind },
      { onSuccess: () => toast(`${svc.name}: ${kind === "restart" ? "restarting" : "rolling back"}`), onError: (e) => toast(errorText(e), "error") },
    );

  return (
    <div className="space-y-2 border-t border-line bg-panel-2/40 px-3 py-2">
      {v.status_error && <ErrorNote error={new Error(`No manager answered: ${v.status_error}`)} />}
      {v.findings.length > 0 && (
        <ul className="space-y-0.5 text-xs">
          {v.findings.map((f, i) => (
            <li key={i} className="flex min-w-0 gap-1.5">
              <Badge tone={f.severity === "blocked" ? "bad" : f.severity === "privileged" ? "warn" : "muted"}>{f.severity}</Badge>
              {f.service && <span className="font-mono text-[11px]">{f.service}</span>}
              <span className="min-w-0 truncate text-muted" title={f.message}>
                {f.message}
              </span>
            </li>
          ))}
        </ul>
      )}
      {!v.status && !v.status_error ? (
        <div className="text-xs text-muted">Not running in the swarm. Deploy it to start its services.</div>
      ) : services.length === 0 && v.status ? (
        <div className="text-xs text-muted">The swarm reports no services for namespace <span className="font-mono">{v.namespace || "—"}</span>.</div>
      ) : (
        <div className="overflow-x-auto">
          <table className="w-full text-xs">
            <thead className="text-left text-[11px] text-faint">
              <tr>
                <th className="py-1 pr-3 font-medium">Service</th>
                <th className="py-1 pr-3 font-medium">Replicas</th>
                <th className="py-1 pr-3 font-medium">Ports</th>
                <th className="py-1 pr-3 font-medium">Image</th>
                <th className="py-1 pr-3 font-medium">Update</th>
                <th className="py-1" />
              </tr>
            </thead>
            <tbody className="divide-y divide-line">
              {services.map((svc) => (
                <ServiceRow key={svc.docker_name} svc={svc} stats={stats.data} busy={act.isPending && act.variables?.service === svc.name} onMenu={(k) => (k === "restart" || k === "rollback" ? run(svc, k) : setDialog({ kind: k, svc }))} />
              ))}
            </tbody>
          </table>
        </div>
      )}
      {dialog?.kind === "scale" && <ScaleDialog id={id} svc={dialog.svc} onClose={() => setDialog(null)} />}
      {dialog?.kind === "logs" && <LogsDrawer id={id} svc={dialog.svc} onClose={() => setDialog(null)} />}
    </div>
  );
}

const Pad = ({ children }: { children: React.ReactNode }) => <div className="border-t border-line px-3 py-2">{children}</div>;

function ServiceRow({ svc, stats, busy, onMenu }: { svc: SwarmStackService; stats?: Map<string, SwarmTaskStat>; busy: boolean; onMenu: (k: "scale" | "restart" | "rollback" | "logs") => void }) {
  const healthy = svc.running >= svc.desired && svc.desired > 0;
  const failed = svc.tasks.filter((t) => t.error).slice(0, 3);
  const global = svc.mode === "global";
  return (
    <tr className="align-top">
      <td className="max-w-56 py-1.5 pr-3">
        <div className="flex min-w-0 items-center gap-1.5">
          <span className="truncate font-medium" title={svc.docker_name}>
            {svc.name}
          </span>
          {global && <Badge tone="muted">global</Badge>}
          {svc.shared && (
            <span title="Attached to forge-shared">
              <Badge tone="accent">
                <Network className="inline size-3" /> shared
              </Badge>
            </span>
          )}
          {busy && <Loader2 className="size-3.5 animate-spin text-muted" />}
        </div>
        {failed.map((t) => (
          <div key={t.id} className="mt-0.5 truncate text-[11px] text-bad select-all" title={`${t.state}: ${t.error}`}>
            {t.slot !== undefined ? `#${t.slot} ` : ""}
            {t.error}
            {t.updated_at !== undefined && <span className="ml-1 text-faint" title={time(toMs(t.updated_at))}>{ago(toMs(t.updated_at))}</span>}
          </div>
        ))}
      </td>
      <td className="py-1.5 pr-3 font-mono text-[11px] tabular-nums">
        <span className={healthy ? "text-ok" : svc.running === 0 ? "text-bad" : "text-warn"}>
          {svc.running}/{svc.desired}
        </span>
        <TaskUsage tasks={svc.tasks} stats={stats} />
      </td>
      <td className="py-1.5 pr-3 font-mono text-[11px]">
        {svc.ports.length
          ? svc.ports.map((p, i) => (
              <div key={i} title={`${p.mode} · ${p.protocol}`}>
                {p.published ?? "—"}→{p.target ?? "—"}
                {p.protocol !== "tcp" ? `/${p.protocol}` : ""}
              </div>
            ))
          : "—"}
      </td>
      <td className="max-w-56 truncate py-1.5 pr-3 font-mono text-[11px] text-muted" title={svc.image}>
        {svc.image ?? "—"}
      </td>
      <td className="py-1.5 pr-3">
        {svc.update_state ? (
          <span title={svc.update_message}>
            <Badge tone={svc.update_state === "completed" ? "ok" : svc.update_state.includes("paused") || svc.update_state.includes("rollback") ? "warn" : "accent"}>{svc.update_state}</Badge>
          </span>
        ) : (
          <span className="text-faint">—</span>
        )}
      </td>
      <td className="py-1.5 text-right">
        <RowMenu
          label={`Actions for ${svc.name}`}
          actions={[
            { label: "Logs", icon: Logs, tone: "accent", onSelect: () => onMenu("logs") },
            { label: "Scale…", icon: Scale, disabled: global, hint: global ? "Global services run one task per node" : undefined, onSelect: () => onMenu("scale") },
            { label: "Restart (rolling)", icon: RefreshCw, tone: "warn", onSelect: () => onMenu("restart") },
            { label: "Roll back to previous spec", icon: RotateCcw, tone: "warn", onSelect: () => onMenu("rollback") },
          ]}
        />
      </td>
    </tr>
  );
}

function ScaleDialog({ id, svc, onClose }: { id: string; svc: SwarmStackService; onClose: () => void }) {
  const act = useStackServiceAction(id);
  const toast = useToast();
  const [n, setN] = useState(String(svc.desired));
  const replicas = Number(n);
  const ok = Number.isInteger(replicas) && replicas >= 0 && replicas <= 1000;
  return (
    <Modal open onOpenChange={(o) => !o && onClose()} title={`Scale ${svc.name}`} description={`Running ${svc.running} of ${svc.desired}. 0 stops every task.`}>
      <form
        onSubmit={(e) => {
          e.preventDefault();
          if (!ok) return;
          act.mutate(
            { service: svc.name, kind: "scale", replicas },
            {
              onSuccess: () => {
                toast(`${svc.name}: scaling to ${replicas}`);
                onClose();
              },
              onError: (e) => toast(errorText(e), "error"),
            },
          );
        }}
      >
        <Field label="Replicas" hint={ok ? (replicas === 0 ? "Scaling to 0 stops the service: expect an outage." : undefined) : <span className="text-bad">0 to 1000</span>}>
          <Input type="number" min={0} max={1000} value={n} onChange={(e) => setN(e.target.value)} className="w-28 font-mono" autoFocus />
        </Field>
        <div className="flex justify-end gap-2 pt-3">
          <Button type="button" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" disabled={!ok || act.isPending}>
            {act.isPending && <Loader2 className="size-4 animate-spin" />} Scale
          </Button>
        </div>
      </form>
    </Modal>
  );
}

const TAILS = [100, 200, 500, 2000] as const;

function LogsDrawer({ id, svc, onClose }: { id: string; svc: SwarmStackService; onClose: () => void }) {
  const [tail, setTail] = useState<number>(200);
  const q = useStackServiceLogs(id, svc.name, tail);
  return (
    <Drawer open onOpenChange={(o) => !o && onClose()} title={`Logs · ${svc.name}`} wide>
      <div className="mb-2 flex items-center gap-2 text-xs">
        <Select value={String(tail)} onChange={(e) => setTail(Number(e.target.value))} aria-label="Lines" className="w-auto">
          {TAILS.map((t) => (
            <option key={t} value={t}>
              last {t} lines
            </option>
          ))}
        </Select>
        <Button onClick={() => void q.refetch()} disabled={q.isFetching}>
          {q.isFetching ? <Loader2 className="size-4 animate-spin" /> : <RefreshCw className="size-4" />} Refresh
        </Button>
        {q.data && <CopyButton text={q.data} label="Copy logs" />}
      </div>
      {q.error ? (
        <ErrorNote error={q.error} />
      ) : q.isLoading ? (
        <Skeleton rows={8} />
      ) : (
        <pre className="max-h-[75vh] overflow-auto rounded-md border border-line bg-bg p-2 font-mono text-[11px] whitespace-pre-wrap select-text">{q.data || "No log lines."}</pre>
      )}
    </Drawer>
  );
}
