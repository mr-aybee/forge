import { Copy, Eraser, Pause, Play, Search } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";

import { MAX_LOG_LINES, useLogFollow } from "../../api/logFollow";
import { useToast } from "../../app/toast";
import { COPY_BLOCKED, copyText } from "../../lib/clipboard";
import { LogLine, stripAnsi } from "../../components/logColor";
import LogPane from "../../components/LogPane";
import { Badge, Drawer, IconButton } from "../../components/ui";
import type { TreePlacement, TreeService } from "../../lib/explorerTree";

/** Most nodes followed at once in the "All nodes" view; each one is a running follow loop. */
const MAX_FOLLOWED = 8;
const ALL = "all";

/**
 * Logs of one service: one node (the dock's log pane) or all of them merged,
 * each line tagged with its node. Mount it only while open.
 */
export function ServiceLogDrawer({ service, onClose }: { service: TreeService; onClose: () => void }) {
  const deployed = service.placements.filter((p) => p.instance.status !== "pending");
  const [source, setSource] = useState<string>(deployed.length > 1 ? ALL : (deployed[0]?.serverId ?? ""));
  const one = deployed.find((p) => p.serverId === source);

  return (
    <Drawer open onOpenChange={(o) => !o && onClose()} title={`Logs: ${service.name}`} wide>
      {deployed.length === 0 ? (
        <p className="text-xs text-muted">The service is not deployed to any node yet.</p>
      ) : (
        <div className="flex h-[calc(100vh-7rem)] flex-col gap-2">
          <label className="flex items-center gap-2 text-xs">
            <span className="text-muted">Source</span>
            <select className="rounded-md border border-line bg-panel px-2 py-1" value={source} onChange={(e) => setSource(e.target.value)} aria-label="Log source">
              {deployed.length > 1 && <option value={ALL}>All nodes ({Math.min(deployed.length, MAX_FOLLOWED)})</option>}
              {deployed.map((p) => (
                <option key={p.serverId} value={p.serverId}>
                  {p.nodeName} · {p.instance.role}
                </option>
              ))}
            </select>
          </label>
          <div className="flex min-h-0 flex-1 overflow-hidden rounded-md border border-line">
            {one ? (
              <LogPane key={one.serverId} spec={{ source: "service", appId: service.id, serverId: one.serverId, name: service.name, node: one.nodeName }} visible onStatus={() => undefined} />
            ) : (
              <MergedLogs service={service} placements={deployed.slice(0, MAX_FOLLOWED)} />
            )}
          </div>
        </div>
      )}
    </Drawer>
  );
}

interface Tagged {
  node: string;
  line: string;
  ts: number;
  seq: number;
}

const TS = /(\d{4}-\d\d-\d\d[T ]\d\d:\d\d:\d\d(?:\.\d+)?(?:Z|[+-]\d\d:?\d\d)?)/;

/** Merge per-node lines by their leading timestamp; a line without one sorts right after its predecessor. */
function merge(byNode: Record<string, { node: string; lines: string[] }>): Tagged[] {
  const out: Tagged[] = [];
  let seq = 0;
  for (const { node, lines } of Object.values(byNode)) {
    let last = 0;
    for (const line of lines) {
      const m = TS.exec(stripAnsi(line).slice(0, 64));
      const t = m?.[1] ? Date.parse(m[1]) : NaN;
      if (!Number.isNaN(t)) last = t;
      out.push({ node, line, ts: last, seq: seq++ });
    }
  }
  out.sort((a, b) => a.ts - b.ts || a.seq - b.seq);
  return out.slice(-MAX_LOG_LINES);
}

