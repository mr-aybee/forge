/**
 * Agents panel (GitHub issue #4): each node's agent version against the
 * control plane's, Update per node, a canary rollout in batches
 * (`agent.rollout`), and automatic updates inside a weekly UTC window.
 * Updates run as `agent.update` jobs (MFA-guarded); a node whose new agent
 * does not come back restores the previous one by itself. Step-up (403
 * `step_up_required`) is handled by the API client, which retries once.
 */
import { Link } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { CheckCircle2, CircleArrowUp, Clock, Loader2, RefreshCw, TriangleAlert } from "lucide-react";
import { useMemo, useState, type ReactNode } from "react";

import { ApiError, api, get, post } from "../api/client";
import { ISSUE_TEXT, agentUpdateKeys, useAgentVersions, type AgentVersionRow, type UpdateIssue } from "../api/agentUpdates";
import { useJob, useJobs, useMe } from "../api/hooks";
import type { Job } from "../api/types";
import { errorText, useToast } from "../app/toast";
import { JobDetail } from "./JobsDrawer";
import { isActive } from "./jobMeta";
import { Pager, usePaged } from "./Pager";
import { Badge, Button, Field, Input, Modal, Panel, SearchInput, Status, Switch, Table, Td, UnsavedBar, ago, short, time } from "./ui";

// ------------------------------------------------------------------ data

type Rec = Record<string, unknown>;
const rec = (v: unknown): Rec => (v && typeof v === "object" && !Array.isArray(v) ? (v as Rec) : {});
const str = (v: unknown): string => (typeof v === "string" ? v : "");
const optStr = (v: unknown): string | null => (typeof v === "string" && v ? v : null);
const num = (v: unknown): number | null => (typeof v === "number" && Number.isFinite(v) ? v : null);

export interface AutoUpdate {
  enabled: boolean;
  days: string[];
  start: string;
  end: string;
  batch: number;
  timezone: string;
  enabled_by: string | null;
  last_target: string | null;
  last_job: string | null;
  last_error: string | null;
}

const DAYS = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"] as const;
const DAY_LABEL: Record<string, string> = { mon: "Mon", tue: "Tue", wed: "Wed", thu: "Thu", fri: "Fri", sat: "Sat", sun: "Sun" };

function normAuto(raw: unknown): AutoUpdate {
  const r = rec(raw);
  return {
    enabled: r.enabled === true,
    days: (Array.isArray(r.days) ? r.days : []).filter((d): d is string => typeof d === "string" && (DAYS as readonly string[]).includes(d)),
    start: str(r.start) || "02:00",
    end: str(r.end) || "05:00",
    batch: num(r.batch) ?? 1,
    timezone: str(r.timezone) || "UTC",
    enabled_by: optStr(r.enabled_by),
    last_target: optStr(r.last_target),
    last_job: optStr(r.last_job),
    last_error: optStr(r.last_error),
  };
}

/** Automatic-update settings; `null` data when the server has no such endpoint (older server). */
function useAutoUpdate() {
  return useQuery({
    queryKey: agentUpdateKeys.autoUpdate,
    queryFn: () =>
      get<unknown>("/api/v1/agents/auto-update")
        .then(normAuto)
        .catch((e: unknown) => {
          if (e instanceof ApiError && (e.status === 404 || e.status === 405)) return null;
          throw e;
        }),
    staleTime: 15_000,
    refetchInterval: 60_000,
  });
}

interface RolloutStarted {
  job?: Job;
  target?: string;
  nodes?: number;
  batches?: number;
}

const clampBatch = (n: number) => (Number.isFinite(n) ? Math.min(100, Math.max(1, Math.round(n))) : 1);
const nodeLabel = (n: Pick<AgentVersionRow, "display_name" | "hostname">) => n.display_name || n.hostname;
/** A rollout over "every outdated node" includes these. */
const eligible = (n: AgentVersionRow) => n.verdict === "will_update" || n.verdict === "unknown";
const issueList = (list: UpdateIssue[]) => list.map((i) => ISSUE_TEXT[i]).join("; ");
const REINSTALL_GUIDE = "https://forge.orbyntech.com/docs/guides/add-nodes/";

