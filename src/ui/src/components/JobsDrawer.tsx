import { Link } from "@tanstack/react-router";
import { ArrowLeft, ChevronRight, CircleCheck, CircleDot, CircleX, Clock, RotateCw, Workflow, XCircle } from "lucide-react";
import { type ReactNode, useMemo, useState } from "react";

import { useJob, useJobSteps, useJobs } from "../api/hooks";
import type { Job, JobStatus, JobStep } from "../api/types";
import { CancelJobButton } from "./CancelJob";
import { JobConsole, jobHasConsole } from "./Console";
import { type JobNames, type JobTarget, elapsed, isActive, jobTarget, jobTitle, startedAt, useJobNames, useNow } from "./jobMeta";
import { CopyIcon, Drawer, Empty, ErrorNote, SearchInput, Skeleton, Status, ago, duration, short, time } from "./ui";

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

function StepIcon({ status }: { status: string }) {
  if (status === "succeeded") return <CircleCheck className="size-3.5 text-ok" />;
  if (status === "failed") return <CircleX className="size-3.5 text-bad" />;
  if (status === "retrying") return <RotateCw className="size-3.5 text-warn" />;
  if (status === "running") return <CircleDot className="size-3.5 animate-pulse text-accent" />;
  return <Clock className="size-3.5 text-faint" />;
}

