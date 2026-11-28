import * as Popover from "@radix-ui/react-popover";
import { Link } from "@tanstack/react-router";
import { Bell, CheckCheck, CircleX, ServerOff } from "lucide-react";
import { useCallback, useMemo, useState, type ReactNode } from "react";

import { useJobs, useServers } from "../api/hooks";
import type { Job } from "../api/types";
import { JobDetail } from "./JobsDrawer";
import { jobTarget, jobTitle, useJobNames, type JobTarget } from "./jobMeta";
import { Drawer, ago, time } from "./ui";

const READ_KEY = "forge.notifications.read";
const MAX_READ = 300;
const MAX_JOBS = 30;

/** Human names for job kinds; the item title is "<name> failed". */
const KIND: Record<string, string> = {
  "agent.update": "Agent update",
  "agent.command": "Command",
  "stack.action": "Stack deploy",
  "stack.deploy": "Stack deploy",
  "package.action": "Package action",
  "app.action": "Service action",
  "node.bootstrap": "Node onboarding",
};

const prettyKind = (k: string | undefined) => {
  if (!k) return "Job";
  const known = KIND[k];
  if (known) return known;
  const t = k.replace(/[._-]+/g, " ").trim();
  return t ? t.charAt(0).toUpperCase() + t.slice(1) : "Job";
};

function loadRead(): Set<string> {
  try {
    const v: unknown = JSON.parse(localStorage.getItem(READ_KEY) ?? "[]");
    return new Set(Array.isArray(v) ? v.filter((x): x is string => typeof x === "string") : []);
  } catch {
    return new Set();
  }
}

function saveRead(s: Set<string>) {
  try {
    localStorage.setItem(READ_KEY, JSON.stringify([...s].slice(-MAX_READ)));
  } catch {
    /* storage unavailable: read state stays in memory */
  }
}

/** Renders `code` spans in an error message as inline code. */
function ErrorText({ text }: { text: string }) {
  const parts = text.split("`");
  return (
    <>
      {parts.map((p, i) =>
        i % 2 === 1 && i < parts.length - 1 ? (
          <code key={i} className="rounded bg-panel-2 px-1 font-mono text-[10.5px] text-fg">
            {p}
          </code>
        ) : (
          <span key={i}>{i % 2 === 1 ? `\`${p}` : p}</span>
        ),
      )}
    </>
  );
}

interface Item {
  key: string;
  tone: "bad" | "warn";
  icon: ReactNode;
  title: string;
  detail?: string;
  target?: JobTarget;
  error?: string;
  at: number | null;
  jobId?: string;
  nodeId?: string;
}

