import { useQuery } from "@tanstack/react-query";
import { ArrowDown, Check, Copy, Loader2, Terminal } from "lucide-react";
import { Fragment, type ReactNode, useLayoutEffect, useMemo, useRef, useState } from "react";

import { get } from "../api/client";
import { normalizeLines, useJob, useOnboardingConsole } from "../api/hooks";
import type { ConsoleLine, Job } from "../api/types";
import { useToast } from "../app/toast";
import { COPY_BLOCKED, copyText } from "../lib/clipboard";
import { isActive } from "./jobMeta";

// ------------------------------------------------------------------ line model

export type LineKind = "cmd" | "err" | "done" | "out";

/** Shell-style classification: `$ cmd` prompts, installer/agent errors, the rest is output. */
export function lineKind(line: string): LineKind {
  if (line.startsWith("$ ")) return "cmd";
  if (/^forge-install: error:/i.test(line) || /^error:/i.test(line) || /^installer failed \(exit -?\d+\)/i.test(line)) return "err";
  if (line === "installer finished") return "done";
  return "out";
}

/** Plain text (agent output) as console lines. */
export function textLines(text: string | null | undefined): ConsoleLine[] {
  if (!text) return [];
  const parts = text.replace(/\r\n?/g, "\n").split("\n");
  if (parts[parts.length - 1] === "") parts.pop();
  return parts.map((line) => ({ line }));
}

const clock = (t: number) => {
  const d = new Date(t);
  return Number.isNaN(d.getTime()) ? "" : d.toLocaleTimeString([], { hour12: false });
};

// ------------------------------------------------------------------ renderer

/**
 * Dark terminal panel for install and job output. `$ command` lines are
 * prompts, errors are red, the rest is plain. Follows the end until the user
 * scrolls up; "Follow" resumes. Text is selectable; the copy button takes it all.
 */
export function Console({
  lines,
  active,
  title = "Console",
  height = "h-72",
  placeholder,
  status,
}: {
  lines: ConsoleLine[];
  /** Still producing output: shows a live marker and waiting text. */
  active?: boolean;
  title?: ReactNode;
  /** Tailwind height class of the scroll area. */
  height?: string;
  placeholder?: string;
  /** Short state text in the header (e.g. "failed"). */
  status?: ReactNode;
}) {
  const safe = Array.isArray(lines) ? lines : [];
  const box = useRef<HTMLDivElement>(null);
  const [follow, setFollow] = useState(true);
  const [copied, setCopied] = useState(false);
  const toast = useToast();
  const timed = safe.some((l) => typeof l.t === "number" && l.t > 0);
  const stepped = useMemo(() => new Set(safe.map((l) => l.step).filter(Boolean)).size > 1, [safe]);

  useLayoutEffect(() => {
    if (follow && box.current) box.current.scrollTop = box.current.scrollHeight;
  }, [safe.length, follow]);

  const copyAll = () => {
    void copyText(safe.map((l) => l.line).join("\n")).then((ok) => {
      if (!ok) return toast(COPY_BLOCKED, "error");
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    });
  };

  return (
    <div className="log-console overflow-hidden rounded-md border border-line bg-term text-term-fg">
      <div className="flex min-w-0 items-center gap-2 border-b border-term-line px-3 py-1.5 text-[11px] text-term-muted">
        <Terminal className="size-3.5 shrink-0" />
        <span className="min-w-0 flex-1 truncate">{title}</span>
        {status}
        {active && (
          <span className="inline-flex items-center gap-1 whitespace-nowrap">
            <span aria-hidden className="size-1.5 animate-pulse rounded-full bg-term-ok" /> live
          </span>
        )}
        <span className="whitespace-nowrap tabular-nums">{safe.length} lines</span>
        {!follow && (
          <button
            type="button"
            className="inline-flex items-center gap-1 rounded px-1.5 py-0.5 hover:bg-term-2 hover:text-term-fg"
            onClick={() => setFollow(true)}
          >
            <ArrowDown className="size-3.5" /> Follow
          </button>
        )}
        <button
          type="button"
          aria-label="Copy all output"
          title={copied ? "Copied" : "Copy all output"}
          disabled={!safe.length}
          onClick={copyAll}
          className="grid size-6 place-items-center rounded hover:bg-term-2 hover:text-term-fg disabled:opacity-40"
        >
          {copied ? <Check className="size-3.5 text-term-ok" /> : <Copy className="size-3.5" />}
        </button>
      </div>
      <div
        ref={box}
        role="log"
        aria-label={typeof title === "string" ? title : "Console output"}
        tabIndex={0}
        onScroll={(e) => {
          const el = e.currentTarget;
          setFollow(el.scrollHeight - el.scrollTop - el.clientHeight < 24);
        }}
        className={`${height} overflow-auto py-2 font-mono text-[11.5px] leading-[1.6] focus-visible:outline-offset-[-2px]`}
      >
        {safe.length === 0 ? (
          <div className="flex items-center gap-2 px-3 text-term-muted">
            {active && <Loader2 className="size-3.5 animate-spin" />}
            {placeholder ?? (active ? "Waiting for output…" : "No output.")}
          </div>
        ) : (
          safe.map((l, i) => {
            const kind = lineKind(l.line);
            const newStep = stepped && l.step && safe[i - 1]?.step !== l.step;
            return (
              <Fragment key={i}>
                {newStep && (
                  <div className={`flex items-center gap-2 px-3 pb-0.5 text-[10px] tracking-wider text-term-muted uppercase select-none ${i > 0 ? "pt-2" : ""}`}>
                    <span className="h-px w-3 bg-term-line" />
                    {l.step}
                    <span className="h-px flex-1 bg-term-line" />
                  </div>
                )}
                <div className="flex gap-3 px-3 hover:bg-term-2">
                  {timed && (
                    <span
                      className="hidden w-[52px] shrink-0 text-term-muted/70 tabular-nums select-none sm:block"
                      title={l.t ? new Date(l.t).toLocaleString() : undefined}
                    >
                      {l.t ? clock(l.t) : ""}
                    </span>
                  )}
                  <span
                    className={`min-w-0 flex-1 break-words whitespace-pre-wrap ${
                      kind === "err" ? "text-term-bad" : kind === "done" ? "text-term-ok" : kind === "cmd" ? "font-semibold text-term-fg" : "text-term-fg/85"
                    }`}
                  >
                    {kind === "cmd" ? (
                      <>
                        <span className="text-term-prompt select-none">$ </span>
                        {l.line.slice(2)}
                      </>
                    ) : (
                      l.line
                    )}
                  </span>
                </div>
              </Fragment>
            );
          })
        )}
      </div>
    </div>
  );
}

