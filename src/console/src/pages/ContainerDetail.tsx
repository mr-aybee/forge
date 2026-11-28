import * as Tabs from "@radix-ui/react-tabs";
import {
  Link,
  useNavigate,
  useParams,
  useSearch,
} from "@tanstack/react-router";
import {
  ChevronRight,
  Container,
  Eye,
  EyeOff,
  ExternalLink,
  KeyRound,
  LayoutDashboard,
  Loader2,
  Network,
  Play,
  RefreshCw,
  RotateCw,
  ScrollText,
  ShieldCheck,
  Square,
  SquareTerminal,
  Trash2,
} from "lucide-react";
import { useEffect, useState } from "react";

import { ApiError } from "../api/client";
import {
  useCanRevealEnv,
  useContainerAction,
  useContainerInspect,
  useContainerStats,
  useServer,
} from "../api/hooks";
import { nodeName, type ContainerInspect, type Server } from "../api/types";
import type { ContainerTab } from "../app/router";
import { sessionLabel, useTerminals } from "../app/terminals";
import { errorText, useToast } from "../app/toast";
import { ContainerCharts } from "../components/containers/ContainerCharts";
import { ContainerLogs } from "../components/containers/ContainerLogs";
import { StoredContainerLogs } from "../components/containers/StoredContainerLogs";
import {
  useContainerOwner,
  type ContainerOwner,
} from "../components/containers/owner";
import { OwnerLink } from "../components/containers/OwnerLink";
import { StateChip } from "../components/stacks/parts";
import {
  Badge,
  Button,
  Confirm,
  CopyIcon,
  Empty,
  ErrorNote,
  IconButton,
  PageCrumb,
  PageHeader,
  SearchInput,
  Skeleton,
  ago,
  bytes,
  time,
  actionHover,
} from "../components/ui";

const trigger =
  "flex items-center gap-2 border-b-2 border-transparent px-3 py-2.5 text-[13px] text-muted transition hover:text-fg data-[state=active]:border-accent data-[state=active]:font-medium data-[state=active]:text-fg";

type Act = "start" | "stop" | "restart" | "remove";

