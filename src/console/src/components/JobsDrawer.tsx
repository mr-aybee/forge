import { Link } from "@tanstack/react-router";
import { ArrowLeft, ArrowRight, ChevronRight, Eye, EyeOff, Lightbulb, Workflow } from "lucide-react";
import { type ReactNode, useMemo, useState } from "react";

import { useJob, useJobSteps, useJobs } from "../api/hooks";
import type { Job, JobStatus, JobStep } from "../api/types";
import { CancelJobButton } from "./CancelJob";
import { JobConsole, jobHasConsole } from "./Console";
import { JobStatusPill, JsonView, KindIcon, ResultCard, StatusIcon, jobTone, resultFields, statusLabel, toneBand, toneBorder, toneText } from "./JobVisuals";
import { type JobNames, type JobTarget, elapsed, isActive, isBackground, jobAction, jobTarget, jobTitle, startedAt, useJobNames, useNow } from "./jobMeta";
import { CopyIcon, Drawer, Empty, ErrorNote, SearchInput, Skeleton, ago, duration, short, time } from "./ui";

// ------------------------------------------------------------------ shared bits

/** Node or cluster a job acts on, as a link. */
export function TargetLink({ target, onNavigate }: { target: JobTarget; onNavigate?: () => void }) {
  const cls = "block min-w-0 truncate hover:text-accent";
  if (target.kind === "node" && target.id)
    return (
      <Link to="/nodes/$nodeId" params={{ nodeId: target.id }} className={cls} title={target.label} onClick={onNavigate}>
        {target.label}
      </Link>
    );
  if (target.kind === "cluster" && target.id)
    return (
      <Link to="/clusters/$clusterId" params={{ clusterId: target.id }} className={cls} title={`Cluster ${target.label}`} onClick={onNavigate}>
        {target.label}
      </Link>
    );
  return (
    <span className={`block min-w-0 truncate ${target.kind === "host" ? "font-mono text-[11px]" : "text-faint"}`} title={target.label}>
      {target.label}
    </span>
  );
}

/** Target as a small chip (node / cluster link). */
export function TargetChip({ target, onNavigate }: { target: JobTarget; onNavigate?: () => void }) {
  if (target.label === "—") return null;
  return (
    <span className="inline-flex max-w-[11rem] min-w-0 items-center rounded border border-line bg-panel-2 px-1.5 py-px text-[11px] text-muted" onClick={(e) => e.stopPropagation()}>
      {target.kind === "cluster" && <span className="mr-1 text-faint">cluster</span>}
      <TargetLink target={target} onNavigate={onNavigate} />
    </span>
  );
}

const stepTone = (status: string) =>
  status === "succeeded"
    ? { dot: "bg-ok", text: "text-ok", box: "border-ok/40 bg-ok-soft" }
    : status === "failed"
      ? { dot: "bg-bad", text: "text-bad", box: "border-bad/40 bg-bad-soft" }
      : status === "retrying"
        ? { dot: "bg-warn", text: "text-warn", box: "border-warn/40 bg-warn-soft" }
        : status === "running"
          ? { dot: "bg-accent animate-pulse", text: "text-accent", box: "border-accent/40 bg-accent-soft" }
          : { dot: "bg-line-strong", text: "text-faint", box: "border-line bg-panel-2" };

