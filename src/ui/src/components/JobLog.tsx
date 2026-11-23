import { CircleCheck, CircleDot, CircleX, Clock } from "lucide-react";
import { useState } from "react";

import { useJob, useJobSteps } from "../api/hooks";
import { CancelJobButton } from "./CancelJob";
import { JobConsoleById } from "./Console";
import { Status } from "./ui";

/**
 * The command's output: streamed live from the agent while the job runs,
 * then the full output the job stored. Rendered through `Console`.
 */
export function JobLog({ jobId, height = "h-72" }: { jobId: string; height?: string }) {
  return <JobConsoleById jobId={jobId} height={height} />;
}

function StepDot({ status }: { status: string }) {
  if (status === "succeeded") return <CircleCheck className="size-3.5 text-ok" />;
  if (status === "failed") return <CircleX className="size-3.5 text-bad" />;
  if (status === "running") return <CircleDot className="size-3.5 animate-pulse text-accent" />;
  return <Clock className="size-3.5 text-faint" />;
}

/** Tabs per node, each with its job's steps, live console and a Cancel while it runs. */
export function NodeJobTabs({ jobs }: { jobs: { nodeId: string; nodeName: string; jobId: string }[] }) {
  const list = Array.isArray(jobs) ? jobs.filter((j) => j && j.jobId) : [];
  const [tab, setTab] = useState(list[0]?.nodeId ?? "");
  const current = list.find((j) => j.nodeId === tab) ?? list[0];
  if (!current) return <p className="text-xs text-faint">No jobs were started.</p>;
  return (
    <div>
      {/* overflow-y-hidden: the tabs' -mb-px (underline on the border) made the row 1px taller and showed a stray vertical scrollbar. */}
      <div role="tablist" className="mb-3 flex gap-1 overflow-x-auto overflow-y-hidden border-b border-line">
        {list.map((j) => (
          <NodeTab key={j.nodeId} {...j} active={j.nodeId === current.nodeId} onClick={() => setTab(j.nodeId)} />
        ))}
      </div>
      <JobPane key={current.jobId} jobId={current.jobId} nodeName={current.nodeName} />
    </div>
  );
}

function NodeTab({ nodeName, jobId, active, onClick }: { nodeName: string; jobId: string; active: boolean; onClick: () => void }) {
  const job = useJob(jobId);
  return (
    <button
      role="tab"
      aria-selected={active}
      onClick={onClick}
      className={`-mb-px inline-flex items-center gap-2 border-b-2 px-3 py-2 text-[13px] whitespace-nowrap ${active ? "border-accent text-accent" : "border-transparent text-muted hover:text-fg"}`}
    >
      {nodeName}
      {job.data && <Status value={job.data.status} />}
    </button>
  );
}

function JobPane({ jobId, nodeName }: { jobId: string; nodeName: string }) {
  const job = useJob(jobId);
  const steps = useJobSteps(jobId);
  const list = Array.isArray(steps.data) ? steps.data : [];
  return (
    <div className="space-y-2">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs">
        {list.map((s) => (
          <span key={s.index} className="inline-flex items-center gap-1.5 font-mono text-[11px]" title={s.error ?? undefined}>
            <StepDot status={s.status} /> {s.name}
          </span>
        ))}
        <span className="flex-1" />
        {job.data && <CancelJobButton job={job.data} title={`Job on ${nodeName}`} />}
      </div>
      {job.data?.status === "failed" && job.data.last_error && (
        <div role="alert" className="rounded-md border border-bad/40 bg-bad-soft px-3 py-1.5 font-mono text-[11.5px] break-words whitespace-pre-wrap text-bad">
          {job.data.last_error}
        </div>
      )}
      <JobLog jobId={jobId} />
    </div>
  );
}