/** Step list with per-step durations and errors. */
export function StepTimeline({ steps, now, waiting }: { steps: JobStep[]; now: number; waiting?: boolean }) {
  if (!steps.length)
    return <p className="text-xs text-faint">{waiting ? "Waiting for a worker to pick the job up…" : "No steps were recorded."}</p>;
  return (
    <ol className="divide-y divide-line rounded-md border border-line">
      {steps.map((st) => {
        const end = st.finished_at ?? (st.status === "running" || st.status === "retrying" ? now : null);
        return (
          <li key={st.index} className="px-3 py-1.5">
            <div className="flex items-center gap-2 text-[13px]">
              <StepIcon status={st.status} />
              <span className="min-w-0 flex-1 truncate font-mono text-xs" title={st.name}>
                {st.name}
              </span>
              <span className="text-[11px] text-faint">{st.status}</span>
              <span className="w-16 text-right font-mono text-[11px] text-muted tabular-nums" title={st.started_at ? `Started ${time(st.started_at)}` : undefined}>
                {end && st.started_at ? duration(end - st.started_at) : "—"}
              </span>
            </div>
            {st.error && <div className="mt-0.5 pl-5.5 font-mono text-[11px] break-words whitespace-pre-wrap text-bad">{st.error}</div>}
          </li>
        );
      })}
    </ol>
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

function Json({ value }: { value: unknown }) {
  let text: string;
  try {
    text = JSON.stringify(value ?? {}, null, 2);
  } catch {
    text = String(value);
  }
  return (
    <div className="relative">
      <pre className="max-h-64 overflow-auto rounded-md border border-line bg-panel-2 p-2.5 pr-8 font-mono text-[11px] leading-relaxed">{text}</pre>
      <CopyIcon text={text} label="Copy JSON" className="absolute top-1.5 right-1.5" />
    </div>
  );
}

// ------------------------------------------------------------------ detail

/** One job: header, step timeline, error, console, raw details, cancel. Reused across the console. */
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
  const title = jobTitle(j, names);
  const target = jobTarget(j, names);
  const start = startedAt(j, list);
  const failedStep = list.find((x) => x.status === "failed");
  const raw = rawState(j);

  return (
    <div className="space-y-4">
      <header className="space-y-2">
        <div className="flex items-start gap-2">
          <h2 className="min-w-0 flex-1 text-[15px] leading-snug font-semibold break-words">{title}</h2>
          <Status value={j.status} />
        </div>
        <dl className="grid grid-cols-2 gap-x-4 gap-y-2 sm:grid-cols-4">
          <Meta k="Job">
            <span className="truncate font-mono text-[11px]" title={j.id}>
              {short(j.id)}
            </span>
            <CopyIcon text={j.id} label="Copy job id" />
          </Meta>
          <Meta k="Kind">
            <span className="truncate font-mono text-[11px]">{j.kind || "—"}</span>
          </Meta>
          <Meta k="Target">
            <TargetLink target={target} onNavigate={onNavigate} />
          </Meta>
          <Meta k="Attempt">
            <span className="font-mono text-[11px] tabular-nums">
              {Math.min((j.attempts || 0) + 1, Math.max(1, j.max_attempts || 1))}/{Math.max(1, j.max_attempts || 1)}
            </span>
          </Meta>
          <Meta k="Started">
            <span title={time(start)}>{start ? ago(start) : "—"}</span>
          </Meta>
          <Meta k="Finished">
            <span title={active ? undefined : time(j.updated_at)}>{active ? "—" : ago(j.updated_at)}</span>
          </Meta>
          <Meta k={active ? "Running for" : "Duration"}>
            <span className="font-mono text-[11px] tabular-nums">{duration(elapsed(j, now, list))}</span>
          </Meta>
          {active && (
            <div className="flex items-end justify-end">
              <CancelJobButton job={j} title={title} />
            </div>
          )}
        </dl>
      </header>

      {j.status === "failed" && (
        <div role="alert" className="rounded-md border border-bad/40 bg-bad-soft px-3 py-2 text-bad">
          <div className="flex items-center gap-1.5 text-[13px] font-medium">
            <XCircle className="size-3.5" /> Failed{failedStep ? ` at step ${failedStep.name}` : ""}
          </div>
          <div className="mt-1 font-mono text-[11.5px] break-words whitespace-pre-wrap">{j.last_error || failedStep?.error || "No error message was recorded."}</div>
        </div>
      )}
      {j.status !== "failed" && j.last_error && (
        <div className="rounded-md border border-warn/40 bg-warn-soft px-3 py-2 text-xs text-warn">
          <span className="font-medium">{active ? "Last attempt failed; retrying. " : "Last error: "}</span>
          <span className="font-mono break-words">{j.last_error}</span>
        </div>
      )}

      <Section title="Steps">
        <StepTimeline steps={list} now={now} waiting={active} />
      </Section>

      {jobHasConsole(j) && (
        <Section title="Console">
          <JobConsole job={j} height={consoleHeight} />
        </Section>
      )}

      <details className="group">
        <summary className="flex cursor-pointer list-none items-center gap-1 text-[11px] font-semibold tracking-wider text-faint uppercase select-none hover:text-fg">
          <ChevronRight className="size-3.5 transition group-open:rotate-90" /> Details
        </summary>
        <div className="mt-2 space-y-2">
          <div className="text-[11px] text-faint">
            Created {time(j.created_at)} · updated {time(j.updated_at)}
          </div>
          <div className="text-[11px] text-muted">Payload</div>
          <Json value={j.payload} />
          <div className="text-[11px] text-muted">State</div>
          {Object.keys(raw).length ? <Json value={raw} /> : <p className="text-xs text-faint">—</p>}
        </div>
      </details>
    </div>
  );
}

// ------------------------------------------------------------------ list

type Filter = "all" | "active" | JobStatus;
const FILTERS: { id: Filter; label: string; match: (s: JobStatus) => boolean }[] = [
  { id: "all", label: "All", match: () => true },
  { id: "active", label: "Active", match: (s) => s === "queued" || s === "running" },
  { id: "failed", label: "Failed", match: (s) => s === "failed" },
  { id: "succeeded", label: "Succeeded", match: (s) => s === "succeeded" },
  { id: "cancelled", label: "Cancelled", match: (s) => s === "cancelled" },
];

function JobRow({ j, names, now, onOpen }: { j: Job; names: JobNames; now: number; onOpen: () => void }) {
  const title = jobTitle(j, names);
  const target = jobTarget(j, names);
  const active = isActive(j);
  return (
    <li className="group flex items-center gap-3 px-3 py-1.5 hover:bg-panel-2/60">
      <button onClick={onOpen} className="min-w-0 flex-1 text-left">
        <div className="truncate text-[13px] font-medium group-hover:text-accent" title={title}>
          {title}
        </div>
        <div className="flex min-w-0 items-center gap-1.5 text-[11px] text-faint">
          <span className="truncate font-mono">{j.kind}</span>
          <span>·</span>
          <span className="font-mono">{short(j.id)}</span>
          <span>·</span>
          <span className="whitespace-nowrap" title={time(j.created_at)}>
            {ago(j.created_at)}
          </span>
        </div>
        {j.status === "failed" && j.last_error && (
          <div className="truncate font-mono text-[11px] text-bad" title={j.last_error}>
            {j.last_error}
          </div>
        )}
      </button>
      <div className="hidden w-32 min-w-0 text-xs text-muted sm:block">
        <TargetLink target={target} />
      </div>
      <div className="w-[88px] shrink-0">
        <Status value={j.status} />
      </div>
      <div className="w-16 shrink-0 text-right font-mono text-[11px] text-muted tabular-nums" title={active ? "Running for" : "Duration"}>
        {duration(elapsed(j, now))}
      </div>
      <div className="w-7 shrink-0">
        <CancelJobButton job={j} title={title} compact />
      </div>
    </li>
  );
}

function Group({ label, children, count }: { label: string; count: number; children: ReactNode }) {
  if (!count) return null;
  return (
    <section>
      <h3 className="border-b border-line bg-panel-2/50 px-3 py-1 text-[11px] font-semibold tracking-wider text-faint uppercase">
        {label} <span className="font-mono tabular-nums">{count}</span>
      </h3>
      <ul className="divide-y divide-line">{children}</ul>
    </section>
  );
}

/** Jobs list grouped Running / Recent, with filter, search and per-row cancel; opens a job's detail. */
export function JobsDrawer({ open, onOpenChange }: { open: boolean; onOpenChange: (o: boolean) => void }) {
  const jobs = useJobs();
  const names = useJobNames();
  const [selected, setSelected] = useState<string>();
  const [filter, setFilter] = useState<Filter>("all");
  const [q, setQ] = useState("");
  const all = useMemo(() => (Array.isArray(jobs.data) ? jobs.data : []), [jobs.data]);
  const now = useNow(open && all.some(isActive));

  const needle = q.trim().toLowerCase();
  const current = FILTERS.find((f) => f.id === filter) ?? FILTERS[0]!;
  const rows = all.filter(
    (j) =>
      current.match(j.status) &&
      (!needle || `${jobTitle(j, names)} ${jobTarget(j, names).label} ${j.kind} ${j.id} ${j.last_error ?? ""}`.toLowerCase().includes(needle)),
  );
  const running = rows.filter(isActive);
  const recent = rows.filter((j) => !isActive(j)).slice(0, 100);
  const close = () => onOpenChange(false);

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
            <div role="tablist" aria-label="Job status" className="flex gap-0.5 rounded-md bg-panel-2 p-0.5">
              {FILTERS.map((f) => {
                const n = all.filter((j) => f.match(j.status)).length;
                return (
                  <button
                    key={f.id}
                    role="tab"
                    aria-selected={filter === f.id}
                    onClick={() => setFilter(f.id)}
                    className={`flex items-center gap-1 rounded px-2 py-0.5 text-xs whitespace-nowrap ${filter === f.id ? "bg-panel font-medium text-fg shadow-sm" : "text-muted hover:text-fg"}`}
                  >
                    {f.label}
                    <span className={`font-mono text-[10px] tabular-nums ${filter === f.id ? "text-accent" : "text-faint"}`}>{n}</span>
                  </button>
                );
              })}
            </div>
            <div className="min-w-[160px] flex-1 [&_input]:w-full [&_label]:w-full">
              <SearchInput value={q} onChange={setQ} placeholder="Search jobs, nodes, errors" />
            </div>
          </div>

          <ErrorNote error={jobs.error} />
          {jobs.isLoading ? (
            <Skeleton rows={6} />
          ) : rows.length === 0 ? (
            <Empty icon={<Workflow className="size-5" />} title={all.length ? "No jobs match" : "No jobs yet"}>
              {all.length ? "Change the filter or search." : "Installs, updates and agent commands appear here as they run."}
            </Empty>
          ) : (
            <div className="overflow-hidden rounded-md border border-line">
              <Group label="Running" count={running.length}>
                {running.map((j) => (
                  <JobRow key={j.id} j={j} names={names} now={now} onOpen={() => setSelected(j.id)} />
                ))}
              </Group>
              <Group label="Recent" count={recent.length}>
                {recent.map((j) => (
                  <JobRow key={j.id} j={j} names={names} now={now} onOpen={() => setSelected(j.id)} />
                ))}
              </Group>
            </div>
          )}
          <div className="text-right">
            <Link to="/jobs" onClick={close} className="text-xs text-muted hover:text-accent">
              Open the Jobs page →
            </Link>
          </div>
        </div>
      )}
    </Drawer>
  );
}
