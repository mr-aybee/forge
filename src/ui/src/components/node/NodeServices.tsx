import { Link } from "@tanstack/react-router";
import { Container, Layers, Loader2, Play, RotateCw, ScrollText, Square } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import { ApiError } from "../../api/client";
import { useAllEnvironments, useServiceAction, type ServiceAction, useServiceHealth, useServices, useClusters, useMe } from "../../api/hooks";
import type { ServiceInstance, Service, Role, Server } from "../../api/types";
import { useTerminals } from "../../app/terminals";
import { errorText, useToast } from "../../app/toast";
import { InstanceHealthBadge } from "../health/InstanceHealth";
import { InstanceStateChip, liveState } from "../InstanceState";
import { isStackService } from "../stacks/StackBadge";
import { Badge, Confirm, Empty, ErrorNote, Panel, RowMenu, type RowAction, Skeleton, Status, Table, Td, ago, time, useRowMenu } from "../ui";

/** Roles that grant `deploy` (start/stop/restart). The server enforces it; this only disables the menu items. */
const DEPLOYERS: Role[] = ["platform_admin", "workspace_admin", "operator", "developer"];

const kindOf = (a: Service) => (a.runtime === "container" || a.runtime === "compose-service" ? "container" : "native");

const deployTone = (s: string) => (s === "deployed" ? "succeeded" : s === "failed" ? "failed" : s.endsWith("ing") ? "running" : "pending");

type Pending = { app: Service; action: "stop" | "restart" };