/** One Docker container on a node: overview, logs, shell and environment, each a URL tab. */
export function ContainerDetail() {
  const { nodeId, containerId } = useParams({
    from: "/nodes/$nodeId/containers/$containerId",
  });
  const { tab } = useSearch({ from: "/nodes/$nodeId/containers/$containerId" });
  const navigate = useNavigate();
  const toast = useToast();
  const node = useServer(nodeId);
  const online = node.data?.status === "online";
  const q = useContainerInspect(nodeId, containerId, { enabled: online });
  const act = useContainerAction(nodeId);
  const terminals = useTerminals();
  const [confirm, setConfirm] = useState<Act>();
  const [pending, setPending] = useState<Act>();

  const c = q.data?.result;
  const owner = useContainerOwner(c?.name, c?.labels, !!c);
  const n = node.data;
  const nodeLabel = n ? nodeName(n) : nodeId.slice(0, 8);
  const title = c?.name || containerId.slice(0, 12);

  const setTab = (t: string) =>
    void navigate({
      to: "/nodes/$nodeId/containers/$containerId",
      params: { nodeId, containerId },
      search: { tab: t === "overview" ? undefined : (t as ContainerTab) },
      replace: true,
    });

  const run = (op: Act) => {
    setPending(op);
    act.mutate(
      { op, id: c?.id || containerId },
      {
        onSuccess: (r) => {
          if (r.status === "failed") toast(r.error ?? `${op} failed`, "error");
          else
            toast(
              r.status === "running"
                ? `${op} started — follow it in Jobs`
                : `${op} ${title} done`,
            );
          if (op === "remove" && r.status !== "failed")
            void navigate({
              to: "/nodes/$nodeId",
              params: { nodeId },
              search: { tab: "containers" },
            });
          else void q.refetch();
        },
        onError: (e) => toast(errorText(e), "error"),
        onSettled: () => setPending(undefined),
      },
    );
  };

  const shellSessions = terminals.sessions.filter(
    (s) =>
      s.kind === "container" &&
      s.serverId === nodeId &&
      (s.containerId === containerId || (c && s.containerId === c.id)),
  );
  const openShell = () => {
    if (!n) return;
    terminals.openContainer(n, { id: c?.id || containerId, name: c?.name });
  };

  const busy = act.isPending;
  const why = !online
    ? `The node's agent is ${n?.status ?? "offline"}`
    : busy
      ? "Another container action is running"
      : undefined;
  const running = !!c?.running;

  // Whole-page states: node missing, inspect failed, container gone.
  const missing =
    q.data?.status === "failed" &&
    /no such container|not found|404/i.test(q.data.error ?? "");
  const failed = q.data?.status === "failed" || q.isError;

  return (
    <>
      <PageCrumb
        back={
          <Link
            to="/nodes/$nodeId"
            params={{ nodeId }}
            search={{ tab: "containers" }}
          >
            {nodeLabel} / Containers
          </Link>
        }
        current={title}
      />
      <PageHeader
        title={title}
        description={
          c
            ? `${c.image || "—"} · ${c.state}${c.started_at && running ? ` · up ${ago(c.started_at).replace(/ ago$/, "")}` : ""}`
            : online
              ? "Reading container details…"
              : "The agent must be online to inspect containers."
        }
        actions={
          <div className="flex flex-wrap items-center gap-1.5">
            <Button
              variant="primary"
              disabled={!online || !running}
              title={
                !online
                  ? why
                  : !running
                    ? "The container is not running"
                    : "Open a shell inside the container (docker exec)"
              }
              onClick={openShell}
            >
              <SquareTerminal className="size-4" /> Open shell
            </Button>
            {running ? (
              <Button
                className={actionHover.bad}
                disabled={!!why}
                title={why}
                onClick={() => setConfirm("stop")}
              >
                {pending === "stop" ? (
                  <Loader2 className="size-4 animate-spin" />
                ) : (
                  <Square className="size-4" />
                )}{" "}
                Stop
              </Button>
            ) : (
              <Button
                className={actionHover.ok}
                disabled={!!why || !c}
                title={why}
                onClick={() => run("start")}
              >
                {pending === "start" ? (
                  <Loader2 className="size-4 animate-spin" />
                ) : (
                  <Play className="size-4" />
                )}{" "}
                Start
              </Button>
            )}
            <Button
              className={actionHover.warn}
              disabled={!!why || !c}
              title={why}
              onClick={() => setConfirm("restart")}
            >
              {pending === "restart" ? (
                <Loader2 className="size-4 animate-spin" />
              ) : (
                <RotateCw className="size-4" />
              )}{" "}
              Restart
            </Button>
            <Button
              variant="danger"
              disabled={!!why || !c}
              title={why}
              onClick={() => setConfirm("remove")}
            >
              {pending === "remove" ? (
                <Loader2 className="size-4 animate-spin" />
              ) : (
                <Trash2 className="size-4" />
              )}{" "}
              Remove
            </Button>
            <IconButton
              label="Refresh"
              onClick={() => void q.refetch()}
              disabled={!online}
            >
              <RefreshCw
                className={`size-4 ${q.isFetching ? "animate-spin" : ""}`}
              />
            </IconButton>
          </div>
        }
      />

      {/* Relationship line: node → container → owner */}
      <div className="-mt-2 mb-4 flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 text-xs text-muted">
        <Link
          to="/nodes/$nodeId"
          params={{ nodeId }}
          className="hover:text-accent"
        >
          {nodeLabel}
        </Link>
        <span className="text-faint">→</span>
        <span className="inline-flex min-w-0 items-center gap-1">
          <Container className="size-3.5 shrink-0 text-faint" />
          <span
            className="truncate font-mono text-[11px] text-fg"
            title={c?.id || containerId}
          >
            {(c?.id || containerId).slice(0, 12)}
          </span>
          <CopyIcon text={c?.id || containerId} label="Copy container id" />
        </span>
        {owner && (
          <>
            <span className="text-faint">→</span>
            <OwnerLink owner={owner} />
          </>
        )}
        {c?.health && (
          <>
            <span className="text-faint">·</span>
            <StateChip value={c.health} />
          </>
        )}
      </div>

      {node.isError ? (
        <Empty
          icon={<Container className="size-5" />}
          title="Node not found"
          action={
            <Link to="/nodes">
              <Button>Back to nodes</Button>
            </Link>
          }
        >
          <ErrorNote error={node.error} />
        </Empty>
      ) : !online && n ? (
        <Empty
          icon={<Container className="size-5" />}
          title={`${nodeLabel} is ${n.status}`}
        >
          The node's agent must be online to inspect, log into or control
          containers.
        </Empty>
      ) : missing ? (
        <Empty
          icon={<Container className="size-5" />}
          title="Container not found"
          action={
            <Link
              to="/nodes/$nodeId"
              params={{ nodeId }}
              search={{ tab: "containers" }}
            >
              <Button>Back to the node's containers</Button>
            </Link>
          }
        >
          It was removed, or <span className="font-mono">{containerId}</span> is
          not a container on {nodeLabel}.
        </Empty>
      ) : (
        <Tabs.Root value={tab ?? "overview"} onValueChange={setTab}>
          <Tabs.List
            className="mb-4 flex gap-1 overflow-x-auto border-b border-line"
            aria-label="Container sections"
          >
            <Tabs.Trigger value="overview" className={trigger}>
              <LayoutDashboard className="size-4" /> Overview
            </Tabs.Trigger>
            <Tabs.Trigger value="logs" className={trigger}>
              <ScrollText className="size-4" /> Logs
            </Tabs.Trigger>
            <Tabs.Trigger value="shell" className={trigger}>
              <SquareTerminal className="size-4" /> Shell
              {shellSessions.length > 0 && (
                <Badge tone="accent">{shellSessions.length}</Badge>
              )}
            </Tabs.Trigger>
            <Tabs.Trigger value="environment" className={trigger}>
              <KeyRound className="size-4" /> Environment
              {c && (
                <span className="font-mono text-[10px] text-faint tabular-nums">
                  {c.env_keys.length}
                </span>
              )}
            </Tabs.Trigger>
          </Tabs.List>

          {failed && !missing && (
            <ErrorNote
              error={
                q.data?.error ?? q.error ?? "Inspecting the container failed"
              }
            />
          )}

          <Tabs.Content value="overview">
            {c ? (
              <div className="space-y-4">
                <Overview c={c} owner={owner} node={n} nodeId={nodeId} />
                <ContainerCharts server={nodeId} name={c.name} />
              </div>
            ) : (
              <Skeleton rows={6} />
            )}
          </Tabs.Content>
          <Tabs.Content value="logs">
            {c?.name ? (
              <StoredContainerLogs server={nodeId} name={c.name} containerId={c.id || containerId} height="h-[62vh]" />
            ) : (
              <ContainerLogs server={nodeId} containerId={c?.id || containerId} height="h-[62vh]" />
            )}
          </Tabs.Content>
          <Tabs.Content value="shell">
            <ShellTab
              node={n}
              c={c}
              sessions={shellSessions}
              running={running}
              onOpen={openShell}
            />
          </Tabs.Content>
          <Tabs.Content value="environment">
            {c ? (
              <EnvironmentTab
                nodeId={nodeId}
                containerId={c.id || containerId}
                c={c}
                owner={owner}
              />
            ) : (
              <Skeleton rows={4} />
            )}
          </Tabs.Content>
        </Tabs.Root>
      )}

      <Confirm
        open={!!confirm}
        onOpenChange={(o) => !o && setConfirm(undefined)}
        title={
          confirm === "remove"
            ? `Remove ${title}?`
            : confirm === "stop"
              ? `Stop ${title}?`
              : `Restart ${title}?`
        }
        body={
          confirm === "remove"
            ? `Stops and deletes the container on ${nodeLabel}. Named volumes and bind-mounted data are kept.${owner?.kind === "app" ? ` ${owner.name} is the Forge service behind it; deploy it again to recreate the container.` : owner?.kind === "stack" ? ` It belongs to stack ${owner.name}; deploy the stack again to recreate it.` : ""}`
            : confirm === "stop"
              ? `Stops ${title} on ${nodeLabel}. Anything that connects to its ports${c?.ports.length ? ` (${c.ports.map((p) => p.container).join(", ")})` : ""} loses this upstream until it is started again.`
              : `Restarts ${title} on ${nodeLabel}; expect a short outage of its ports${c?.ports.length ? ` (${c.ports.map((p) => p.container).join(", ")})` : ""}.`
        }
        confirmLabel={
          confirm === "remove"
            ? "Remove"
            : confirm === "stop"
              ? "Stop"
              : "Restart"
        }
        onConfirm={() => {
          if (confirm) run(confirm);
          setConfirm(undefined);
        }}
      />
    </>
  );
}