/** Horizontal step timeline (dispatch → await_result …) with per-step status and duration; errors below. */
export function StepTimeline({ steps, now, waiting }: { steps: JobStep[]; now: number; waiting?: boolean }) {
  const list = Array.isArray(steps) ? steps.filter(Boolean) : [];
  if (!list.length)
    return <p className="text-xs text-faint">{waiting ? "Waiting for a worker to pick the job up…" : "No steps were recorded."}</p>;
  const errors = list.filter((st) => st.error);
  return (
    <div className="space-y-1.5">
      <ol className="flex items-center gap-1 overflow-x-auto pb-1" aria-label="Steps">
        {list.map((st, i) => {
          const end = st.finished_at ?? (st.status === "running" || st.status === "retrying" ? now : null);
          const t = stepTone(st.status);
          return (
            <li key={st.index ?? i} className="flex shrink-0 items-center gap-1">
              {i > 0 && <ArrowRight aria-hidden className="size-3 text-faint" />}
              <span
                className={`inline-flex items-center gap-1.5 rounded-full border px-2 py-0.5 ${t.box}`}
                title={`${st.name}: ${st.status}${st.started_at ? ` · started ${time(st.started_at)}` : ""}`}
              >
                <span aria-hidden className={`size-1.5 rounded-full ${t.dot}`} />
                <span className="font-mono text-[11px] text-fg">{st.name}</span>
                <span className={`font-mono text-[10px] tabular-nums ${t.text}`}>{end && st.started_at ? duration(end - st.started_at) : st.status}</span>
              </span>
            </li>
          );
        })}
      </ol>
      {errors.map((st) => (
        <div key={`e${st.index}`} className="font-mono text-[11px] break-words whitespace-pre-wrap text-bad">
          <span className="text-faint">{st.name}: </span>
          {st.error}
        </div>
      ))}
    </div>
  );
}

function Meta({ k, children }: { k: string; children: ReactNode }) {
  return (
    <div className="min-w-0">
      <dt className="text-[11px] text-faint">{k}</dt>
      <dd className="flex min-w-0 items-center gap-1 text-xs">{children}</dd>
    </div>
  );
}

function Section({ title, children, aside }: { title: string; children: ReactNode; aside?: ReactNode }) {
  return (
    <section className="space-y-1.5">
      <div className="flex items-center justify-between gap-2">
        <h3 className="text-[11px] font-semibold tracking-wider text-faint uppercase">{title}</h3>
        {aside}
      </div>
      {children}
    </section>
  );
}

/** Job state without the console copies (`log`, `output`), which the Console shows. */
function rawState(j: Job): Record<string, unknown> {
  const st = j.state && typeof j.state === "object" ? { ...j.state } : {};
  delete st.log;
  delete st.output;
  return st;
}

/** A next step for common failures. */
function failureHint(err: string): string | undefined {
  const e = err.toLowerCase();
  if (e.includes("did not respond") || e.includes("timed out") || e.includes("offline"))
    return "The agent did not answer in time. Check that the node is online and its agent is running, then retry.";
  if (e.includes("not allowed") || e.includes("denied") || e.includes("forbidden")) return "The node's agent or policy refused this command. Check the node's allowed commands and your role.";
  if (e.includes("nginx -t") || e.includes("nginx: [emerg]")) return "Nginx rejected the generated config; the previous config was restored. See the console for the failing line.";
  if (e.includes("not installed") || e.includes("not found")) return "A required program is missing on the node. Install it from the node's Packages tab.";
  return undefined;
}

// ------------------------------------------------------------------ detail

