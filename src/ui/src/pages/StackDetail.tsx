import { Link, useNavigate, useParams } from "@tanstack/react-router";
import { ArrowLeft, Download, FileText, Globe, History, Layers, Loader2, Pencil, Play, PowerOff, RefreshCw, Rocket, RotateCw, Square, Trash2, X } from "lucide-react";
import { useState } from "react";

import { ApiError } from "../api/client";
import { useAllEnvironments, useServers } from "../api/hooks";
import { type StackDetail as Detail, type StackNode, jobServer, useDeleteStack, useRefreshStack, useStack, useStackServiceAction } from "../api/stacks";
import { nodeName, type Job } from "../api/types";
import { errorText, useToast } from "../app/toast";
import { NodeJobTabs } from "../components/JobLog";
import { WebhooksTab } from "../components/deploy/WebhooksTab";
import { Designer, Findings, NodeLink, StateChip, YamlEditor } from "../components/stacks/parts";
import { RevisionsPanel } from "../components/stacks/Revisions";
import { ServicePorts } from "../components/tunnels/ServicePorts";
import { DeployDialog, DownDialog, ExposeDialog, KindBadge, LogsDialog, UpdateDialog } from "../components/stacks/StackDialogs";
import { Badge, Button, actionHover, Confirm, CopyButton, CopyIcon, Empty, ErrorNote, IconButton, PageHeader, Panel, RowMenu, type RowAction, Skeleton, Table, Td, ago, time, useRowMenu } from "../components/ui";

type Tab = "services" | "compose" | "revisions" | "webhooks";
type ServiceAction = "start" | "stop" | "restart" | "pull";

export function StackDetail() {
  const { stackId } = useParams({ from: "/stacks/$stackId" });
  const q = useStack(stackId);
  const [tab, setTab] = useState<Tab>("services");

  if (q.isLoading) return <Skeleton rows={6} />;
  if (q.error || !q.data) {
    const missing = q.error instanceof ApiError && q.error.status === 404;
    return (
      <Panel>
        {missing ? (
          <Empty icon={<Layers className="size-5" />} title="Stack not found" action={<Link to="/stacks"><Button>Back to stacks</Button></Link>}>
            It was deleted, or you cannot view its environment.
          </Empty>
        ) : (
          <>
            <ErrorNote error={q.error ?? "The stack could not be loaded."} />
            <div className="px-3 pb-3">
              <Button onClick={() => void q.refetch()}>
                <RefreshCw className="size-4" /> Try again
              </Button>
            </div>
          </>
        )}
      </Panel>
    );
  }
  return <StackView d={q.data} tab={tab} setTab={setTab} fetching={q.isFetching} />;
}