/** Every service instance placed on this node, with its live state and the per-node actions of the service page. */
export function NodeServices({ node }: { node: Server }) {
  const apps = useServices();
  const environments = useAllEnvironments().data ?? [];
  const clusters = useClusters().data ?? [];
  const bindings = useMe().data?.bindings;
  const canDeploy = (Array.isArray(bindings) ? bindings : []).some((b) => DEPLOYERS.includes(b.role));
  const act = useServiceAction();
  const toast = useToast();
  const rm = useRowMenu();
  const [busy, setBusy] = useState<string>();
  const [confirm, setConfirm] = useState<Pending>();
  const terminals = useTerminals();

  const rows: { app: Service; inst: ServiceInstance }[] = [];
  for (const a of Array.isArray(apps.data) ? apps.data : []) {
    const inst = (Array.isArray(a.instances) ? a.instances : []).find((i) => i.server_id === node.id);
    if (inst) rows.push({ app: a, inst });
  }
  rows.sort((x, y) => x.app.name.localeCompare(y.app.name));

  const run = (a: Service, action: ServiceAction) => {
    setBusy(`${a.id}:${action}`);
    act.mutate(
      { id: a.id, server: node.id, action },
      {
        onSuccess: (r) => (r.status === "failed" ? toast(r.error ?? `${action} failed`, "error") : toast(`${action} ${r.status === "running" ? "started" : "done"}: ${a.name}`)),
        onError: (e) => toast(errorText(e), "error"),
        onSettled: () => setBusy(undefined),
      },
    );
  };

  const offline = node.status !== "online";
  const counts = rows.reduce<Record<string, number>>((m, r) => {
    const k = liveState(r.inst, node).kind;
    m[k] = (m[k] ?? 0) + 1;
    return m;
  }, {});

  return (
    <Panel
      title="Services"
      description={
        rows.length
          ? `${rows.length} placed · ${counts.running ?? 0} running${counts.failed ? ` · ${counts.failed} failed` : ""}${counts.stopped ? ` · ${counts.stopped} stopped` : ""}${offline ? ` · node ${node.status}` : ""}. State is what the node last reported, not a health check.`
          : "Services placed on this node."
      }
    >
      {apps.isLoading ? (
        <div className="p-4">
          <Skeleton />
        </div>
      ) : apps.isError ? (
        <div className="p-4">
          <ErrorNote error={apps.error} />
        </div>
      ) : rows.length === 0 ? (
        <Empty icon={<Layers className="size-5" />} title="Nothing placed on this node">
          Place a service on this node from its page under <Link to="/services" className="text-accent hover:underline">Services</Link>.
        </Empty>
      ) : (
        <Table head={["Service", "Environment", "Kind", "Role", "Deploy", "State", "Health", ""]}>
          {rows.map(({ app: a, inst: i }) => {
            const ns = environments.find((n) => n.id === a.environment_id);
            const cl = clusters.find((c) => c.id === ns?.cluster_id);
            const stack = isStackService(a);
            const jobBusy = i.status.endsWith("ing");
            const why = !canDeploy
              ? "Needs the deploy permission on this environment"
              : stack
                ? "Managed by its compose stack; act on the stack"
                : offline
                  ? `The node's agent is ${node.status}`
                  : i.status === "pending"
                    ? "Not deployed to this node yet"
                    : jobBusy
                      ? `Wait: ${i.status}`
                      : busy
                        ? "Another action is running"
                        : undefined;
            const live = liveState(i, node).kind;
            const actions: RowAction[] = [
              { label: "Logs", icon: ScrollText, tone: "accent", disabled: i.status === "pending", hint: i.status === "pending" ? "Not deployed to this node yet" : undefined, onSelect: () => terminals.openLogs({ source: "service", appId: a.id, serverId: node.id, name: a.name, node: node.display_name ?? node.hostname }) },
              { label: "Start", icon: Play, tone: "ok", disabled: !!why || live === "running", hint: why ?? (live === "running" ? "Already running on this node" : undefined), onSelect: () => run(a, "start") },
              { label: "Restart", icon: RotateCw, tone: "warn", disabled: !!why, hint: why, onSelect: () => setConfirm({ app: a, action: "restart" }) },
              { label: "Stop", icon: Square, tone: "bad", disabled: !!why || live === "stopped", hint: why ?? (live === "stopped" ? "Already stopped on this node" : undefined), onSelect: () => setConfirm({ app: a, action: "stop" }) },
            ];
            const pressed = busy?.startsWith(`${a.id}:`);
            return (
              <tr key={a.id} {...rm.rowProps(actions, `Actions for ${a.name}`)}>
                <Td>
                  <Link to="/services/$serviceId" params={{ serviceId: a.id }} className="block max-w-[240px] truncate font-medium hover:text-accent" title={a.name}>
                    {a.name}
                  </Link>
                  {a.runtime === "container" && i.status !== "pending" && (
                    <Link
                      to="/nodes/$nodeId/containers/$containerId"
                      params={{ nodeId: node.id, containerId: `forge-app-${a.spec?.environment ?? ns?.name ?? ""}-${a.name}` }}
                      className="inline-flex max-w-[240px] items-center gap-1 font-mono text-[11px] text-muted hover:text-accent"
                      title="Open the container on this node"
                    >
                      <Container className="size-3 shrink-0" />
                      <span className="truncate">forge-app-{a.spec?.environment ?? ns?.name ?? ""}-{a.name}</span>
                    </Link>
                  )}
                </Td>
                <Td className="text-xs">
                  <span className="block max-w-[200px] truncate" title={`${ns?.name ?? a.environment_id}${cl ? ` · cluster ${cl.name}` : ""}`}>
                    {ns?.name ?? "—"}
                    {cl && (
                      <>
                        <span className="text-faint"> · </span>
                        <Link to="/clusters/$clusterId" params={{ clusterId: cl.id }} className="text-muted hover:text-accent">
                          {cl.name}
                        </Link>
                      </>
                    )}
                  </span>
                </Td>
                <Td>
                  <Badge>{kindOf(a)}</Badge>
                  {stack && <span className="ml-1 text-[11px] text-faint">stack</span>}
                </Td>
                <Td className="text-xs text-muted">{i.role}</Td>
                <Td>
                  <span title={`Updated ${time(i.updated_at)}`}>
                    <Status value={deployTone(i.status)} />
                  </span>
                  <span className="ml-1.5 text-xs text-muted">{i.status}</span>
                  {i.last_error && (
                    <div className="max-w-[240px] truncate text-xs text-bad" title={i.last_error}>
                      {i.last_error}
                    </div>
                  )}
                </Td>
                <Td>
                  <InstanceStateChip s={liveState(i, node)} />
                  <div className="font-mono text-[11px] text-faint tabular-nums" title={time(i.updated_at)}>
                    {ago(i.updated_at)}
                  </div>
                </Td>
                <Td>{stack ? <span className="text-xs text-faint" title="Compose services have no Forge health checks">—</span> : <HealthCell appId={a.id} node={node} />}</Td>
                <Td className="text-right whitespace-nowrap">
                  {pressed && <Loader2 className="mr-1 inline size-3.5 animate-spin text-muted" aria-label="Working" />}
                  <RowMenu actions={actions} label={`Actions for ${a.name}`} />
                </Td>
              </tr>
            );
          })}
        </Table>
      )}
      {rm.menu}
      <Confirm
        open={!!confirm}
        onOpenChange={(o) => !o && setConfirm(undefined)}
        title={confirm ? `${confirm.action === "stop" ? "Stop" : "Restart"} ${confirm.app.name} on ${node.display_name ?? node.hostname}?` : ""}
        body={
          confirm
            ? confirm.action === "stop"
              ? `The service stops on this node until you start it again. Ingress endpoints that route to it here lose this upstream; other nodes keep serving.`
              : `The service restarts on this node; expect a short interruption for requests routed here.`
            : ""
        }
        confirmLabel={confirm?.action === "stop" ? "Stop" : "Restart"}
        onConfirm={() => {
          if (confirm) run(confirm.app, confirm.action);
          setConfirm(undefined);
        }}
      />
    </Panel>
  );
}

/** Health of one app on this node, fetched only once the row scrolls into view (one request per app). */
function HealthCell({ appId, node }: { appId: string; node: Server }) {
  const ref = useRef<HTMLSpanElement>(null);
  const [seen, setSeen] = useState(false);
  useEffect(() => {
    const el = ref.current;
    if (!el || seen) return;
    if (typeof IntersectionObserver === "undefined") return setSeen(true);
    const io = new IntersectionObserver((es) => es.some((e) => e.isIntersecting) && setSeen(true));
    io.observe(el);
    return () => io.disconnect();
  }, [seen]);
  const q = useServiceHealth(appId, { enabled: seen, live: true });
  const name = node.display_name ?? node.hostname;
  return (
    <span ref={ref} className="inline-flex">
      {q.data ? (
        <InstanceHealthBadge health={q.data.instances.find((h) => h.server_id === node.id)} nodeName={name} />
      ) : q.isError ? (
        <span className="text-xs text-faint" title={q.error instanceof ApiError && q.error.status === 404 ? "This server doesn't report health" : q.error instanceof ApiError && q.error.status === 403 ? "Needs health.view on this service" : "Couldn't load health"}>
          —
        </span>
      ) : (
        <span className="text-xs text-faint" aria-label="Loading health">…</span>
      )}
    </span>
  );
}