/** One job: status band, step timeline, error, result, console, raw details, cancel. Reused across the console. */
export function JobDetail({ id, consoleHeight = "h-72", onNavigate }: { id: string; consoleHeight?: string; onNavigate?: () => void }) {
  const job = useJob(id);
  const steps = useJobSteps(id);
  const names = useJobNames();
  const j = job.data;
  const now = useNow(isActive(j));
  if (job.error && !j) return <ErrorNote error={job.error} />;
  if (!j) return <Skeleton rows={3} />;

  const list = Array.isArray(steps.data) ? steps.data : [];
  const active = isActive(j);
  const title = jobAction(j, names);
  const target = jobTarget(j, names);
  const start = startedAt(j, list);
  const failedStep = list.find((x) => x?.status === "failed");
  const raw = rawState(j);
  const tone = jobTone(j);
  const background = isBackground(j);
  const fields = resultFields(j.state?.output);
  const err = j.last_error || failedStep?.error || "";
  const hint = err ? failureHint(err) : undefined;
  const maxAttempts = Math.max(1, j.max_attempts || 1);

  return (
    <div className="space-y-4">
      <header className={`rounded-md border px-3 py-2.5 ${toneBand[tone]}`}>
        <div className="flex items-start gap-3">
          <StatusIcon job={j} className="mt-0.5 size-7 shrink-0" />
          <div className="min-w-0 flex-1 space-y-1">
            <div className="flex items-start gap-2">
              <h2 className="min-w-0 flex-1 text-[15px] leading-snug font-semibold break-words text-fg">{title}</h2>
              <span className={`text-xs font-semibold whitespace-nowrap uppercase ${toneText[tone]}`}>{statusLabel(j)}</span>
            </div>
            <div className="flex min-w-0 flex-wrap items-center gap-1.5">
              <KindIcon job={j} />
              <span className="rounded border border-line bg-panel px-1.5 py-px font-mono text-[11px] text-muted">{j.kind || "—"}</span>
              <TargetChip target={target} onNavigate={onNavigate} />
              {background && <span className="rounded bg-panel px-1.5 py-px text-[11px] text-faint">background check</span>}
              <span className="inline-flex items-center gap-0.5 rounded bg-panel px-1.5 py-px">
                <span className="font-mono text-[11px] text-muted select-all" title={j.id}>
                  {short(j.id)}
                </span>
                <CopyIcon text={j.id} label="Copy job id" />
              </span>
            </div>
          </div>
        </div>
        <dl className="mt-2.5 grid grid-cols-2 gap-x-4 gap-y-2 border-t border-line/60 pt-2 sm:grid-cols-5">
          <Meta k="Started">
            <span title={time(start)}>{start ? ago(start) : "—"}</span>
          </Meta>
          <Meta k="Finished">
            <span title={active ? undefined : time(j.updated_at)}>{active ? "—" : ago(j.updated_at)}</span>
          </Meta>
          <Meta k={active ? "Running for" : "Duration"}>
            <span className="font-mono text-[11px] tabular-nums">{duration(elapsed(j, now, list))}</span>
          </Meta>
          <Meta k="Attempt">
            <span className="font-mono text-[11px] tabular-nums">
              {Math.min((j.attempts || 0) + 1, maxAttempts)}/{maxAttempts}
            </span>
          </Meta>
          {active ? (
            <div className="flex items-end justify-end">
              <CancelJobButton job={j} title={jobTitle(j, names)} />
            </div>
          ) : (
            <Meta k="Created">
              <span title={time(j.created_at)}>{ago(j.created_at)}</span>
            </Meta>
          )}
        </dl>
      </header>

      {j.status === "failed" && (
        <div role="alert" className={`rounded-md border-l-4 px-3 py-2 ${background ? "border-l-warn bg-warn-soft text-warn" : "border-l-bad bg-bad-soft text-bad"}`}>
          <div className="text-[13px] font-semibold">
            {background ? "Check failed" : "Failed"}
            {failedStep ? ` at step ${failedStep.name}` : ""}
          </div>
          <div className="mt-1 font-mono text-[11.5px] break-words whitespace-pre-wrap select-text">{err || "No error message was recorded."}</div>
          {hint && (
            <div className="mt-1.5 flex items-start gap-1.5 text-xs text-fg">
              <Lightbulb aria-hidden className="mt-px size-3.5 shrink-0 text-warn" />
              {hint}
            </div>
          )}
        </div>
      )}
      {j.status !== "failed" && j.last_error && (
        <div className="rounded-md border-l-4 border-l-warn bg-warn-soft px-3 py-2 text-xs text-warn">
          <span className="font-medium">{active ? "Last attempt failed; retrying. " : "Last error: "}</span>
          <span className="font-mono break-words">{j.last_error}</span>
        </div>
      )}

      <Section title="Steps">
        <StepTimeline steps={list} now={now} waiting={active} />
      </Section>

      {fields.length > 0 && (
        <Section title="Result">
          <ResultCard fields={fields} />
        </Section>
      )}

      {jobHasConsole(j) && (
        <Section title="Console">
          <JobConsole job={j} height={consoleHeight} />
        </Section>
      )}

      <details className="group">
        <summary className="flex cursor-pointer list-none items-center gap-1 text-[11px] font-semibold tracking-wider text-faint uppercase select-none hover:text-fg">
          <ChevronRight className="size-3.5 transition group-open:rotate-90" /> Raw payload and result
        </summary>
        <div className="mt-2 space-y-2">
          <div className="text-[11px] text-faint">
            Created {time(j.created_at)} · updated {time(j.updated_at)}
          </div>
          <div className="text-[11px] text-muted">Payload</div>
          <JsonView value={j.payload} />
          {j.state?.output !== undefined && j.state?.output !== null && (
            <>
              <div className="text-[11px] text-muted">Result</div>
              <JsonView value={j.state.output} />
            </>
          )}
          <div className="text-[11px] text-muted">State</div>
          {Object.keys(raw).length ? <JsonView value={raw} /> : <p className="text-xs text-faint">—</p>}
        </div>
      </details>
    </div>
  );
}