// ------------------------------------------------------------------ feeds

/** Final output a job stored (`state.output`), as text. */
function storedOutput(j?: Job | null): string | null {
  const o = j?.state?.output;
  if (o === undefined || o === null) return null;
  if (typeof o === "string") return o;
  try {
    return JSON.stringify(o, null, 2);
  } catch {
    return String(o);
  }
}

/** Whether a job has (or will have) console output worth showing. */
export function jobHasConsole(j?: Job | null): boolean {
  if (!j) return false;
  if (["node.bootstrap", "package.action", "app.action"].includes(j.kind)) return true;
  const st = j.state ?? {};
  return st.operation_id !== undefined || st.output !== undefined || Array.isArray(st.log);
}

/** Onboarding (`node.bootstrap`): live console lines, falling back to the job's stored `state.log`. */
function BootstrapConsole({ job, height }: { job: Job; height?: string }) {
  const active = isActive(job);
  const live = useOnboardingConsole({ jobId: job.id });
  const stored = normalizeLines(job.state?.log);
  const fromLive = live.data && !live.data.unavailable ? live.data.lines : [];
  const lines = fromLive.length >= stored.length ? fromLive : stored;
  return (
    <Console
      lines={lines}
      active={active}
      title="Install console"
      height={height}
      placeholder={active ? "Connecting to the machine…" : "No console output was recorded."}
    />
  );
}

/** Package / agent jobs: live agent output while running, then `state.output`. */
function OutputConsole({ job, height }: { job: Job; height?: string }) {
  const active = isActive(job);
  const live = useQuery({
    queryKey: ["job-live", job.id],
    queryFn: () =>
      get<{ output?: string | null; status?: string }>(`/api/v1/jobs/${job.id}/live`)
        .then((r) => ({ output: typeof r?.output === "string" ? r.output : null }))
        .catch(() => ({ output: null })),
    refetchInterval: active ? 1_500 : false,
    enabled: active,
  });
  const text = (active ? live.data?.output : null) ?? storedOutput(job) ?? live.data?.output ?? "";
  return (
    <Console
      lines={textLines(text)}
      active={active}
      title={active ? "Live output" : "Output"}
      height={height}
      placeholder={active ? "Waiting for output from the node…" : "No output."}
    />
  );
}

/** The right console for any job: onboarding console or agent output. */
export function JobConsole({ job, height }: { job: Job; height?: string }) {
  return job.kind === "node.bootstrap" ? <BootstrapConsole job={job} height={height} /> : <OutputConsole job={job} height={height} />;
}

/** `JobConsole` by job id (loads the job). */
export function JobConsoleById({ jobId, height }: { jobId: string; height?: string }) {
  const job = useJob(jobId);
  if (!job.data) return <Console lines={[]} active={!job.error} height={height} placeholder={job.error ? "Job not found." : "Loading…"} />;
  return <JobConsole job={job.data} height={height} />;
}

/** A manual install's console, reported by the installer with its enrollment token. */
export function TokenConsole({ tokenId, height }: { tokenId: string; height?: string }) {
  const c = useOnboardingConsole({ tokenId });
  const d = c.data;
  const running = !d || d.status === "running" || d.status === "queued";
  return (
    <Console
      lines={d?.lines ?? []}
      active={running && !d?.unavailable}
      title="Installer console"
      height={height}
      status={d && !d.unavailable && d.status !== "running" ? <span className={d.status === "failed" ? "text-term-bad" : "text-term-ok"}>{d.status}</span> : undefined}
      placeholder={
        d?.unavailable
          ? "This control plane does not report installer progress."
          : "Waiting for the installer. Lines appear here once the command runs on the machine."
      }
    />
  );
}