function StackView({ d, tab, setTab, fetching }: { d: Detail; tab: Tab; setTab: (t: Tab) => void; fetching: boolean }) {
  const toast = useToast();
  const navigate = useNavigate();
  const environments = useAllEnvironments().data ?? [];
  const servers = useServers().data ?? [];
  const ns = environments.find((n) => n.id === d.stack.environment_id);
  const refresh = useRefreshStack();
  const del = useDeleteStack();
  const svcAction = useStackServiceAction();
  const [jobs, setJobs] = useState<Job[]>([]);
  const [dialog, setDialog] = useState<
    | { k: "deploy"; revision?: number }
    | { k: "update" }
    | { k: "down"; node?: StackNode }
    | { k: "expose" }
    | { k: "delete" }
    | { k: "logs"; service: string; server: string }
    | null
  >(null);
  const [pending, setPending] = useState<string>();
  const close = () => setDialog(null);

  const host = (id: string) => {
    const s = servers.find((x) => x.id === id);
    return s ? nodeName(s) : id.slice(0, 8) || "node";
  };
  // The server refuses Delete while any node still has a part (409).
  const live = d.nodes.length > 0;
  const busy = d.nodes.some((n) => /ing$/.test(n.status));
  const s = d.stack;

  const runService = (node: StackNode, action: ServiceAction, services?: string[]) => {
    const key = `${node.server_id}:${action}:${services?.join(",") ?? "*"}`;
    setPending(key);
    svcAction.mutate(
      { id: s.id, server_id: node.server_id, action, services },
      {
        onSuccess: (js) => {
          toast(`${action} ${services?.join(", ") ?? "all services"} on ${host(node.server_id)}`);
          setJobs(js);
        },
        onError: (e) => toast(errorText(e), "error"),
        onSettled: () => setPending(undefined),
      },
    );
  };

  return (
    <>
      <PageHeader
        eyebrow={`Stacks / ${ns?.name ?? "—"}`}
        title={s.name}
        actions={
          <>
            <IconButton
              label="Refresh state from the nodes"
              disabled={refresh.isPending || !live}
              onClick={() =>
                refresh.mutate(s.id, {
                  onSuccess: (js) => {
                    toast(`Reading state on ${js.length} node${js.length === 1 ? "" : "s"}`);
                    setJobs(js);
                  },
                  onError: (e) => toast(errorText(e), "error"),
                })
              }
            >
              <RefreshCw className={`size-4 ${refresh.isPending || fetching ? "animate-spin" : ""}`} />
            </IconButton>
            <Button onClick={() => setDialog({ k: "update" })}>
              <Pencil className="size-4" /> Update
            </Button>
            <Button variant="primary" disabled={busy} title={busy ? "A job is running on a node" : undefined} onClick={() => setDialog({ k: "deploy" })}>
              <Rocket className="size-4" /> Deploy
            </Button>
            <RowMenu
              label="More stack actions"
              actions={[
                { label: "Revisions", icon: History, onSelect: () => setTab("revisions") },
                { label: "Expose via Nginx", icon: Globe, disabled: !d.preview, hint: "The stack has no valid preview", onSelect: () => setDialog({ k: "expose" }) },
                { label: "Down…", icon: PowerOff, danger: true, disabled: !live || busy, hint: !live ? "Not deployed" : "A job is running", onSelect: () => setDialog({ k: "down" }) },
                { label: "Delete…", icon: Trash2, danger: true, disabled: live, hint: "Take it down on every node first", onSelect: () => setDialog({ k: "delete" }) },
              ]}
            />
          </>
        }
      />
      <dl className="-mt-3 mb-4 flex flex-wrap items-center gap-x-5 gap-y-1 text-xs">
        <Meta k="Kind">
          <KindBadge kind={s.kind} />
        </Meta>
        <Meta k="Environment">
          {ns ? (
            <Link to="/stacks" search={{ ns: ns.id }} className="hover:text-accent">
              {ns.name}
            </Link>
          ) : (
            "—"
          )}
        </Meta>
        <Meta k="Revision">
          <span className="font-mono tabular-nums">r{s.revision}</span>
        </Meta>
        <Meta k="Deployed">
          <span className="font-mono tabular-nums">{s.deployed_revision ? `r${s.deployed_revision}` : "—"}</span>
          {s.deployed_revision > 0 && s.revision > s.deployed_revision && (
            <span className="ml-1.5">
              <Badge tone="warn">r{s.revision} not deployed</Badge>
            </span>
          )}
        </Meta>
        <Meta k="Folder">
          <span className="inline-flex min-w-0 items-center">
            <span className="max-w-[320px] truncate font-mono text-[11px]" title={d.dir || undefined}>
              {d.dir || "—"}
            </span>
            {d.dir && <CopyIcon text={d.dir} label="Copy folder" />}
          </span>
        </Meta>
        <Meta k="Created from">
          <NodeLink id={s.default_server} />
        </Meta>
        <Meta k="Updated">
          <span title={time(s.updated_at)}>{s.updated_at ? ago(s.updated_at) : "—"}</span>
        </Meta>
      </dl>

      {jobs.length > 0 && (
        <Panel
          title="Last action"
          className="mb-4"
          actions={
            <IconButton label="Hide" onClick={() => setJobs([])}>
              <X className="size-4" />
            </IconButton>
          }
        >
          <div className="p-3">
            <NodeJobTabs jobs={jobs.map((j) => ({ nodeId: jobServer(j) || j.id, nodeName: host(jobServer(j)), jobId: j.id }))} />
          </div>
        </Panel>
      )}

      <div className="mb-4 flex gap-1 border-b border-line" role="tablist">
        {(
          [
            ["services", "Services"],
            ["compose", "Compose"],
            ["revisions", "Revisions"],
            ["webhooks", "Webhooks"],
          ] as const
        ).map(([id, label]) => (
          <button key={id} role="tab" aria-selected={tab === id} onClick={() => setTab(id)} className={`press -mb-px border-b-2 px-3 py-2 text-[13px] focus-visible:outline-2 focus-visible:outline-accent ${tab === id ? "border-accent text-accent" : "border-transparent text-muted hover:text-fg"}`}>
            {label}
            {id === "revisions" && <span className="ml-1.5 font-mono text-[11px] text-faint">{s.revision}</span>}
          </button>
        ))}
      </div>

      <div key={tab} className="view-enter">
        {tab === "services" ? (
          d.nodes.length === 0 ? (
            <Panel>
              <Empty icon={<Rocket className="size-5" />} title="Not deployed yet" action={<Button variant="primary" onClick={() => setDialog({ k: "deploy" })}>Deploy</Button>}>
                Deploy writes the compose file to each node and starts the services you pick.
              </Empty>
            </Panel>
          ) : (
            <div className="space-y-4">
              {d.nodes.map((n) => (
                <NodePart key={n.server_id} d={d} n={n} pending={pending} onAction={runService} onLogs={(service) => setDialog({ k: "logs", service, server: n.server_id })} onDown={() => setDialog({ k: "down", node: n })} />
              ))}
              <ServicePorts kind="stack" service={s.name} />
            </div>
          )
        ) : tab === "compose" ? (
          <div className="grid gap-4 xl:grid-cols-2">
            <Panel title={`compose.yaml · r${s.revision}`} actions={d.content ? <CopyButton text={d.content} /> : undefined} className="min-w-0">
              <div className="p-3">
                <YamlEditor value={d.content} docKey={`view-${s.id}-${s.revision}`} onChange={() => {}} readOnly height="h-[60vh]" />
                <p className="mt-2 text-[11px] text-muted">
                  .env: {s.env_keys.length ? <span className="font-mono">{s.env_keys.join(", ")}</span> : "no variables"} <span className="text-faint">(values are sealed and never shown)</span>
                </p>
              </div>
            </Panel>
            <Panel title="Placement and policy" className="min-w-0">
              <div className="max-h-[66vh] overflow-y-auto p-3">{d.preview ? <Designer preview={d.preview} /> : <p className="rounded-md bg-bad-soft px-3 py-2 text-xs text-bad">{d.preview_error ?? "No preview available."}</p>}</div>
            </Panel>
          </div>
        ) : tab === "webhooks" ? (
          <WebhooksTab target={{ kind: "stack", id: s.id }} targetName={s.name} />
        ) : (
          <Panel>
            <RevisionsPanel d={d} onRollback={(rev) => setDialog({ k: "deploy", revision: rev })} />
          </Panel>
        )}
      </div>

      {dialog?.k === "deploy" && <DeployDialog d={d} revision={dialog.revision} onClose={close} onJobs={setJobs} />}
      {dialog?.k === "update" && <UpdateDialog d={d} onClose={close} onSaved={() => setTab("services")} />}
      {dialog?.k === "down" && <DownDialog d={d} node={dialog.node} onClose={close} onJobs={setJobs} />}
      {dialog?.k === "expose" && <ExposeDialog d={d} onClose={close} />}
      {dialog?.k === "logs" && <LogsDialog d={d} service={dialog.service} server={dialog.server} onClose={close} />}
      <Confirm
        open={dialog?.k === "delete"}
        onOpenChange={(o) => !o && close()}
        title={`Delete stack ${s.name}?`}
        body={
          <>
            Deletes the stack, its {s.revision} revision{s.revision === 1 ? "" : "s"} and its sealed .env. Services registered from it disappear from Services. Files on the nodes are kept.
            {live && <span className="mt-2 block text-bad">It is still deployed: take it down on every node first.</span>}
          </>
        }
        confirmLabel="Delete"
        busy={del.isPending}
        disabled={live}
        onConfirm={() =>
          del.mutate(s.id, {
            onSuccess: () => {
              toast(`Stack ${s.name} deleted`);
              void navigate({ to: "/stacks", search: { ns: s.environment_id } });
            },
            onError: (e) => toast(e instanceof ApiError && e.status === 409 ? "The stack is still deployed; take it down first." : errorText(e), "error"),
          })
        }
      />
      <div className="mt-6">
        <Link to="/stacks" search={{ ns: s.environment_id }} className="inline-flex items-center gap-1 text-xs text-muted hover:text-accent">
          <ArrowLeft className="size-3.5" /> All stacks in {ns?.name ?? "this environment"}
        </Link>
      </div>
    </>
  );
}