const dash = <span className="text-faint">—</span>;
const when = (ms?: number) =>
  ms ? <span title={time(ms)}>{ago(ms)}</span> : dash;
const upFor = (ms?: number) => (ms ? ago(ms).replace(/ ago$/, "") : "");

/** One fact in the summary strip: small label above a one-line value. */
function Fact({
  k,
  children,
  hint,
}: {
  k: string;
  children: React.ReactNode;
  hint?: string;
}) {
  return (
    <div className="min-w-0 px-3 py-2.5">
      <div className="mb-0.5 text-[11px] text-faint" title={hint}>
        {k}
      </div>
      <div className="flex min-h-6 min-w-0 items-center gap-1.5 text-[13px]">
        {children}
      </div>
    </div>
  );
}

/** A section card: header strip with title, count and actions, then a body. */
function Section({
  title,
  count,
  children,
  actions,
  className = "",
}: {
  title: React.ReactNode;
  count?: number;
  children: React.ReactNode;
  actions?: React.ReactNode;
  className?: string;
}) {
  return (
    <section
      className={`min-w-0 overflow-hidden rounded-lg border border-line bg-panel ${className}`}
    >
      <div className="flex min-h-9 flex-wrap items-center gap-2 border-b border-line bg-panel-2 px-3 py-1.5">
        <h3 className="text-xs font-semibold text-fg">{title}</h3>
        {count !== undefined && (
          <span className="rounded bg-panel px-1.5 font-mono text-[10px] text-muted tabular-nums">
            {count}
          </span>
        )}
        {actions && (
          <div className="ml-auto flex min-w-0 flex-wrap items-center gap-2">
            {actions}
          </div>
        )}
      </div>
      {children}
    </section>
  );
}

/** A one-line note inside a card body (empty state or hint). */
function CardNote({ children }: { children: React.ReactNode }) {
  return <p className="px-3 py-3 text-xs text-faint">{children}</p>;
}

function Kv({ k, children }: { k: string; children: React.ReactNode }) {
  return (
    <div className="grid min-h-8 grid-cols-[88px_minmax(0,1fr)] items-center gap-x-3 px-3 py-1">
      <dt className="text-xs text-muted">{k}</dt>
      <dd className="flex min-w-0 items-center">{children}</dd>
    </div>
  );
}