/** An error for display: the server's detail, and whether it is a "nothing to do" conflict. */
function Problem({ error, warn }: { error: unknown; warn?: boolean }) {
  if (!error) return null;
  const soft = warn || (error instanceof ApiError && (error.status === 409 || error.code === "step_up_required"));
  return (
    <p role="alert" className={`flex items-start gap-1.5 rounded-md px-2.5 py-1.5 text-xs ${soft ? "bg-warn-soft text-warn" : "bg-bad-soft text-bad"}`}>
      <TriangleAlert className="mt-px size-3.5 shrink-0" />
      <span className="min-w-0 break-words">{error instanceof ApiError && error.code === "step_up_required" ? `Not verified: ${error.message}` : errorText(error)}</span>
    </p>
  );
}

// ------------------------------------------------------------------ panel

export function AgentUpdates() {
  const qc = useQueryClient();
  const toast = useToast();
  const q = useAgentVersions();
  const jobs = useJobs().data;
  const [rolloutOpen, setRolloutOpen] = useState(false);
  const [startedJob, setStartedJob] = useState<string>();
  const [detail, setDetail] = useState<string>();
  const [search, setSearch] = useState("");

  const update = useMutation({
    mutationFn: (ids: string[]) =>
      post<{ jobs?: { server_id: string; job_id: string }[]; refused?: { server_id: string; error: string }[] }>("/api/v1/agents/update", { server_ids: ids }),
    onSuccess: (r) => {
      const ok = Array.isArray(r?.jobs) ? r.jobs.length : 0;
      const refused = Array.isArray(r?.refused) ? r.refused : [];
      toast(`${ok} agent update${ok === 1 ? "" : "s"} started${refused.length ? ` · ${refused.length} refused: ${refused[0]?.error ?? ""}` : ""}`, refused.length ? "error" : undefined);
      void qc.invalidateQueries({ queryKey: ["jobs"] });
      void qc.invalidateQueries({ queryKey: agentUpdateKeys.versions });
    },
    onError: (e) => toast(errorText(e), "error"),
  });

  const v = q.data;
  const nodes = v?.nodes ?? [];
  const names = useMemo(() => new Map(nodes.map((n) => [n.id, nodeLabel(n)])), [nodes]);
  const ready = nodes.filter(eligible);
  const outdated = nodes.filter((n) => n.outdated);
  // Search narrows the table only; "Update agents" always covers every ready node.
  const needle = search.trim().toLowerCase();
  const shown = needle
    ? nodes.filter((n) => `${nodeLabel(n)} ${n.hostname} ${n.status} ${n.os} ${n.arch} ${n.os}/${n.arch} ${n.agent_version} ${n.outdated ? "outdated" : "current"} ${n.verdict.replace("_", " ")}`.toLowerCase().includes(needle))
    : nodes;
  const paged = usePaged(shown, { reset: needle });
  // The rollout to follow: one started here, else any rollout still running.
  const running = (Array.isArray(jobs) ? jobs : []).find((j) => j.kind === "agent.rollout" && isActive(j));
  const followId = startedJob ?? running?.id;

  return (
    <div className="mb-4 space-y-4">
      <Panel
        title="Agents"
        description={v ? `Agents on offer ${v.target || "—"}${v.server && v.server !== v.target ? ` (server ${v.server})` : ""}. Updates are verified and roll back on their own if a node does not reconnect.` : q.isError ? "Agent versions are unavailable." : "Loading…"}
        actions={
          <div className="flex flex-wrap gap-2">
            {nodes.length > 0 && <SearchInput value={search} onChange={setSearch} placeholder="Search node, platform, version, status" />}
            <Button onClick={() => void q.refetch()} disabled={q.isFetching} title="Refresh" aria-label="Refresh">
              <RefreshCw className={`size-4 ${q.isFetching ? "animate-spin" : ""}`} />
            </Button>
            <Button
              variant="primary"
              disabled={!outdated.length}
              title={ready.length ? "Roll the new agent out in batches, canary first" : outdated.length ? "No outdated node can update itself now; open to see why" : "Every agent is current"}
              onClick={() => setRolloutOpen(true)}
            >
              <CircleArrowUp className="size-4" /> Update agents ({ready.length})
            </Button>
          </div>
        }
      >
        {followId && <RolloutProgress jobId={followId} names={names} onDetails={setDetail} onDismiss={startedJob ? () => setStartedJob(undefined) : undefined} />}
        {q.isError && <div className="p-3"><Problem error={q.error} /></div>}
        <Table head={["Node", "Status", "Platform", "Agent", ""]}>
          {paged.rows.map((n) => (
            <tr key={n.id}>
              <Td className="max-w-56 font-medium">
                <Link to="/nodes/$nodeId" params={{ nodeId: n.id }} className="block truncate hover:text-accent" title={nodeLabel(n)}>
                  {nodeLabel(n)}
                </Link>
              </Td>
              <Td>
                <Status value={n.status} />
              </Td>
              <Td className="text-xs text-muted">{n.os || n.arch ? `${n.os || "—"}/${n.arch || "—"}` : "—"}</Td>
              <Td className="font-mono text-[11px] tabular-nums">
                {n.outdated ? (
                  <span className="inline-flex flex-wrap items-center gap-1.5">
                    {n.agent_version || "unknown"} <Badge tone="warn">outdated</Badge>
                    {n.verdict === "needs_reinstall" && (
                      <span title={issueList(n.reasons)}>
                        <Badge tone="bad">needs reinstall</Badge>
                      </span>
                    )}
                    {n.warnings.includes("ephemeral_container") && (
                      <span title={ISSUE_TEXT.ephemeral_container}>
                        <Badge>container</Badge>
                      </span>
                    )}
                  </span>
                ) : n.ahead ? (
                  <span className="inline-flex items-center gap-1.5" title={`Newer than ${v?.target ?? "the version on offer"}; updates leave it alone`}>
                    {n.agent_version} <Badge tone="accent">ahead</Badge>
                  </span>
                ) : (
                  <span className="inline-flex items-center gap-1 text-ok">
                    <CheckCircle2 className="size-3.5" /> {n.agent_version || "—"}
                  </span>
                )}
              </Td>
              <Td className="text-right">
                {n.outdated && (
                  <Button
                    disabled={!eligible(n) || update.isPending}
                    title={
                      n.verdict === "needs_reinstall"
                        ? issueList(n.reasons)
                        : n.status !== "online"
                          ? "The node is offline"
                          : `Update to ${v?.target ?? "the version on offer"}`
                    }
                    onClick={() => update.mutate([n.id])}
                  >
                    {update.isPending && update.variables?.[0] === n.id && <Loader2 className="size-4 animate-spin" />}
                    Update
                  </Button>
                )}
              </Td>
            </tr>
          ))}
          {!nodes.length && (
            <tr>
              <Td colSpan={5} className="py-4 text-center text-muted">
                {q.isLoading ? "Loading…" : q.isError ? "—" : "No nodes. Add a node to see its agent version here."}
              </Td>
            </tr>
          )}
          {nodes.length > 0 && !shown.length && (
            <tr>
              <Td colSpan={5} className="py-4 text-center text-muted">
                No node matches “{search}”.
              </Td>
            </tr>
          )}
        </Table>
        <Pager paged={paged} />
      </Panel>

      <AutoUpdatePanel onJob={setDetail} />

      {rolloutOpen && (
      <RolloutDialog
        open={rolloutOpen}
        onOpenChange={setRolloutOpen}
        target={v?.target ?? ""}
        nodes={outdated}
        onStarted={(id) => setStartedJob(id)}
        names={names}
        onDetails={setDetail}
      />
      )}
      <Modal open={!!detail} onOpenChange={(o) => !o && setDetail(undefined)} title="Job" wide>
        {detail && <JobDetail id={detail} onNavigate={() => setDetail(undefined)} />}
      </Modal>
    </div>
  );
}