// ------------------------------------------------------------------ list

type Filter = "all" | "active" | JobStatus;
const FILTERS: { id: Filter; label: string; tone: string; match: (s: JobStatus) => boolean }[] = [
  { id: "all", label: "All", tone: "text-fg", match: () => true },
  { id: "active", label: "Active", tone: "text-accent", match: (s) => s === "queued" || s === "running" },
  { id: "failed", label: "Failed", tone: "text-bad", match: (s) => s === "failed" },
  { id: "succeeded", label: "Succeeded", tone: "text-ok", match: (s) => s === "succeeded" },
  { id: "cancelled", label: "Cancelled", tone: "text-muted", match: (s) => s === "cancelled" },
];

const BG_KEY = "forge.jobs.background";
function readBg(): boolean {
  try {
    return localStorage.getItem(BG_KEY) === "1";
  } catch {
    return false;
  }
}
function writeBg(v: boolean) {
  try {
    localStorage.setItem(BG_KEY, v ? "1" : "0");
  } catch {
    /* storage unavailable: the choice just isn't remembered */
  }
}

/** A row: one job, or a run of identical background checks folded together. */
interface Item {
  lead: Job;
  count: number;
}

/** Folds consecutive identical background checks (same check, target and outcome) into one item. */
export function foldProbes(rows: Job[], names: JobNames): Item[] {
  const out: Item[] = [];
  let lastKey = "";
  for (const j of rows) {
    const key = isBackground(j) && !isActive(j) ? `${jobTitle(j, names)}|${j.status}` : "";
    const prev = out[out.length - 1];
    if (key && prev && key === lastKey) prev.count += 1;
    else out.push({ lead: j, count: 1 });
    lastKey = key;
  }
  return out;
}

const DAY = 86_400_000;
function timeGroup(t: number, now: number): string {
  if (!t) return "Older";
  if (now - t < 15 * 60_000) return "Just now";
  const midnight = new Date(now);
  midnight.setHours(0, 0, 0, 0);
  const m = midnight.getTime();
  if (t >= m) return "Earlier today";
  if (t >= m - DAY) return "Yesterday";
  return "Older";
}

