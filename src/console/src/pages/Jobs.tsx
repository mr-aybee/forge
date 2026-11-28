import { Workflow } from "lucide-react";
import { useState } from "react";

import { useJobs } from "../api/hooks";
import type { JobStatus } from "../api/types";
import { CancelJobButton } from "../components/CancelJob";
import { JobDetail, TargetLink } from "../components/JobsDrawer";
import { JobStatusPill, KindIcon, jobTone, toneBorder, toneText } from "../components/JobVisuals";
import { elapsed, isActive, isBackground, jobAction, jobTarget, jobTitle, useJobNames, useNow } from "../components/jobMeta";
import { Drawer, Empty, ErrorNote, PageHeader, Panel, SearchInput, Select, Skeleton, Table, Td, ago, duration, short, time } from "../components/ui";

const STATUSES: (JobStatus | "")[] = ["", "queued", "running", "succeeded", "failed", "cancelled"];

export function Jobs() {
  const jobs = useJobs();
  const [q, setQ] = useState("");
  const [status, setStatus] = useState<JobStatus | "">("");
  const [open, setOpen] = useState<string>();
  const names = useJobNames();
  const now = useNow((jobs.data ?? []).some(isActive));

  const rows = (jobs.data ?? []).filter(
    (j) =>
      (!status || j.status === status) &&
      `${jobTitle(j, names)} ${jobTarget(j, names).label} ${j.kind} ${j.id} ${j.last_error ?? ""}`.toLowerCase().includes(q.toLowerCase()),
  );
  const counts = (s: JobStatus) => (jobs.data ?? []).filter((j) => j.status === s).length;

  return (
    <>
      <PageHeader
        eyebrow="Operations"
        title="Jobs"
        description="Every operation runs as a durable job: steps are persisted, re-authorized before each step, retried with backoff, and resumed after a restart."
      />
      <div className="mb-4 flex flex-wrap gap-2 text-xs">
        {(["running", "queued", "failed", "succeeded"] as const).map((s) => (
          <button key={s} onClick={() => setStatus(status === s ? "" : s)} className={`rounded-full border px-3 py-1 ${status === s ? "border-accent text-accent" : "border-line text-muted"}`}>
            {s} · {counts(s)}
          </button>
        ))}
      </div>
      <Panel
        title={`${rows.length} job${rows.length === 1 ? "" : "s"}`}
        actions={
          <div className="flex gap-2">
            <SearchInput value={q} onChange={setQ} placeholder="Search jobs, nodes, errors" />
            <Select className="w-auto py-1.5" value={status} onChange={(e) => setStatus(e.target.value as JobStatus | "")} aria-label="Filter status">
              {STATUSES.map((s) => (
                <option key={s} value={s}>
                  {s || "Any status"}
                </option>
              ))}
            </Select>
          </div>
        }
      >
        <ErrorNote error={jobs.error} />
        {jobs.isLoading ? (
          <Skeleton />
        ) : rows.length === 0 ? (
          <Empty icon={<Workflow className="size-5" />} title={(jobs.data ?? []).length ? "No jobs match" : "No jobs yet"}>
            {(jobs.data ?? []).length ? "Change the filter or search." : "Installs, updates and agent commands appear here as they run."}
          </Empty>
        ) : (
          <Table head={["Job", "Target", "Status", "Duration", "Attempts", "Created", "Error", ""]}>
            {rows.map((j) => {
              const title = jobTitle(j, names);
              return (
                <tr key={j.id} className="cursor-pointer hover:bg-panel-2/60" onClick={() => setOpen(j.id)}>
                  <Td className={`max-w-[360px] border-l-[3px] !py-1.5 ${toneBorder[jobTone(j)]}`}>
                    <div className="flex min-w-0 items-center gap-2">
                      <KindIcon job={j} />
                      <div className="min-w-0">
                        <div className={`truncate ${isBackground(j) ? "text-muted" : "font-medium"}`} title={title}>
                          {jobAction(j, names)}
                        </div>
                        <div className="truncate font-mono text-[11px] text-faint">
                          {j.kind} · {short(j.id)}
                        </div>
                      </div>
                    </div>
                  </Td>
                  <Td className="max-w-[160px] text-muted" onClick={(e) => e.stopPropagation()}>
                    <TargetLink target={jobTarget(j, names)} />
                  </Td>
                  <Td>
                    <JobStatusPill job={j} />
                  </Td>
                  <Td className="font-mono text-[11px] text-muted tabular-nums">{duration(elapsed(j, now))}</Td>
                  <Td className="font-mono text-[11px] tabular-nums">
                    {Math.min((j.attempts || 0) + 1, Math.max(1, j.max_attempts || 1))}/{Math.max(1, j.max_attempts || 1)}
                  </Td>
                  <Td className="whitespace-nowrap text-muted">
                    <span title={time(j.created_at)}>{ago(j.created_at)}</span>
                  </Td>
                  <Td className="max-w-xs">
                    {j.last_error ? (
                      <span className={`block truncate font-mono text-[11px] ${j.status === "failed" ? toneText[jobTone(j)] : "text-warn"}`} title={j.last_error}>
                        {j.last_error}
                      </span>
                    ) : (
                      <span className="text-faint">—</span>
                    )}
                  </Td>
                  <Td className="text-right" onClick={(e) => e.stopPropagation()}>
                    <CancelJobButton job={j} title={title} compact />
                  </Td>
                </tr>
              );
            })}
          </Table>
        )}
      </Panel>
      <Drawer open={!!open} onOpenChange={(o) => !o && setOpen(undefined)} title="Job details" wide>
        {open && <JobDetail id={open} onNavigate={() => setOpen(undefined)} />}
      </Drawer>
    </>
  );
}