// ------------------------------------------------------------------ rollout

function RolloutDialog({
  open,
  onOpenChange,
  target,
  nodes,
  onStarted,
  names,
  onDetails,
}: {
  open: boolean;
  onOpenChange: (o: boolean) => void;
  target: string;
  nodes: AgentVersionRow[];
  onStarted: (jobId: string) => void;
  names: Map<string, string>;
  onDetails: (jobId: string) => void;
}) {
  const qc = useQueryClient();
  const toast = useToast();
  // Unticked ids; every other node that will update is selected (so new
  // outdated nodes start ticked).
  const [off, setOff] = useState<Set<string>>(new Set());
  const willUpdate = nodes.filter(eligible);
  const reinstall = nodes.filter((n) => n.verdict === "needs_reinstall");
  const offline = nodes.filter((n) => n.verdict === "offline");
  const [batchText, setBatchText] = useState("1");
  const [started, setStarted] = useState<string>();
  const batch = clampBatch(Number(batchText));
  const picked = willUpdate.filter((n) => !off.has(n.id));
  const batches = picked.length ? Math.ceil(picked.length / batch) : 0;

  const start = useMutation({
    mutationFn: () => post<RolloutStarted>("/api/v1/agents/rollouts", { server_ids: picked.map((n) => n.id), batch }),
    onSuccess: (r) => {
      const id = r?.job?.id;
      const n = num(r?.nodes) ?? picked.length;
      const b = num(r?.batches) ?? batches;
      toast(`Rollout started: ${n} node${n === 1 ? "" : "s"} in ${b} batch${b === 1 ? "" : "es"}`);
      void qc.invalidateQueries({ queryKey: ["jobs"] });
      if (id) {
        setStarted(id);
        onStarted(id);
      }
    },
  });

  const close = (o: boolean) => {
    onOpenChange(o);
    if (!o) {
      setStarted(undefined);
      start.reset();
      setOff(new Set());
    }
  };
  const progressId = started;

  return (
    <Modal open={open} onOpenChange={close} title="Update agents" description={target ? `Update outdated agents to ${target}.` : undefined} wide>
      {progressId ? (
        <div className="space-y-3">
          <div className="-mx-5 border-t border-line">
            <RolloutProgress jobId={progressId} names={names} onDetails={(id) => { close(false); onDetails(id); }} />
          </div>
          <p className="text-xs text-muted">You can close this dialog; the rollout keeps running and its progress stays on the Forge updates page.</p>
          <div className="flex justify-end">
            <Button onClick={() => close(false)}>Close</Button>
          </div>
        </div>
      ) : (
        <div className="space-y-3">
          <div className="rounded-md border border-line">
            <div className="flex items-center justify-between gap-2 border-b border-line bg-panel-2 px-3 py-1.5 text-xs text-muted">
              <label className="inline-flex items-center gap-2">
                <input
                  type="checkbox"
                  className="accent-accent"
                  checked={willUpdate.length > 0 && !willUpdate.some((n) => off.has(n.id))}
                  ref={(el) => {
                    if (el) {
                      const unticked = willUpdate.filter((n) => off.has(n.id)).length;
                      el.indeterminate = unticked > 0 && unticked < willUpdate.length;
                    }
                  }}
                  onChange={(e) => setOff(e.target.checked ? new Set() : new Set(willUpdate.map((n) => n.id)))}
                  aria-label="Select all nodes that will update"
                />
                Will update
              </label>
              <span className="font-mono text-[11px] tabular-nums">
                {picked.length} of {willUpdate.length}
              </span>
            </div>
            <ul className="max-h-56 divide-y divide-line overflow-y-auto">
              {willUpdate.map((n) => (
                <li key={n.id}>
                  <label className="flex min-w-0 cursor-pointer items-center gap-2 px-3 py-1.5 text-[13px] hover:bg-panel-2">
                    <input
                      type="checkbox"
                      className="accent-accent"
                      checked={!off.has(n.id)}
                      onChange={(e) =>
                        setOff((s) => {
                          const next = new Set(s);
                          if (e.target.checked) next.delete(n.id);
                          else next.add(n.id);
                          return next;
                        })
                      }
                    />
                    <span className="min-w-0 flex-1 truncate" title={nodeLabel(n)}>
                      {nodeLabel(n)}
                    </span>
                    {n.verdict === "unknown" && (
                      <span title="An older agent that does not report whether it can update itself; the node decides">
                        <Badge>not checked</Badge>
                      </span>
                    )}
                    {n.warnings.includes("ephemeral_container") && (
                      <span title={ISSUE_TEXT.ephemeral_container}>
                        <Badge tone="warn">until recreated</Badge>
                      </span>
                    )}
                    <span className="shrink-0 font-mono text-[11px] text-muted tabular-nums">
                      {n.agent_version || "unknown"} → {target || "—"}
                    </span>
                  </label>
                </li>
              ))}
              {!willUpdate.length && <li className="px-3 py-3 text-center text-xs text-muted">No outdated node can update itself now.</li>}
            </ul>
          </div>

          {willUpdate.some((n) => n.warnings.includes("ephemeral_container")) && (
            <p className="rounded-md bg-warn-soft px-2.5 py-1.5 text-xs text-warn">
              Some nodes run in containers whose agent comes from the image: the update lasts until the container is recreated. Update the image too.
            </p>
          )}

          {reinstall.length > 0 && (
            <NodeGroup title="Needs reinstall" count={reinstall.length}>
              {reinstall.map((n) => (
                <li key={n.id} className="flex min-w-0 items-start gap-2 px-3 py-1.5 text-[13px]">
                  <span className="min-w-0 flex-1">
                    <span className="block truncate" title={nodeLabel(n)}>
                      {nodeLabel(n)}
                    </span>
                    <span className="block text-xs text-muted">{issueList(n.reasons)}</span>
                  </span>
                  <span className="shrink-0 font-mono text-[11px] text-muted tabular-nums">{n.agent_version || "unknown"}</span>
                </li>
              ))}
              <li className="px-3 py-1.5 text-xs text-muted">
                Run the node installer again on these nodes:{" "}
                <a className="text-accent hover:underline" href={REINSTALL_GUIDE} target="_blank" rel="noreferrer">
                  Add nodes
                </a>
                .
              </li>
            </NodeGroup>
          )}

          {offline.length > 0 && (
            <NodeGroup title="Offline (skipped)" count={offline.length}>
              {offline.map((n) => (
                <li key={n.id} className="flex min-w-0 items-center gap-2 px-3 py-1.5 text-[13px]">
                  <span className="min-w-0 flex-1 truncate" title={nodeLabel(n)}>
                    {nodeLabel(n)}
                  </span>
                  <span className="shrink-0 font-mono text-[11px] text-muted tabular-nums">{n.agent_version || "unknown"}</span>
                </li>
              ))}
              <li className="px-3 py-1.5 text-xs text-muted">Start another rollout once they are online.</li>
            </NodeGroup>
          )}

          <div className="grid gap-3 sm:grid-cols-[8rem_1fr]">
            <Field label="Batch size">
              <Input type="number" min={1} max={100} value={batchText} onChange={(e) => setBatchText(e.target.value)} onBlur={() => setBatchText(String(batch))} />
            </Field>
            <p className="text-xs text-faint sm:pt-6">
              The first batch is the canary. The next batch starts only when every node in this one is back on the new version or was skipped; the first failure stops the rollout.
            </p>
          </div>

          <div className="text-xs text-muted">
            <div className="mb-1 font-medium text-fg">What happens</div>
            <ol className="list-decimal space-y-0.5 pl-4">
              <li>
                {picked.length} node{picked.length === 1 ? "" : "s"} in <span className="font-mono tabular-nums">{batches}</span> batch{batches === 1 ? "" : "es"} of up to{" "}
                <span className="font-mono tabular-nums">{batch}</span>; each node downloads and verifies the signed agent.
              </li>
              <li>Each agent restarts; jobs and terminals on that node pause for a few seconds.</li>
              <li>A node that does not reconnect on the new version rolls back by itself, and the rollout stops there.</li>
              <li>A node that goes offline or turns out unable to update itself is skipped and listed; the rollout carries on.</li>
            </ol>
          </div>

          <Problem error={start.error} />

          <div className="flex justify-end gap-2">
            <Button onClick={() => close(false)}>Cancel</Button>
            <Button variant="primary" disabled={!picked.length || start.isPending} onClick={() => start.mutate()} title={picked.length ? undefined : "Tick at least one node"}>
              {start.isPending && <Loader2 className="size-4 animate-spin" />}
              Start rollout
            </Button>
          </div>
        </div>
      )}
    </Modal>
  );
}