function JobRow({ item, names, now, onOpen }: { item: Item; names: JobNames; now: number; onOpen: () => void }) {
  const j = item.lead;
  const action = jobAction(j, names);
  const target = jobTarget(j, names);
  const active = isActive(j);
  const tone = jobTone(j);
  const background = isBackground(j);
  return (
    <li className={`group relative flex items-center gap-2.5 border-l-[3px] py-1.5 pr-2 pl-2.5 hover:bg-panel-2/60 ${toneBorder[tone]}`}>
      <KindIcon job={j} />
      <button onClick={onOpen} className="min-w-0 flex-1 rounded text-left outline-none focus-visible:ring-2 focus-visible:ring-accent">
        <div className="flex min-w-0 items-center gap-1.5">
          <span className={`min-w-0 truncate text-[13px] group-hover:text-accent ${background ? "text-muted" : "font-medium text-fg"}`} title={jobTitle(j, names)}>
            {action}
          </span>
          {item.count > 1 && (
            <span className="shrink-0 rounded bg-panel-2 px-1 font-mono text-[10px] text-muted tabular-nums" title={`${item.count} identical checks in a row; showing the latest`}>
              ×{item.count}
            </span>
          )}
        </div>
        <div className="flex min-w-0 items-center gap-1.5 text-[11px] text-faint">
          <span className="font-mono">{short(j.id)}</span>
          <span>·</span>
          <span className="whitespace-nowrap" title={time(j.created_at)}>
            {item.count > 1 ? `last ${ago(j.created_at)}` : ago(j.created_at)}
          </span>
          {background && <span className="truncate">· background check</span>}
        </div>
        {j.status === "failed" && j.last_error && (
          <div className={`truncate font-mono text-[11px] ${background ? "text-warn" : "text-bad"}`} title={j.last_error}>
            {j.last_error}
          </div>
        )}
        {active && (
          <div aria-hidden className="mt-1 h-0.5 w-full overflow-hidden rounded bg-accent-soft">
            <div className={`h-full w-1/3 rounded bg-accent ${j.status === "running" ? "animate-pulse" : "opacity-40"}`} />
          </div>
        )}
      </button>
      <div className="hidden w-32 min-w-0 justify-end sm:flex">
        <TargetChip target={target} />
      </div>
      <div className="hidden w-[104px] shrink-0 justify-end sm:flex">
        <JobStatusPill job={j} />
      </div>
      <div className={`flex w-16 shrink-0 items-center justify-end gap-1 font-mono text-[11px] tabular-nums ${active ? "text-accent" : "text-muted"}`} title={active ? "Running for" : "Duration"}>
        <span className="sm:hidden">
          <StatusIcon job={j} />
        </span>
        {duration(elapsed(j, now))}
      </div>
      <div className="w-7 shrink-0">
        <CancelJobButton job={j} title={jobTitle(j, names)} compact />
      </div>
    </li>
  );
}

function Group({ label, children, count, tone = "text-faint" }: { label: string; count: number; tone?: string; children: ReactNode }) {
  if (!count) return null;
  return (
    <section>
      <h3 className={`sticky top-0 z-10 flex items-center gap-1.5 border-y border-line bg-panel-2 px-3 py-1 text-[11px] font-semibold tracking-wider uppercase ${tone}`}>
        {label} <span className="font-mono font-normal text-faint tabular-nums">{count}</span>
      </h3>
      <ul className="divide-y divide-line">{children}</ul>
    </section>
  );
}

const ORDER = ["Just now", "Earlier today", "Yesterday", "Older"];

