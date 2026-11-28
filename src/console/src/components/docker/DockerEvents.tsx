import { Activity, RefreshCw, Search } from "lucide-react";
import { useMemo, useState } from "react";

import { EVENT_WINDOWS, type EventWindow, useDockerEvents } from "../../api/dockerResources";
import { Badge, Empty, IconButton, Segmented, Skeleton, Table, Td, ago, stamp, time } from "../ui";
import { DockerProblem, Toolbar } from "./shared";

const WINDOW_LABEL: Record<EventWindow, string> = { 300: "5m", 3600: "1h", 86400: "24h" };
const tone = (a: string) =>
  /die|kill|oom|destroy|delete|remove|stop|unhealthy/.test(a) ? "bad" : /start|create|pull|connect|healthy|mount/.test(a) ? "ok" : "muted";

export function DockerEvents({ server }: { server: string }) {
  const [since, setSince] = useState<EventWindow>(300);
  const [search, setSearch] = useState("");
  const q = useDockerEvents(server, since);
  const all = useMemo(() => q.data ?? [], [q.data]);
  const rows = useMemo(() => {
    const s = search.trim().toLowerCase();
    return s ? all.filter((e) => `${e.type} ${e.action} ${e.name ?? ""} ${e.id ?? ""}`.toLowerCase().includes(s)) : all;
  }, [all, search]);

  return (
    <div>
      <Toolbar>
        <label className="flex min-w-0 flex-1 items-center gap-2">
          <Search className="size-3.5 shrink-0 text-faint" />
          <input data-bare className="min-w-0 flex-1 bg-transparent text-[13px] outline-none" placeholder="Filter by type, action or name" aria-label="Filter events" value={search} onChange={(e) => setSearch(e.target.value)} />
        </label>
        <Segmented label="Events since" value={String(since)} onChange={(v) => setSince(Number(v) as EventWindow)} options={EVENT_WINDOWS.map((w) => ({ id: String(w), label: WINDOW_LABEL[w] }))} />
        <IconButton label="Refresh events" onClick={() => void q.refetch()} disabled={q.isFetching}>
          <RefreshCw className={`size-4 ${q.isFetching ? "animate-spin" : ""}`} />
        </IconButton>
      </Toolbar>
      {q.error ? (
        <DockerProblem error={q.error} server={server} />
      ) : q.isLoading ? (
        <Skeleton rows={5} />
      ) : rows.length === 0 ? (
        <Empty icon={<Activity className="size-5" />} title={all.length ? "No event matches" : `No events in the last ${WINDOW_LABEL[since]}`}>
          {all.length ? "Change the search." : "Pick a longer window; the list refreshes every 15 seconds."}
        </Empty>
      ) : (
        <>
          <Table head={["Time", "Type", "Action", "Object"]}>
            {rows.map((e, i) => (
              // Events have no id of their own; time + position is stable for a read-only list.
              <tr key={`${e.time ?? 0}-${i}`} className="hover:bg-panel-2/60">
                <Td className="font-mono text-[11px] whitespace-nowrap text-muted tabular-nums" title={`${time(e.time)} · ${ago(e.time)}`}>
                  {stamp(e.time)}
                </Td>
                <Td className="text-xs text-muted">{e.type}</Td>
                <Td>
                  <Badge tone={tone(e.action)}>{e.action}</Badge>
                </Td>
                <Td className="max-w-[360px] truncate font-mono text-[11px] select-text" title={[e.name, e.id].filter(Boolean).join("\n")}>
                  {e.name ?? "—"}
                  {e.id && <span className="ml-2 text-faint">{e.id.replace(/^sha256:/, "").slice(0, 12)}</span>}
                </Td>
              </tr>
            ))}
          </Table>
          <p className="border-t border-line px-4 py-1.5 font-mono text-[11px] text-faint tabular-nums">
            {rows.length} of {all.length} events · newest first · at most 1000
          </p>
        </>
      )}
    </div>
  );
}