/** A titled, read-only list of nodes in the rollout dialog. */
function NodeGroup({ title, count, children }: { title: string; count: number; children: ReactNode }) {
  return (
    <div className="rounded-md border border-line">
      <div className="flex items-center justify-between gap-2 border-b border-line bg-panel-2 px-3 py-1.5 text-xs text-muted">
        <span>{title}</span>
        <span className="font-mono text-[11px] tabular-nums">{count}</span>
      </div>
      <ul className="max-h-40 divide-y divide-line overflow-y-auto">{children}</ul>
    </div>
  );
}

/** One line per rollout: status, batch progress, per-node results and the error that stopped it. */
export function RolloutProgress({ jobId, names, onDetails, onDismiss }: { jobId: string; names: Map<string, string>; onDetails: (id: string) => void; onDismiss?: () => void }) {
  const job = useJob(jobId);
  const j = job.data;
  const st = rec(j?.state);
  const total = num(st.batches);
  const done = j?.status === "succeeded" && total !== null ? total : (num(st.done_batches) ?? 0);
  const pct = total ? Math.min(100, Math.round((done / total) * 100)) : 0;
  const results = (Array.isArray(st.nodes) ? st.nodes : []).map(rec).filter((n) => typeof n.server_id === "string");
  const target = optStr(st.target) ?? optStr(rec(j?.payload).target);
  const auto = optStr(rec(j?.payload).origin) === "auto";

  return (
    <div className="space-y-1.5 border-b border-line px-4 py-2">
      <div className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1 text-[13px]">
        <span className="font-medium">Rollout{target ? <span className="font-mono text-[11px]"> {target}</span> : null}</span>
        {auto && <Badge>automatic</Badge>}
        {j ? <Status value={j.status} /> : job.isError ? <span className="text-xs text-faint">—</span> : <Loader2 className="size-3.5 animate-spin text-muted" />}
        <span className="font-mono text-[11px] text-muted tabular-nums">
          batch {total !== null ? `${Math.min(done + (isActive(j) ? 1 : 0), total)} of ${total}` : "—"}
        </span>
        <div className="h-1.5 w-32 overflow-hidden rounded-full bg-panel-2" role="progressbar" aria-valuemin={0} aria-valuemax={total ?? 0} aria-valuenow={done} aria-label="Batches done">
          <div className={`h-full rounded-full ${j?.status === "failed" ? "bg-bad" : j?.status === "succeeded" ? "bg-ok" : "bg-accent"}`} style={{ width: `${pct}%` }} />
        </div>
        {j && (
          <span className="text-[11px] text-faint" title={time(j.updated_at)}>
            {ago(j.updated_at)}
          </span>
        )}
        <span className="ml-auto flex gap-1">
          <Button variant="ghost" className="px-2 py-0.5 text-xs" onClick={() => onDetails(jobId)}>
            Details
          </Button>
          {onDismiss && !isActive(j) && (
            <Button variant="ghost" className="px-2 py-0.5 text-xs" onClick={onDismiss}>
              Dismiss
            </Button>
          )}
        </span>
      </div>
      {job.isError && !j && <Problem error={job.error} />}
      {j?.last_error && <Problem error={new Error(j.last_error)} />}
      {results.length > 0 && (
        <ul className="flex flex-wrap gap-1">
          {results.map((n, i) => {
            const id = str(n.server_id);
            const label = names.get(id) ?? short(id);
            const childJob = optStr(n.job_id);
            // A skipped node's job succeeded, but nothing was installed.
            const shown = str(n.outcome) === "skipped" ? "skipped" : str(n.status) || "unknown";
            const why = optStr(n.error) ?? optStr(n.reason);
            return (
              <li key={`${id}-${i}`} className="inline-flex max-w-full min-w-0 items-center gap-1 rounded border border-line px-1.5 py-0.5 text-xs" title={why ?? undefined}>
                <Link to="/nodes/$nodeId" params={{ nodeId: id }} className="min-w-0 truncate hover:text-accent">
                  {label}
                </Link>
                {childJob ? (
                  <button type="button" className="rounded focus-visible:outline" onClick={() => onDetails(childJob)} aria-label={`Job for ${label}`}>
                    <Status value={shown} />
                  </button>
                ) : (
                  <Status value={shown} />
                )}
              </li>
            );
          })}
        </ul>
      )}
    </div>
  );
}