/** Jobs list: running first, then grouped by time; status filter with counts, search, background-check toggle. */
export function JobsDrawer({ open, onOpenChange }: { open: boolean; onOpenChange: (o: boolean) => void }) {
  const jobs = useJobs();
  const names = useJobNames();
  const [selected, setSelected] = useState<string>();
  const [filter, setFilter] = useState<Filter>("all");
  const [q, setQ] = useState("");
  const [showBg, setShowBg] = useState(readBg);
  const all = useMemo(() => (Array.isArray(jobs.data) ? jobs.data.filter(Boolean) : []), [jobs.data]);
  const now = useNow(open && all.some(isActive));

  // Failed checks always surface; healthy background checks only on request.
  const hiddenBg = all.filter((j) => isBackground(j) && j.status !== "failed");
  const pool = showBg ? all : all.filter((j) => !(isBackground(j) && j.status !== "failed"));
  const needle = q.trim().toLowerCase();
  const current = FILTERS.find((f) => f.id === filter) ?? FILTERS[0]!;
  const rows = pool.filter(
    (j) =>
      current.match(j.status) &&
      (!needle || `${jobTitle(j, names)} ${jobTarget(j, names).label} ${j.kind} ${j.id} ${j.last_error ?? ""}`.toLowerCase().includes(needle)),
  );
  const running = rows.filter(isActive);
  const done = foldProbes(
    rows.filter((j) => !isActive(j)),
    names,
  ).slice(0, 150);
  const groups = ORDER.map((label) => ({ label, items: done.filter((it) => timeGroup(it.lead.created_at, now) === label) }));
  const close = () => onOpenChange(false);
  const toggleBg = () => {
    writeBg(!showBg);
    setShowBg(!showBg);
  };

  return (
    <Drawer open={open} onOpenChange={onOpenChange} title={selected ? "Job" : "Jobs"} wide>
      {selected ? (
        <div className="space-y-3">
          <button onClick={() => setSelected(undefined)} className="inline-flex items-center gap-1 text-xs text-muted hover:text-fg">
            <ArrowLeft className="size-3.5" /> All jobs
          </button>
          <JobDetail id={selected} onNavigate={close} />
        </div>
      ) : (
        <div className="space-y-3">
          <div className="flex flex-wrap items-center gap-2">
            <div role="tablist" aria-label="Job status" className="flex flex-wrap gap-0.5 rounded-md bg-panel-2 p-0.5">
              {FILTERS.map((f) => {
                const n = pool.filter((j) => f.match(j.status)).length;
                return (
                  <button
                    key={f.id}
                    role="tab"
                    aria-selected={filter === f.id}
                    onClick={() => setFilter(f.id)}
                    className={`flex items-center gap-1 rounded px-2 py-0.5 text-xs whitespace-nowrap ${filter === f.id ? "bg-panel font-medium text-fg shadow-sm" : "text-muted hover:text-fg"}`}
                  >
                    {f.label}
                    <span className={`font-mono text-[10px] tabular-nums ${n ? f.tone : "text-faint"}`}>{n}</span>
                  </button>
                );
              })}
            </div>
            <div className="min-w-[160px] flex-1 [&_input]:w-full [&_label]:w-full">
              <SearchInput value={q} onChange={setQ} placeholder="Search jobs, nodes, errors" />
            </div>
          </div>
          <div className="flex items-center justify-between gap-2 text-[11px] text-faint">
            <button
              type="button"
              aria-pressed={showBg}
              onClick={toggleBg}
              className="inline-flex items-center gap-1 rounded px-1 py-0.5 hover:text-fg focus-visible:ring-2 focus-visible:ring-accent focus-visible:outline-none"
              title="Status polls, fact collection and container lists the console runs on its own. Failed checks always show."
            >
              {showBg ? <EyeOff className="size-3.5" /> : <Eye className="size-3.5" />}
              {showBg ? "Hide background checks" : "Show background checks"}
              {!showBg && hiddenBg.length > 0 && <span className="font-mono tabular-nums">({hiddenBg.length} hidden)</span>}
            </button>
            <Link to="/jobs" onClick={close} className="text-xs text-muted hover:text-accent">
              Open the Jobs page →
            </Link>
          </div>

          <ErrorNote error={jobs.error} />
          {jobs.isLoading ? (
            <Skeleton rows={6} />
          ) : jobs.error && !all.length ? null : rows.length === 0 ? (
            <Empty icon={<Workflow className="size-5" />} title={pool.length ? "No jobs match" : all.length ? "Only background checks" : "No jobs yet"}>
              {pool.length
                ? "Change the filter or search."
                : all.length
                  ? "Every recent job is a healthy background check. Use “Show background checks” to see them."
                  : "Installs, updates and agent commands appear here as they run."}
            </Empty>
          ) : (
            <div className="overflow-clip rounded-md border border-line">
              <Group label="Running" count={running.length} tone="text-accent">
                {running.map((j) => (
                  <JobRow key={j.id} item={{ lead: j, count: 1 }} names={names} now={now} onOpen={() => setSelected(j.id)} />
                ))}
              </Group>
              {groups.map((g) => (
                <Group key={g.label} label={g.label} count={g.items.length}>
                  {g.items.map((it) => (
                    <JobRow key={it.lead.id} item={it} names={names} now={now} onOpen={() => setSelected(it.lead.id)} />
                  ))}
                </Group>
              ))}
            </div>
          )}
        </div>
      )}
    </Drawer>
  );
}