function Meta({ k, children }: { k: string; children: React.ReactNode }) {
  return (
    <div className="flex min-w-0 items-center gap-1.5">
      <dt className="text-muted">{k}</dt>
      <dd className="min-w-0">{children}</dd>
    </div>
  );
}

/** One node's part: its status and a row per service from the last `compose ps`. */
function NodePart({
  d,
  n,
  pending,
  onAction,
  onLogs,
  onDown,
}: {
  d: Detail;
  n: StackNode;
  pending?: string;
  onAction: (n: StackNode, a: ServiceAction, services?: string[]) => void;
  onLogs: (service: string) => void;
  onDown: () => void;
}) {
  const busy = /ing$/.test(n.status);
  const state = Array.isArray(n.state) ? n.state : [];
  const names = [...new Set([...(n.services ?? []), ...state.map((x) => x.service).filter(Boolean)])];
  const ports = (d.preview?.nodes ?? []).find((p) => p.server_id === n.server_id)?.ports ?? [];
  const findings = (d.preview?.nodes ?? []).find((p) => p.server_id === n.server_id)?.findings ?? [];
  const spin = (a: string) => pending === `${n.server_id}:${a}:*`;
  const rm = useRowMenu();
  return (
    <Panel
      className="min-w-0"
      title={undefined}
      actions={undefined}
    >
      <header className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1 border-b border-line px-4 py-2">
        <NodeLink id={n.server_id} />
        <StateChip value={n.status} />
        <span className="font-mono text-[11px] text-muted tabular-nums">r{n.revision || "—"}</span>
        <span className="text-[11px] text-faint" title={time(n.updated_at)}>
          {n.updated_at ? ago(n.updated_at) : ""}
        </span>
        {n.last_job_id && (
          <Link to="/jobs" className="font-mono text-[11px] text-faint hover:text-accent" title={`Last job ${n.last_job_id}`}>
            job {n.last_job_id.slice(0, 8)}
          </Link>
        )}
        <span className="flex-1" />
        <Button className="h-7 px-2 text-xs" disabled={busy || !!pending} onClick={() => onAction(n, "pull")} title="Pull the images of this node's services">
          {spin("pull") ? <Loader2 className="size-3.5 animate-spin" /> : <Download className="size-3.5" />} Pull
        </Button>
        <Button className={`h-7 px-2 text-xs ${actionHover.warn}`} disabled={busy || !!pending} onClick={() => onAction(n, "restart")} title="Restart every service on this node (short outage)">
          {spin("restart") ? <Loader2 className="size-3.5 animate-spin" /> : <RotateCw className="size-3.5" />} Restart all
        </Button>
        <Button variant="danger" className="h-7 px-2 text-xs" disabled={busy || n.status === "pending"} title={busy ? "A job is running on this node" : n.status === "pending" ? "Not deployed on this node yet" : "Stop and remove this node's containers"} onClick={onDown}>
          <PowerOff className="size-3.5" /> Down
        </Button>
      </header>
      {n.last_error && (
        <div role="alert" className="border-b border-line bg-bad-soft px-4 py-1.5 font-mono text-[11.5px] break-words whitespace-pre-wrap text-bad">
          {n.last_error}
        </div>
      )}
      {findings.length > 0 && (
        <div className="border-b border-line px-4 py-1.5">
          <Findings list={findings} />
        </div>
      )}
      {names.length === 0 ? (
        <p className="px-4 py-3 text-xs text-muted">No services on this node.</p>
      ) : (
        <Table head={["Service", "State", "Health", "Ports", "Image", ""]}>
          {names.map((name) => {
            const st = state.filter((x) => x.service === name);
            const first = st[0];
            const up = st.some((x) => x.state === "running");
            const svcPorts = ports.filter((p) => p.service === name);
            const selected = (n.selected ?? []).length === 0 || n.selected.includes(name);
            const working = pending?.startsWith(`${n.server_id}:`) && pending.endsWith(`:${name}`);
            const actions: RowAction[] = [
              { label: "Logs", icon: FileText, tone: "accent", onSelect: () => onLogs(name) },
              up
                ? { label: "Stop", icon: Square, tone: "bad", disabled: busy || !!pending, hint: "A job is running", onSelect: () => onAction(n, "stop", [name]) }
                : { label: "Start", icon: Play, tone: "ok", disabled: busy || !!pending, hint: "A job is running", onSelect: () => onAction(n, "start", [name]) },
              { label: "Restart", icon: RotateCw, tone: "warn", disabled: busy || !!pending || !first, hint: !first ? "Not created yet; deploy or start it" : "A job is running", onSelect: () => onAction(n, "restart", [name]) },
              { label: "Pull image", icon: Download, disabled: busy || !!pending, hint: "A job is running", onSelect: () => onAction(n, "pull", [name]) },
            ];
            return (
              <tr key={name} className="hover:bg-panel-2/60" {...rm.rowProps(actions, `Actions for ${name}`)}>
                <Td className="max-w-[220px]">
                  <div className="flex min-w-0 items-center gap-1.5">
                    <span className="truncate font-mono text-[13px] font-medium" title={name}>
                      {name}
                    </span>
                    {working && <Loader2 className="size-3.5 shrink-0 animate-spin text-accent" />}
                  </div>
                  <div className="flex min-w-0 flex-wrap gap-x-2 font-mono text-[11px] text-faint" title={st.map((x) => x.name).join(", ") || undefined}>
                    {st.length === 0
                      ? selected
                        ? "no container"
                        : "not selected"
                      : st
                          .filter((x) => x.name)
                          .map((x) => (
                            <Link
                              key={x.name}
                              to="/nodes/$nodeId/containers/$containerId"
                              params={{ nodeId: n.server_id, containerId: x.name }}
                              className="max-w-full truncate hover:text-accent"
                              title={`Open container ${x.name}`}
                            >
                              {x.name}
                            </Link>
                          ))}
                  </div>
                </Td>
                <Td>
                  {first ? (
                    <>
                      <StateChip value={first.state} />
                      <div className="max-w-[180px] truncate text-[11px] text-faint" title={first.status}>
                        {first.status}
                        {first.exit_code !== null && first.state !== "running" && ` · exit ${first.exit_code}`}
                      </div>
                    </>
                  ) : (
                    <span className="text-xs text-faint">—</span>
                  )}
                </Td>
                <Td>{first?.health ? <StateChip value={first.health} /> : <span className="text-xs text-faint">—</span>}</Td>
                <Td className="max-w-[220px]">
                  {svcPorts.length ? (
                    <div className="space-y-0.5">
                      {svcPorts.map((p, i) => (
                        <div key={i} className="flex min-w-0 items-center gap-1.5">
                          <span className="truncate font-mono text-[11px] tabular-nums" title={`${p.host_ip}:${p.published} → ${p.target}/${p.protocol}`}>
                            {p.host_ip ? `${p.host_ip}:` : ""}
                            {p.published || "auto"}→{p.target}/{p.protocol}
                          </span>
                          {p.public ? <Badge tone="warn">public</Badge> : <Badge>internal</Badge>}
                        </div>
                      ))}
                    </div>
                  ) : first?.ports ? (
                    <span className="block truncate font-mono text-[11px]" title={first.ports}>
                      {first.ports}
                    </span>
                  ) : (
                    <span className="text-xs text-faint">—</span>
                  )}
                </Td>
                <Td className="max-w-[220px]">
                  <span className="block truncate font-mono text-[11px] text-muted" title={first?.image}>
                    {first?.image || d.preview?.services.find((x) => x.name === name)?.image || "—"}
                  </span>
                </Td>
                <Td className="text-right whitespace-nowrap">
                  <Button variant="ghost" className="mr-1 h-7 px-2 text-xs" onClick={() => onLogs(name)} title="Log tail of this service">
                    <FileText className="size-3.5" /> Logs
                  </Button>
                  <RowMenu label={`Actions for ${name}`} actions={actions.slice(1)} />
                </Td>
              </tr>
            );
          })}
        </Table>
      )}
      {rm.menu}
    </Panel>
  );
}