function MergedLogs({ service, placements }: { service: TreeService; placements: TreePlacement[] }) {
  const [byNode, setByNode] = useState<Record<string, { node: string; lines: string[] }>>({});
  const [errors, setErrors] = useState<Record<string, string | undefined>>({});
  const [paused, setPaused] = useState(false);
  const [filter, setFilter] = useState("");
  // Clear hides what was already received; following carries on.
  const [cut, setCut] = useState<Record<string, number>>({});
  const toast = useToast();
  const box = useRef<HTMLPreElement>(null);
  const [stick, setStick] = useState(true);

  const merged = useMemo(() => merge(Object.fromEntries(Object.entries(byNode).map(([id, v]) => [id, { node: v.node, lines: v.lines.slice(cut[id] ?? 0) }]))), [byNode, cut]);
  const needle = filter.trim().toLowerCase();
  const shown = needle ? merged.filter((t) => `${t.node} ${stripAnsi(t.line)}`.toLowerCase().includes(needle)) : merged;

  useEffect(() => {
    if (stick && box.current) box.current.scrollTop = box.current.scrollHeight;
  }, [shown.length, stick]);

  const failing = Object.entries(errors).filter(([, e]) => e);

  return (
    <div className="flex min-w-0 flex-1 flex-col">
      {placements.map((p) => (
        <NodeFollower
          key={p.serverId}
          service={service}
          p={p}
          paused={paused}
          onLines={(lines) => setByNode((m) => ({ ...m, [p.serverId]: { node: p.nodeName, lines } }))}
          onError={(e) => setErrors((m) => (m[p.serverId] === e ? m : { ...m, [p.serverId]: e }))}
        />
      ))}
      <div className="flex shrink-0 flex-wrap items-center gap-1 border-b border-line px-2 py-1">
        <IconButton label={paused ? "Resume following" : "Pause following"} onClick={() => setPaused(!paused)}>
          {paused ? <Play className="size-4" /> : <Pause className="size-4" />}
        </IconButton>
        {paused ? <span className="text-xs text-faint">paused</span> : <span className="text-xs text-ok">following {placements.length} nodes</span>}
        {failing.map(([id, e]) => (
          <Badge key={id} tone="bad">
            <span title={e}>{placements.find((p) => p.serverId === id)?.nodeName}: error</span>
          </Badge>
        ))}
        <div className="relative ml-auto">
          <Search className="pointer-events-none absolute top-1.5 left-2 size-3.5 text-faint" />
          <input
            aria-label="Filter lines"
            className="h-6 w-44 rounded border border-line bg-panel pr-2 pl-7 text-xs outline-none focus:border-accent"
            placeholder="Filter lines or node"
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
          />
        </div>
        <span className="font-mono text-[11px] text-faint tabular-nums" title={`Keeps the newest ${MAX_LOG_LINES} lines`}>
          {needle ? `${shown.length}/${merged.length}` : merged.length || "—"}
        </span>
        <IconButton label="Copy" disabled={!shown.length} onClick={() => void copyText(shown.map((t) => `[${t.node}] ${stripAnsi(t.line)}`).join("\n")).then((ok) => !ok && toast(COPY_BLOCKED, "error"))}>
          <Copy className="size-4" />
        </IconButton>
        <IconButton
          label="Clear"
          onClick={() => {
            setCut(Object.fromEntries(Object.entries(byNode).map(([id, v]) => [id, v.lines.length])));
          }}
        >
          <Eraser className="size-4" />
        </IconButton>
      </div>
      <pre
        ref={box}
        tabIndex={0}
        aria-label={`Logs of ${service.name} on all nodes`}
        onScroll={(e) => {
          const el = e.currentTarget;
          setStick(el.scrollHeight - el.scrollTop - el.clientHeight < 40);
        }}
        className="log-console min-h-0 flex-1 overflow-auto bg-term p-3 font-mono text-[11.5px] leading-relaxed break-all whitespace-pre-wrap text-term-fg select-text"
      >
        {shown.length
          ? shown.map((t) => (
              <div key={t.seq} className="flex gap-2">
                <span className="shrink-0 text-accent">[{t.node}]</span>
                <LogLine line={t.line} />
              </div>
            ))
          : needle && merged.length
            ? "No line matches the filter."
            : "Waiting for log lines…"}
      </pre>
    </div>
  );
}

/** Follows one node and reports its lines to the merged view; renders nothing. */
function NodeFollower({ service, p, paused, onLines, onError }: { service: TreeService; p: TreePlacement; paused: boolean; onLines: (l: string[]) => void; onError: (e: string | undefined) => void }) {
  const f = useLogFollow({ source: "service", appId: service.id, serverId: p.serverId, name: service.name, node: p.nodeName }, true);
  const { start, stop } = f;
  // Bridges a stream (not server state) up to the merged view.
  useEffect(() => onLines(f.lines), [f.lines]); // eslint-disable-line react-hooks/exhaustive-deps -- report on new lines only
  useEffect(() => onError(f.error), [f.error]); // eslint-disable-line react-hooks/exhaustive-deps -- report on change only
  useEffect(() => {
    if (paused) stop();
    else if (!f.running) start();
  }, [paused]); // eslint-disable-line react-hooks/exhaustive-deps -- react to the pause toggle only
  return null;
}