/** A small horizontal meter: bar plus value text. */
function Meter({
  pct,
  children,
  title,
}: {
  pct?: number;
  children: React.ReactNode;
  title?: string;
}) {
  const tone =
    pct === undefined
      ? "bg-accent"
      : pct > 90
        ? "bg-bad"
        : pct > 75
          ? "bg-warn"
          : "bg-accent";
  return (
    <span className="flex w-full min-w-0 items-center gap-2" title={title}>
      <span
        className="h-1.5 max-w-40 min-w-12 flex-1 overflow-hidden rounded-full bg-panel-2"
        aria-hidden
      >
        {pct !== undefined && (
          <span
            className={`block h-full rounded-full ${tone}`}
            style={{
              width: `${Math.max(pct > 0 ? 2 : 0, Math.min(100, pct))}%`,
            }}
          />
        )}
      </span>
      <span className="shrink-0 font-mono text-[11px] whitespace-nowrap tabular-nums">
        {children}
      </span>
    </span>
  );
}

function Cmd({ argv }: { argv?: string[] }) {
  if (!argv || argv.length === 0)
    return (
      <span
        className="text-xs text-faint italic"
        title="Not overridden; the image's own setting applies"
      >
        image default
      </span>
    );
  const s = argv
    .map((a) => (/[\s"'$]/.test(a) ? JSON.stringify(a) : a))
    .join(" ");
  return (
    <span className="inline-flex max-w-full min-w-0 items-center gap-1">
      <code
        className="truncate rounded bg-panel-2 px-1.5 py-0.5 font-mono text-[11px] select-text"
        title={s}
      >
        {s}
      </code>
      <CopyIcon text={s} label="Copy command" />
    </span>
  );
}

type PortGroup = {
  key: string;
  hostPort: string;
  cport: string;
  proto: string;
  binds: string[];
};

/** Group published ports by host port, container port and protocol, so the
 *  IPv4 and IPv6 bindings of one mapping show as one row. */
function groupPorts(ports: ContainerInspect["ports"] | undefined): PortGroup[] {
  const out = new Map<string, PortGroup>();
  for (const p of ports ?? []) {
    const [cport = "", proto = "tcp"] = (p.container || "").split("/");
    const i = p.host ? p.host.lastIndexOf(":") : -1;
    const hostPort = p.host && i >= 0 ? p.host.slice(i + 1) : (p.host ?? "");
    const bind = p.host && i >= 0 ? p.host.slice(0, i) : "";
    const key = `${hostPort}|${cport}|${proto}`;
    const g = out.get(key) ?? { key, hostPort, cport, proto, binds: [] };
    if (bind && !g.binds.includes(bind)) g.binds.push(bind);
    out.set(key, g);
  }
  return [...out.values()];
}

const bindTitle = (b: string) =>
  b === "::"
    ? "Bound on all IPv6 addresses"
    : b === "0.0.0.0"
      ? "Bound on all IPv4 addresses"
      : `Bound on ${b}`;

function Overview({
  c,
  owner,
  node,
  nodeId,
}: {
  c: ContainerInspect;
  owner: ContainerOwner;
  node?: Server;
  nodeId: string;
}) {
  const stats = useContainerStats(nodeId, c.id, c.running);
  const [labelFilter, setLabelFilter] = useState("");
  const labels = Object.entries(c.labels ?? {}).sort(
    ([a], [b]) =>
      Number(!a.startsWith("forge.")) - Number(!b.startsWith("forge.")) ||
      a.localeCompare(b),
  );
  const lf = labelFilter.trim().toLowerCase();
  const shownLabels = lf
    ? labels.filter(
        ([k, v]) =>
          k.toLowerCase().includes(lf) || v.toLowerCase().includes(lf),
      )
    : labels;
  const ips = new Map((c.network_ips ?? []).map((n) => [n.name, n.ip]));
  const networks = c.networks ?? [];
  const mounts = c.mounts ?? [];
  const ports = groupPorts(c.ports);
  const digest = c.image_id?.replace(/^sha256:/, "").slice(0, 12);
  const s = stats.data;
  const memPct =
    s?.memory_bytes !== undefined && s.memory_limit
      ? Math.min(100, (s.memory_bytes / s.memory_limit) * 100)
      : undefined;
  const host = node?.hostname;
  const loading = c.running && stats.isLoading;

  return (
    <div className="space-y-4">
      {/* Summary strip */}
      <div className="grid grid-cols-2 overflow-hidden rounded-lg border border-line bg-panel sm:grid-cols-3 xl:grid-cols-6 [&>*]:border-line max-sm:[&>*:nth-child(odd)]:border-r sm:[&>*]:border-r">
        <Fact k="State">
          <StateChip value={c.state} />
          {c.running ? (
            c.started_at && (
              <span
                className="truncate font-mono text-[11px] text-muted tabular-nums"
                title={`Started ${time(c.started_at)}`}
              >
                up {upFor(c.started_at)}
              </span>
            )
          ) : (
            <span
              className="truncate font-mono text-[11px] text-muted tabular-nums"
              title={
                c.finished_at ? `Stopped ${time(c.finished_at)}` : undefined
              }
            >
              {c.exit_code !== undefined ? `exit ${c.exit_code}` : ""}
              {c.finished_at ? ` · ${ago(c.finished_at)}` : ""}
            </span>
          )}
        </Fact>
        <Fact k="Health" hint="From the image's HEALTHCHECK">
          {c.health ? (
            <StateChip value={c.health} />
          ) : (
            <span
              className="text-xs text-faint"
              title="The image defines no HEALTHCHECK, so Docker reports no health"
            >
              no healthcheck
            </span>
          )}
        </Fact>
        <Fact k="Image">
          {c.image ? (
            <>
              <span
                className="truncate font-mono text-[11px]"
                title={c.image_id ? `${c.image}\n${c.image_id}` : c.image}
              >
                {c.image}
              </span>
              <CopyIcon text={c.image} label="Copy image" />
            </>
          ) : (
            dash
          )}
          {digest && (
            <span
              className="hidden shrink-0 font-mono text-[10px] text-faint 2xl:inline"
              title={c.image_id}
            >
              @{digest}
            </span>
          )}
        </Fact>
        <Fact k="Node">
          <Link
            to="/nodes/$nodeId"
            params={{ nodeId }}
            className="truncate text-accent hover:underline"
            title={node ? nodeName(node) : nodeId}
          >
            {node ? nodeName(node) : nodeId.slice(0, 8)}
          </Link>
        </Fact>
        <Fact k="Owner" hint="The Forge service or stack that created it">
          <OwnerLink owner={owner} compact />
        </Fact>
        <Fact k="Restart policy" hint="Docker --restart">
          <Badge
            tone={
              c.restart_policy === "no" || !c.restart_policy
                ? "muted"
                : "accent"
            }
          >
            {c.restart_policy || "no"}
          </Badge>
        </Fact>
      </div>

      <div className="grid items-start gap-4 lg:grid-cols-2">
        <Section title="Resources">
          <dl className="divide-y divide-line">
            <Kv k="CPU">
              {!c.running ? (
                dash
              ) : s?.cpu_percent !== undefined ? (
                <Meter
                  pct={s.cpu_percent}
                  title="CPU use; 100 % is one full core"
                >
                  {s.cpu_percent.toFixed(1)} %
                </Meter>
              ) : (
                <span className="font-mono text-[11px]">
                  {loading ? "…" : dash}
                </span>
              )}
            </Kv>
            <Kv k="Memory">
              {!c.running || s?.memory_bytes === undefined ? (
                <span className="font-mono text-[11px]">
                  {loading ? "…" : dash}
                </span>
              ) : (
                <Meter
                  pct={memPct}
                  title={
                    memPct !== undefined
                      ? `${memPct.toFixed(0)} % of the limit`
                      : "No memory limit"
                  }
                >
                  {bytes(s.memory_bytes)}
                  {s.memory_limit ? (
                    <span className="text-faint">
                      {" "}
                      / {bytes(s.memory_limit)}
                    </span>
                  ) : null}
                </Meter>
              )}
            </Kv>
          </dl>
          <div className="flex flex-wrap items-center gap-x-3 border-t border-line px-3 py-1.5 text-[11px] text-faint">
            <span>
              PID{" "}
              <span className="font-mono text-muted tabular-nums">
                {c.pid ?? "—"}
              </span>
            </span>
            {c.running && !stats.isLoading && !s && (
              <span>The agent returned no live stats.</span>
            )}
          </div>
        </Section>

        <Section title="Runtime">
          <dl className="divide-y divide-line">
            <Kv k="Entrypoint">
              <Cmd argv={c.entrypoint} />
            </Kv>
            <Kv k="Command">
              <Cmd argv={c.command} />
            </Kv>
            <Kv k="Created">
              <span className="text-xs">{when(c.created_at)}</span>
            </Kv>
            <Kv k={c.running ? "Started" : "Stopped"}>
              <span className="text-xs">
                {c.running ? when(c.started_at) : when(c.finished_at)}
              </span>
            </Kv>
          </dl>
        </Section>

        <Section title="Ports" count={ports.length}>
          {ports.length === 0 ? (
            <CardNote>No ports exposed.</CardNote>
          ) : (
            <ul className="divide-y divide-line">
              {ports.map((g) => {
                const loopback =
                  g.binds.length > 0 &&
                  g.binds.every((b) => /^(127\.|::1|\[::1\])/.test(b));
                const url =
                  g.hostPort && g.proto === "tcp" && host && !loopback
                    ? `http://${host}:${g.hostPort}`
                    : undefined;
                return (
                  <li
                    key={g.key}
                    className="flex min-h-8 min-w-0 flex-wrap items-center gap-x-2 gap-y-1 px-3 py-1 font-mono text-[11px] tabular-nums"
                  >
                    {g.hostPort ? (
                      <span className="inline-flex items-center">
                        <span className="text-[13px] font-semibold">
                          {g.hostPort}
                        </span>
                        <CopyIcon text={g.hostPort} label="Copy host port" />
                      </span>
                    ) : (
                      <span className="text-faint">not published</span>
                    )}
                    <span className="text-faint" aria-label="maps to">
                      →
                    </span>
                    <span>{g.cport || "—"}</span>
                    <Badge>{g.proto}</Badge>
                    {g.binds.map((b) => (
                      <span
                        key={b}
                        className="max-w-40 truncate rounded border border-line bg-panel-2 px-1 text-[10px] text-muted"
                        title={bindTitle(b)}
                      >
                        {b}
                      </span>
                    ))}
                    {url && (
                      <a
                        href={url}
                        target="_blank"
                        rel="noreferrer noopener"
                        className="ml-auto inline-flex items-center gap-1 font-sans text-xs text-accent hover:underline"
                        title={`Open ${url} (only if the node's address is reachable from your browser)`}
                      >
                        <ExternalLink className="size-3.5" /> open
                      </a>
                    )}
                    {loopback && (
                      <span
                        className="ml-auto font-sans text-xs text-faint"
                        title="Bound to loopback; reachable only on the node"
                      >
                        loopback only
                      </span>
                    )}
                  </li>
                );
              })}
            </ul>
          )}
        </Section>

        <Section title="Networks" count={networks.length}>
          {networks.length === 0 ? (
            <CardNote>Not attached to any network.</CardNote>
          ) : (
            <ul className="divide-y divide-line">
              {networks.map((nw) => {
                const ip = ips.get(nw);
                return (
                  <li
                    key={nw}
                    className="flex min-h-8 min-w-0 items-center gap-2 px-3 py-1 text-xs"
                  >
                    <Network className="size-3.5 shrink-0 text-faint" />
                    <span className="truncate font-medium" title={nw}>
                      {nw}
                    </span>
                    {ip && (
                      <span className="ml-auto inline-flex shrink-0 items-center font-mono text-[11px] text-muted tabular-nums">
                        {ip}
                        <CopyIcon text={ip} label="Copy IP" />
                      </span>
                    )}
                  </li>
                );
              })}
            </ul>
          )}
        </Section>

        <Section title="Mounts" count={mounts.length} className="lg:col-span-2">
          {mounts.length === 0 ? (
            <CardNote>
              No mounts. Data written inside the container is lost when it is
              removed.
            </CardNote>
          ) : (
            <ul className="divide-y divide-line">
              {mounts.map((m, i) => {
                const ro = m.rw === false || m.mode === "ro";
                return (
                  <li
                    key={`${m.source}-${m.destination}-${i}`}
                    className="grid min-w-0 grid-cols-[auto_minmax(0,1fr)_auto_minmax(0,1fr)_auto] items-center gap-2 px-3 py-1 font-mono text-[11px]"
                  >
                    <Badge>{m.type || "?"}</Badge>
                    <span className="inline-flex min-w-0 items-center gap-1">
                      <span className="truncate" title={m.source}>
                        {m.source || "—"}
                      </span>
                      {m.source && (
                        <CopyIcon text={m.source} label="Copy source" />
                      )}
                    </span>
                    <span className="text-faint">→</span>
                    <span className="truncate" title={m.destination}>
                      {m.destination || "—"}
                    </span>
                    <Badge tone={ro ? "muted" : "warn"}>
                      {ro ? "ro" : m.rw ? "rw" : m.mode || "rw"}
                    </Badge>
                  </li>
                );
              })}
            </ul>
          )}
        </Section>

        <details
          className="group min-w-0 overflow-hidden rounded-lg border border-line bg-panel lg:col-span-2"
          open={labels.length > 0 && labels.length <= 8}
        >
          <summary className="flex min-h-9 cursor-pointer list-none items-center gap-2 bg-panel-2 px-3 py-1.5 focus-visible:outline-2 focus-visible:outline-accent group-open:border-b group-open:border-line">
            <ChevronRight className="size-3.5 text-faint transition group-open:rotate-90" />
            <h3 className="text-xs font-semibold text-fg">Labels</h3>
            <span className="rounded bg-panel px-1.5 font-mono text-[10px] text-muted tabular-nums">
              {labels.length}
            </span>
          </summary>
          {labels.length === 0 ? (
            <CardNote>No labels.</CardNote>
          ) : (
            <>
              {labels.length > 8 && (
                <div className="border-b border-line px-3 py-2">
                  <SearchInput
                    value={labelFilter}
                    onChange={setLabelFilter}
                    placeholder="Filter labels…"
                  />
                </div>
              )}
              <dl className="divide-y divide-line">
                {shownLabels.map(([k, v]) => {
                  const forge = k.startsWith("forge.");
                  return (
                    <div
                      key={k}
                      className="group/row grid grid-cols-1 gap-x-3 px-3 py-1 hover:bg-panel-2/60 sm:grid-cols-[minmax(10rem,30%)_minmax(0,1fr)]"
                    >
                      <dt
                        className={`truncate font-mono text-[11px] ${forge ? "text-accent" : "text-muted"}`}
                        title={forge ? `${k} (set by Forge)` : k}
                      >
                        {k}
                      </dt>
                      <dd className="flex min-w-0 items-start gap-1">
                        <span className="min-w-0 py-0.5 font-mono text-[11px] break-all select-text">
                          {v || <span className="text-faint">(empty)</span>}
                        </span>
                        {v && (
                          <CopyIcon
                            text={v}
                            label={`Copy ${k}`}
                            className="-my-0.5 opacity-0 group-hover/row:opacity-100 focus-visible:opacity-100"
                          />
                        )}
                      </dd>
                    </div>
                  );
                })}
                {shownLabels.length === 0 && (
                  <CardNote>No label matches “{labelFilter}”.</CardNote>
                )}
              </dl>
            </>
          )}
        </details>
      </div>
    </div>
  );
}

function ShellTab({
  node,
  c,
  sessions,
  running,
  onOpen,
}: {
  node?: Server;
  c?: ContainerInspect;
  sessions: ReturnType<typeof useTerminals>["sessions"];
  running: boolean;
  onOpen: () => void;
}) {
  const t = useTerminals();
  const why = !node
    ? "Loading the node…"
    : node.status !== "online"
      ? `The node's agent is ${node.status}`
      : !c
        ? "Loading the container…"
        : !running
          ? "The container is not running"
          : undefined;
  return (
    <div className="grid gap-4 xl:grid-cols-12">
      <div className="space-y-3 xl:col-span-7">
        <div className="flex flex-wrap items-center gap-2">
          <Button
            variant="primary"
            disabled={!!why}
            title={why}
            onClick={onOpen}
          >
            <SquareTerminal className="size-4" /> Open shell
          </Button>
          <span className="text-xs text-muted">
            Starts <span className="font-mono">bash</span> (or{" "}
            <span className="font-mono">sh</span>) inside{" "}
            {c?.name ? (
              <span className="font-mono">{c.name}</span>
            ) : (
              "the container"
            )}{" "}
            through the node's agent. The session opens in the terminal panel,
            stays connected while you navigate and can pop out into its own
            window.
          </span>
        </div>
        {why && <p className="text-xs text-warn">{why}.</p>}
        <ul className="divide-y divide-line border-y border-line">
          {sessions.map((s) => (
            <li
              key={s.key}
              className="flex items-center gap-2 py-1.5 text-[13px]"
            >
              <span
                className="size-2 shrink-0 rounded-full bg-ok"
                aria-hidden
              />
              <button
                className="min-w-0 flex-1 truncate text-left font-mono text-xs hover:text-accent"
                onClick={() => {
                  t.activate(s.key);
                  t.setDock({ open: true });
                }}
                title="Show in the terminal panel"
              >
                {sessionLabel(s)}
              </button>
              {s.detached ? (
                <Badge tone="accent">separate window</Badge>
              ) : (
                <Badge>panel</Badge>
              )}
              {!s.detached && (
                <IconButton
                  label="Open in a separate window"
                  onClick={() => t.detach(s.key)}
                >
                  <ExternalLink className="size-4" />
                </IconButton>
              )}
              <Button variant="ghost" onClick={() => t.close(s.key)}>
                Close
              </Button>
            </li>
          ))}
          {sessions.length === 0 && (
            <li className="py-4 text-xs text-muted">
              No open shell in this container.
            </li>
          )}
        </ul>
      </div>
      <div className="xl:col-span-5 text-xs text-muted">
        <p className="flex items-start gap-1.5">
          <ShieldCheck className="mt-0.5 size-3.5 shrink-0 text-faint" />
          <span>
            A shell inside a container runs as the container's user, often root.
            It needs <span className="font-mono">container.exec</span> on the
            node, and every shell is audited as{" "}
            <span className="font-mono">container.exec</span>.
          </span>
        </p>
      </div>
    </div>
  );
}

type SealedView = "all" | "hide" | "only";

/**
 * Names from `inspect`; one button reveals every value at once. The tab content
 * unmounts when another tab is chosen (Radix), so revealed values go with it;
 * the reveal query has `gcTime: 0`, so nothing stays in the cache.
 */
function EnvironmentTab({
  nodeId,
  containerId,
  c,
  owner,
}: {
  nodeId: string;
  containerId: string;
  c: ContainerInspect;
  owner: ContainerOwner;
}) {
  const toast = useToast();
  const canReveal = useCanRevealEnv();
  const [view, setView] = useState<{
    reveal: boolean;
    filter: string;
    sealed: SealedView;
  }>({ reveal: false, filter: "", sealed: "all" });
  const revealed = useContainerInspect(nodeId, containerId, {
    reveal: true,
    enabled: view.reveal,
  });
  const failure = view.reveal
    ? revealed.data?.status === "failed"
      ? (revealed.data.error ?? "Revealing the values failed")
      : revealed.isError
        ? revealed.error
        : undefined
    : undefined;
  // Report a failed reveal once, then drop back to masked.
  useEffect(() => {
    if (failure === undefined) return;
    const e = failure;
    toast(
      e instanceof ApiError && e.status === 403
        ? "You are not allowed to reveal environment values (container.inspect_env)."
        : e instanceof ApiError && e.status === 409
          ? "The node's agent is too old to reveal values; update the agent."
          : errorText(e),
      "error",
    );
    setView((v) => ({ ...v, reveal: false }));
  }, [failure, toast]);

  const sealed = new Set(
    (owner?.kind === "app" ? (owner.app.env ?? []) : [])
      .filter((e) => e?.secret)
      .map((e) => e.key),
  );
  const values = new Map(
    (revealed.data?.result?.env ?? []).map((e) => [e.key, e.value]),
  );
  const have = view.reveal && revealed.data?.status === "succeeded";
  const loading = view.reveal && revealed.isFetching;
  const keys = c.env_keys ?? [];
  const f = view.filter.trim().toLowerCase();
  const rows = keys.filter(
    (k) =>
      (!f || k.toLowerCase().includes(f)) &&
      (view.sealed === "all" || (view.sealed === "only") === sealed.has(k)),
  );
  const deny =
    canReveal === false
      ? "Needs container.inspect_env (platform admin, workspace admin or operator)"
      : undefined;

  return (
    <Section
      title="Environment variables"
      count={keys.length}
      actions={
        <>
          {keys.length > 0 && (
            <SearchInput
              value={view.filter}
              onChange={(filter) => setView((v) => ({ ...v, filter }))}
              placeholder="Filter by name…"
            />
          )}
          {sealed.size > 0 && (
            <div
              className="inline-flex overflow-hidden rounded-md border border-line bg-panel text-xs"
              role="group"
              aria-label="Sealed secrets"
            >
              {(["all", "hide", "only"] as const).map((s) => (
                <button
                  key={s}
                  type="button"
                  aria-pressed={view.sealed === s}
                  onClick={() => setView((v) => ({ ...v, sealed: s }))}
                  className={`px-2 py-1 focus-visible:outline-2 focus-visible:outline-accent ${view.sealed === s ? "bg-accent-soft text-fg" : "text-muted hover:text-fg"}`}
                >
                  {s === "all"
                    ? "All"
                    : s === "hide"
                      ? "Hide sealed"
                      : "Only sealed"}
                </button>
              ))}
            </div>
          )}
          {have ? (
            <Button onClick={() => setView((v) => ({ ...v, reveal: false }))}>
              <EyeOff className="size-4" /> Hide values
            </Button>
          ) : (
            <Button
              variant="primary"
              disabled={
                keys.length === 0 ||
                loading ||
                !!deny ||
                canReveal === undefined
              }
              title={
                keys.length === 0
                  ? "No environment variables"
                  : (deny ??
                    "Fetches every value from the node; needs container.inspect_env and an MFA-verified session")
              }
              onClick={() => setView((v) => ({ ...v, reveal: true }))}
            >
              {loading ? (
                <Loader2 className="size-4 animate-spin" />
              ) : (
                <Eye className="size-4" />
              )}{" "}
              Reveal values
            </Button>
          )}
        </>
      }
    >
      <p className="flex items-start gap-1.5 border-b border-line px-3 py-1.5 text-[11px] text-muted">
        <ShieldCheck className="mt-px size-3.5 shrink-0 text-faint" />
        <span>
          <span title="Names come from docker inspect. A reveal is audited as container.env.reveal.">
            Values load only on request after an MFA check, aren't cached, and
            are cleared when you leave. Reveals are audited.
          </span>
          {sealed.size > 0 && (
            <>
              {" "}
              {sealed.size} secret{sealed.size === 1 ? "" : "s"} sealed by Forge
              stay <span className="font-mono">&lt;sealed&gt;</span>.
            </>
          )}
          {deny && <span className="text-warn"> {deny}.</span>}
        </span>
      </p>
      {keys.length === 0 ? (
        <Empty
          icon={<KeyRound className="size-5" />}
          title="No environment variables"
        >
          The container's config has no <span className="font-mono">Env</span>{" "}
          entries.
        </Empty>
      ) : rows.length === 0 ? (
        <CardNote>No variable matches the filter.</CardNote>
      ) : (
        <dl className="divide-y divide-line">
          {f || view.sealed !== "all" ? (
            <div className="px-3 py-1 text-[11px] text-faint tabular-nums">
              {rows.length} of {keys.length} shown
            </div>
          ) : null}
          {rows.map((k) => {
            const isSealed = sealed.has(k);
            const v = values.get(k);
            return (
              <div
                key={k}
                className="group/row grid grid-cols-1 gap-x-3 px-3 py-1.5 hover:bg-panel-2/60 sm:grid-cols-[minmax(8rem,30%)_minmax(0,1fr)]"
              >
                <dt className="flex min-w-0 items-center">
                  <span
                    className="truncate font-mono text-[11px] text-fg select-text"
                    title={k}
                  >
                    {k}
                  </span>
                  <CopyIcon
                    text={k}
                    label={`Copy name ${k}`}
                    className="-my-1 opacity-0 group-hover/row:opacity-100 focus-visible:opacity-100"
                  />
                </dt>
                <dd className="flex min-w-0 items-start gap-1">
                  {isSealed ? (
                    <span
                      className="rounded bg-panel-2 px-1.5 font-mono text-[11px] text-faint"
                      title="A secret sealed by Forge for this service; it is never shown here"
                    >
                      &lt;sealed&gt;
                    </span>
                  ) : !have ? (
                    <span
                      className="rounded bg-panel-2 px-1.5 font-mono text-[11px] tracking-widest text-faint"
                      aria-label="hidden value"
                    >
                      ••••••
                    </span>
                  ) : v === undefined ? (
                    <span
                      className="font-mono text-[11px] text-faint"
                      title="The node did not return this value"
                    >
                      —
                    </span>
                  ) : (
                    <>
                      <span className="min-w-0 font-mono text-[11px] break-all select-text">
                        {v || <span className="text-faint">(empty)</span>}
                      </span>
                      <CopyIcon
                        text={v}
                        label={`Copy value of ${k}`}
                        className="-my-1 opacity-0 group-hover/row:opacity-100 focus-visible:opacity-100"
                      />
                    </>
                  )}
                </dd>
              </div>
            );
          })}
        </dl>
      )}
    </Section>
  );
}