export function Notifications() {
  const jobs = useJobs();
  const servers = useServers();
  const names = useJobNames();
  const [open, setOpen] = useState(false);
  const [job, setJob] = useState<string>();
  const [read, setRead] = useState(loadRead);

  const items = useMemo<Item[]>(() => {
    const list: Item[] = [];
    for (const s of Array.isArray(servers.data) ? servers.data : []) {
      if (s?.status !== "offline") continue;
      list.push({
        key: `node:${s.id}:${s.last_seen_at ?? ""}`,
        tone: "warn",
        icon: <ServerOff className="size-3.5" />,
        title: "Node offline",
        target: { kind: "node", id: s.id, label: s.hostname || s.id.slice(0, 8) },
        error: s.last_seen_at ? undefined : "Never connected.",
        at: s.last_seen_at ?? null,
        nodeId: s.id,
      });
    }
    const failed = (Array.isArray(jobs.data) ? jobs.data : [])
      .filter((j): j is Job => !!j && j.status === "failed")
      .sort((a, b) => (b.updated_at ?? 0) - (a.updated_at ?? 0))
      .slice(0, MAX_JOBS);
    for (const j of failed) {
      const title = `${prettyKind(j.kind)} failed`;
      const target = jobTarget(j, names);
      const human = jobTitle(j, names);
      list.push({
        key: `job:${j.id}`,
        tone: "bad",
        icon: <CircleX className="size-3.5" />,
        title,
        detail: KIND[j.kind] && j.kind !== "agent.update" && !human.toLowerCase().startsWith(j.kind.replace(/\./g, " ")) ? human : undefined,
        target: target.label !== "—" ? target : undefined,
        error: j.last_error?.replace(/^[\w.]+ failed:\s*/, "") || undefined,
        at: j.updated_at ?? null,
        jobId: j.id,
      });
    }
    return list.sort((a, b) => (b.at ?? 0) - (a.at ?? 0));
  }, [jobs.data, servers.data, names]);

  const unread = items.filter((i) => !read.has(i.key)).length;

  const mark = useCallback((keys: string[]) => {
    setRead((prev) => {
      const next = new Set(prev);
      keys.forEach((k) => next.add(k));
      saveRead(next);
      return next;
    });
  }, []);

  const openItem = (i: Item) => {
    mark([i.key]);
    setOpen(false);
    if (i.jobId) setJob(i.jobId);
  };

  return (
    <>
      <Popover.Root open={open} onOpenChange={setOpen}>
        <Popover.Trigger asChild>
          <button
            className="relative grid size-8 place-items-center rounded-md text-muted outline-none hover:bg-panel-2 hover:text-fg focus-visible:ring-2 focus-visible:ring-accent"
            aria-label={`Notifications (${unread} unread)`}
          >
            <Bell className="size-4" />
            {unread > 0 && (
              <span className="absolute -top-0.5 -right-0.5 min-w-4 rounded-full bg-bad px-1 text-center font-mono text-[10px] leading-4 font-semibold text-bg tabular-nums">
                {unread > 9 ? "9+" : unread}
              </span>
            )}
          </button>
        </Popover.Trigger>
        <Popover.Portal>
          <Popover.Content
            align="end"
            sideOffset={6}
            collisionPadding={8}
            className="z-50 flex w-[min(360px,calc(100vw-16px))] flex-col rounded-lg border border-line bg-panel text-[13px] shadow-[var(--shadow-pop)] outline-none"
          >
            <div className="flex items-center gap-2 border-b border-line px-3 py-2">
              <span className="font-semibold">Notifications</span>
              {items.length > 0 && (
                <span className="rounded bg-panel-2 px-1.5 font-mono text-[11px] text-muted tabular-nums" title={`${unread} unread of ${items.length}`}>
                  {unread}/{items.length}
                </span>
              )}
              <button
                className="ml-auto flex items-center gap-1 rounded px-1.5 py-0.5 text-xs text-muted outline-none hover:bg-panel-2 hover:text-fg focus-visible:ring-2 focus-visible:ring-accent disabled:pointer-events-none disabled:opacity-50"
                disabled={unread === 0}
                title={unread === 0 ? "Nothing unread" : undefined}
                onClick={() => mark(items.map((i) => i.key))}
              >
                <CheckCheck className="size-3.5" /> Mark all read
              </button>
              <Link
                to="/jobs"
                className="rounded px-1.5 py-0.5 text-xs text-muted outline-none hover:bg-panel-2 hover:text-fg focus-visible:ring-2 focus-visible:ring-accent"
                onClick={() => setOpen(false)}
              >
                Jobs
              </Link>
            </div>

            {items.length === 0 ? (
              <div className="flex flex-col items-center gap-1 px-3 py-8 text-center">
                <CheckCheck className="size-5 text-ok" />
                <div className="font-medium">You're all caught up</div>
                <div className="text-xs text-muted">Failed jobs and offline nodes show up here.</div>
              </div>
            ) : (
              <ul className="max-h-[min(420px,60vh)] overflow-y-auto py-1">
                {items.map((i) => {
                  const isRead = read.has(i.key);
                  return (
                    <li key={i.key} className="group relative flex gap-2.5 px-3 py-2 hover:bg-panel-2/70 focus-within:bg-panel-2/70">
                      <span className={`mt-0.5 grid size-5 shrink-0 place-items-center rounded ${i.tone === "bad" ? "bg-bad-soft text-bad" : "bg-warn-soft text-warn"}`}>{i.icon}</span>
                      <div className="min-w-0 flex-1">
                        <div className="flex items-baseline gap-2">
                          {i.jobId ? (
                            <button
                              className={`min-w-0 truncate text-left outline-none after:absolute after:inset-0 focus-visible:underline ${isRead ? "text-muted" : "font-medium text-fg"}`}
                              title={i.title}
                              onClick={() => openItem(i)}
                            >
                              {i.title}
                            </button>
                          ) : (
                            <Link
                              to="/nodes/$nodeId"
                              params={{ nodeId: i.nodeId ?? "" }}
                              className={`min-w-0 truncate outline-none after:absolute after:inset-0 focus-visible:underline ${isRead ? "text-muted" : "font-medium text-fg"}`}
                              onClick={() => openItem(i)}
                            >
                              {i.title}
                            </Link>
                          )}
                          <span className="ml-auto shrink-0 font-mono text-[11px] text-faint tabular-nums" title={time(i.at)}>
                            {i.at ? ago(i.at) : "—"}
                          </span>
                          <span aria-label={isRead ? undefined : "Unread"} className={`size-1.5 shrink-0 self-center rounded-full ${isRead ? "bg-transparent" : "bg-accent"}`} />
                        </div>
                        {(i.target || i.detail) && (
                          <div className="flex min-w-0 items-center gap-1 text-xs text-muted">
                            {i.target?.kind === "node" && i.target.id ? (
                              <Link
                                to="/nodes/$nodeId"
                                params={{ nodeId: i.target.id }}
                                className="relative z-10 min-w-0 truncate hover:text-accent"
                                title={i.target.label}
                                onClick={() => mark([i.key])}
                              >
                                {i.target.label}
                              </Link>
                            ) : i.target?.kind === "cluster" && i.target.id ? (
                              <Link
                                to="/clusters/$clusterId"
                                params={{ clusterId: i.target.id }}
                                className="relative z-10 min-w-0 truncate hover:text-accent"
                                title={`Cluster ${i.target.label}`}
                                onClick={() => mark([i.key])}
                              >
                                {i.target.label}
                              </Link>
                            ) : i.target ? (
                              <span className="min-w-0 truncate" title={i.target.label}>
                                {i.target.label}
                              </span>
                            ) : null}
                            {i.detail && (
                              <>
                                {i.target && <span className="text-faint">·</span>}
                                <span className="min-w-0 truncate text-faint" title={i.detail}>
                                  {i.detail}
                                </span>
                              </>
                            )}
                          </div>
                        )}
                        {i.error && (
                          <p className="mt-0.5 line-clamp-2 text-xs break-words text-faint" title={i.error}>
                            <ErrorText text={i.error} />
                          </p>
                        )}
                      </div>
                    </li>
                  );
                })}
              </ul>
            )}

            <div className="flex items-center justify-between border-t border-line px-3 py-1.5 text-xs">
              <span className="text-faint">{jobs.isError ? "Jobs could not be loaded." : `${items.length} item${items.length === 1 ? "" : "s"}`}</span>
              <Link to="/jobs" className="text-accent hover:underline" onClick={() => setOpen(false)}>
                View all jobs →
              </Link>
            </div>
          </Popover.Content>
        </Popover.Portal>
      </Popover.Root>
      <Drawer open={!!job} onOpenChange={(o) => !o && setJob(undefined)} title="Job details" wide>
        {job && <JobDetail id={job} onNavigate={() => setJob(undefined)} />}
      </Drawer>
    </>
  );
}