// ------------------------------------------------------------------ auto-update

// Automatic updates are fleet-wide: `system.update` on the platform (platform admins).
const MANAGERS = ["platform_admin"];

function AutoUpdatePanel({ onJob }: { onJob: (id: string) => void }) {
  const qc = useQueryClient();
  const toast = useToast();
  const q = useAutoUpdate();
  const bindings = useMe().data?.bindings;
  const canEdit = (Array.isArray(bindings) ? bindings : []).some((b) => MANAGERS.includes(b?.role));
  const [draft, setDraft] = useState<AutoUpdate>();
  const [batchText, setBatchText] = useState<string>();
  const server = q.data ?? undefined;
  const form = draft ?? server;

  const save = useMutation({
    mutationFn: (a: AutoUpdate) =>
      api<unknown>("PUT", "/api/v1/agents/auto-update", { enabled: a.enabled, days: a.days, start: a.start, end: a.end, batch: a.batch }).then(normAuto),
    onSuccess: (a) => {
      qc.setQueryData(agentUpdateKeys.autoUpdate, a);
      setDraft(undefined);
      setBatchText(undefined);
      toast(a.enabled ? "Automatic agent updates are on" : "Automatic agent updates are off");
    },
  });

  if (q.data === null) return null; // Older server: no automatic updates.

  const set = (p: Partial<AutoUpdate>) => form && setDraft({ ...form, ...p });
  const toggleDay = (d: string) => form && set({ days: form.days.includes(d) ? form.days.filter((x) => x !== d) : DAYS.filter((x) => x === d || form.days.includes(x)) });
  const dirty = !!draft && !!server && JSON.stringify({ ...draft, batch: clampBatch(draft.batch) }) !== JSON.stringify(server);
  const timeOk = (t: string) => /^([01]\d|2[0-3]):[0-5]\d$/.test(t);
  const invalid = form ? (!timeOk(form.start) || !timeOk(form.end) ? "Start and end: HH:MM" : form.start === form.end ? "Start and end must differ" : null) : null;
  const ro = !canEdit || save.isPending;

  return (
    <Panel
      title="Automatic updates"
      description="Runs once per new version, inside this window, as the person who turned it on; it needs an MFA-verified session to change."
      actions={
        server?.enabled ? <Badge tone="ok">on</Badge> : server ? <Badge>off</Badge> : undefined
      }
    >
      {q.isLoading ? (
        <div className="space-y-2 p-4" aria-busy>
          <div className="h-6 animate-pulse rounded bg-panel-2" />
          <div className="h-6 animate-pulse rounded bg-panel-2" />
        </div>
      ) : q.isError || !form ? (
        <div className="p-3">
          <Problem error={q.error ?? new Error("Automatic update settings are unavailable.")} />
        </div>
      ) : (
        <div className="space-y-3 px-4 py-3">
          <Switch checked={form.enabled} onChange={(v) => set({ enabled: v })} label="Update agents automatically" disabled={ro} title={canEdit ? undefined : "Only platform admins change this"} />

          <div className="flex flex-wrap items-end gap-x-4 gap-y-2">
            <div>
              <div className="mb-1 text-xs font-medium text-muted">Days</div>
              <div className="flex flex-wrap gap-1" role="group" aria-label="Days">
                {DAYS.map((d) => {
                  const on = form.days.includes(d);
                  return (
                    <button
                      key={d}
                      type="button"
                      aria-pressed={on}
                      disabled={ro}
                      onClick={() => toggleDay(d)}
                      className={`rounded border px-2 py-0.5 text-xs transition disabled:cursor-not-allowed disabled:opacity-60 ${on ? "border-accent bg-accent-soft text-accent" : "border-line text-muted hover:border-accent/60"}`}
                    >
                      {DAY_LABEL[d]}
                    </button>
                  );
                })}
              </div>
              <div className="mt-0.5 text-[11px] text-faint">{form.days.length ? `${form.days.length} day${form.days.length === 1 ? "" : "s"} a week` : "None picked: every day"}</div>
            </div>
            <label className="block">
              <span className="mb-1 block text-xs font-medium text-muted">Start <span className="text-faint">UTC</span></span>
              <Input type="time" className="w-28 font-mono text-xs tabular-nums" value={form.start} disabled={ro} onChange={(e) => set({ start: e.target.value })} />
            </label>
            <label className="block">
              <span className="mb-1 block text-xs font-medium text-muted">End <span className="text-faint">UTC</span></span>
              <Input type="time" className="w-28 font-mono text-xs tabular-nums" value={form.end} disabled={ro} onChange={(e) => set({ end: e.target.value })} />
            </label>
            <label className="block">
              <span className="mb-1 block text-xs font-medium text-muted">Batch size</span>
              <Input
                type="number"
                min={1}
                max={100}
                className="w-20 font-mono text-xs tabular-nums"
                value={batchText ?? String(form.batch)}
                disabled={ro}
                onChange={(e) => {
                  setBatchText(e.target.value);
                  set({ batch: clampBatch(Number(e.target.value)) });
                }}
                onBlur={() => setBatchText(undefined)}
              />
            </label>
          </div>
          {form.start > form.end && timeOk(form.start) && timeOk(form.end) && <p className="text-[11px] text-faint">The window runs past midnight and belongs to the day it starts.</p>}
          {canEdit && (
            <UnsavedBar
              dirty={dirty}
              summary={invalid ?? undefined}
              saveDisabled={!!invalid}
              saving={save.isPending}
              onSave={() => draft && save.mutate({ ...draft, batch: clampBatch(draft.batch) })}
              onDiscard={() => {
                setDraft(undefined);
                setBatchText(undefined);
                save.reset();
              }}
            />
          )}
          {!canEdit && <p className="text-[11px] text-faint">Only platform admins change automatic updates.</p>}
          <Problem error={save.error} />

          <div className="flex min-w-0 flex-wrap items-center gap-x-4 gap-y-1 border-t border-line pt-2 text-xs text-muted">
            <span className="min-w-0 truncate">
              Turned on by{" "}
              <span className="font-mono text-[11px] text-fg" title={server?.enabled_by ?? undefined}>
                {server?.enabled_by ?? "—"}
              </span>
            </span>
            <span className="inline-flex min-w-0 items-center gap-1.5">
              <Clock className="size-3.5 shrink-0" /> Last run
              <span className="font-mono text-[11px] text-fg tabular-nums">{server?.last_target ?? "—"}</span>
              {server?.last_job && (
                <button type="button" className="font-mono text-[11px] text-accent hover:underline" onClick={() => onJob(server.last_job!)} title="Open the rollout job">
                  job {short(server.last_job)}
                </button>
              )}
            </span>
          </div>
          {server?.last_error && <Problem warn error={new Error(`Last automatic rollout: ${server.last_error}`)} />}
        </div>
      )}
    </Panel>
  );
}
