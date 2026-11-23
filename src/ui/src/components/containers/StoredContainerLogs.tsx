import { Link } from "@tanstack/react-router";
import { ExternalLink, Loader2 } from "lucide-react";
import { useMemo, useState } from "react";

import { analyticsUnavailable } from "../../api/analytics";
import { type LogFilter, useLogEvents } from "../../api/logs";
import { Button, CopyButton, Empty, ErrorNote, Input, Skeleton, Switch, time } from "../ui";
import { ContainerLogs } from "./ContainerLogs";

const stamp = (t: number) => new Date(t).toLocaleString([], { month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit", second: "2-digit" });

/**
 * A container's logs: the stored history from the analytics database (filter
 * `node` + `container`), or, with **Live**, the node's own output followed
 * as it is written. Stored logs survive container restarts and removal.
 */
export function StoredContainerLogs({ server, name, containerId, height = "h-[55vh]" }: { server: string; name: string; containerId: string; height?: string }) {
  const [live, setLive] = useState(false);
  const [find, setFind] = useState("");
  const filter: LogFilter = useMemo(() => ({ nodes: [server], container: name, source: "container" }), [server, name]);
  const events = useLogEvents(filter);
  const off = analyticsUnavailable(events.error);
  // Oldest first, like a terminal.
  const lines = useMemo(() => (events.data?.pages ?? []).flatMap((p) => p.events).reverse(), [events.data]);
  const needle = find.trim().toLowerCase();
  const shown = needle ? lines.filter((e) => e.message.toLowerCase().includes(needle)) : lines;

  return (
    <div className="min-w-0">
      <div className="mb-2 flex min-w-0 flex-wrap items-center gap-2">
        <Switch checked={live} onChange={setLive} label="Live" title="Follow the output on the node as it is written" />
        {!live && (
          <>
            <Input className="h-7 max-w-xs py-0 text-xs" placeholder="Filter lines" aria-label="Filter stored lines" value={find} onChange={(e) => setFind(e.target.value)} />
            <Link to="/logs" search={{ node: server, container: name, source: "container" }} className="inline-flex items-center gap-1 text-xs text-accent hover:underline">
              <ExternalLink className="size-3.5" /> Open in Logs
            </Link>
            <span className="ml-auto font-mono text-[11px] text-faint tabular-nums">{lines.length ? `${shown.length} lines` : "—"}</span>
            {shown.length > 0 && <CopyButton text={shown.map((e) => `${new Date(e.ts).toISOString()} ${e.stream ?? ""} ${e.message}`).join("\n")} label="Copy" />}
          </>
        )}
      </div>
      {live ? (
        <ContainerLogs server={server} containerId={containerId} height={height} />
      ) : events.isPending ? (
        <Skeleton rows={8} />
      ) : off ? (
        <div className="rounded-lg border border-line">
          <Empty title="Stored logs are not available" action={<Button onClick={() => setLive(true)}>Follow live instead</Button>}>
            {off}
          </Empty>
        </div>
      ) : events.isError ? (
        <ErrorNote error={events.error} />
      ) : lines.length === 0 ? (
        <div className="rounded-lg border border-line">
          <Empty title="No stored logs" action={<Button onClick={() => setLive(true)}>Follow live</Button>}>
            Nothing collected for <span className="font-mono">{name}</span> yet. Container collection may be off on this node, or the container is excluded (node <b>Details</b> → Container telemetry).
          </Empty>
        </div>
      ) : (
        <div className={`${height} overflow-auto rounded-lg bg-term p-2 font-mono text-[11.5px] leading-relaxed text-term-fg`}>
          {events.hasNextPage && (
            <button type="button" className="mb-1 text-[11px] text-faint underline hover:text-term-fg" disabled={events.isFetchingNextPage} onClick={() => void events.fetchNextPage()}>
              {events.isFetchingNextPage ? <Loader2 className="inline size-3 animate-spin" /> : null} Load older lines
            </button>
          )}
          {events.isFetchNextPageError && <p className="text-[11px] text-bad">Could not load older lines.</p>}
          {shown.map((e) => (
            <div key={e.id} className="flex min-w-0 gap-2 select-text">
              <time className="shrink-0 opacity-60" title={time(e.ts)}>
                {stamp(e.ts)}
              </time>
              {e.stream === "stderr" && <span className="shrink-0 text-bad" title="stderr">err</span>}
              <span className="min-w-0 break-all whitespace-pre-wrap">{e.message}</span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
