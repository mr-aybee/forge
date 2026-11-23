import { Download, Eraser, Moon, Pause, Play, Search, Sun } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import { MAX_LOG_LINES, useLogFollow } from "../api/logFollow";
import { usePrefs } from "../app/prefs";
import type { LogSpec } from "../app/terminals";
import type { PaneStatus } from "./terminal/TerminalPane";
import { LogLine, stripAnsi } from "./logColor";
import { Badge, IconButton } from "./ui";

/**
 * A log viewer tab in the bottom dock. Starts following as soon as it mounts
 * and stops when the tab closes. Key it on the spec.
 */
export default function LogPane({ spec, visible, onStatus }: { spec: LogSpec; visible: boolean; onStatus(s: PaneStatus): void }) {
  const f = useLogFollow(spec, true);
  const { consoleTheme, setConsoleTheme } = usePrefs();
  const [filter, setFilter] = useState("");
  const [stick, setStick] = useState(true);
  const box = useRef<HTMLPreElement>(null);

  const needle = filter.trim().toLowerCase();
  const shown = needle ? f.lines.filter((l) => stripAnsi(l).toLowerCase().includes(needle)) : f.lines;

  // Tab dot: green while following, red on error, grey when paused.
  const state: PaneStatus["state"] = f.error && !f.running ? "error" : f.running ? (f.error ? "connecting" : "ready") : "closed";
  useEffect(() => {
    onStatus({ state, message: f.error });
    // eslint-disable-next-line react-hooks/exhaustive-deps -- onStatus is a fresh closure each render; report on change only
  }, [state, f.error]);

  useEffect(() => {
    if (visible && stick && box.current) box.current.scrollTop = box.current.scrollHeight;
  }, [shown.length, stick, visible]);

  const download = () => {
    const url = URL.createObjectURL(new Blob([f.lines.map(stripAnsi).join("\n")], { type: "text/plain" }));
    const a = document.createElement("a");
    a.href = url;
    a.download = `${spec.name}-${spec.node}-${new Date().toISOString().replace(/[:.]/g, "-")}.log`;
    a.click();
    URL.revokeObjectURL(url);
  };

  return (
    <div className="flex min-w-0 flex-1 flex-col">
      <div className="flex shrink-0 flex-wrap items-center gap-1 border-b border-line px-2 py-1">
        <IconButton label={f.running ? "Pause following" : "Resume following"} onClick={() => (f.running ? f.stop() : f.start())}>
          {f.running ? <Pause className="size-4" /> : <Play className="size-4" />}
        </IconButton>
        {f.running ? (
          <span className="inline-flex items-center gap-1.5 text-xs text-ok">
            <span className="size-2 animate-pulse rounded-full bg-ok" /> following
          </span>
        ) : (
          <span className="text-xs text-faint">paused</span>
        )}
        {f.unitState && <Badge tone={f.unitState === "active" ? "ok" : "warn"}>{f.unitState}</Badge>}
        {f.error && (
          <span className="min-w-0 truncate text-xs text-bad" title={f.error}>
            {f.error}
          </span>
        )}
        <div className="relative ml-auto">
          <Search className="pointer-events-none absolute top-1.5 left-2 size-3.5 text-faint" />
          <input
            aria-label="Filter lines"
            className="h-6 w-44 rounded border border-line bg-panel pl-7 pr-2 text-xs outline-none focus:border-accent"
            placeholder="Filter lines"
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
          />
        </div>
        <span className="font-mono text-[11px] text-faint tabular-nums" title={`Keeps the newest ${MAX_LOG_LINES} lines`}>
          {needle ? `${shown.length}/${f.lines.length}` : f.lines.length || "—"}
        </span>
        <IconButton label="Clear" onClick={f.clear}>
          <Eraser className="size-4" />
        </IconButton>
        <IconButton label="Download" disabled={!f.lines.length} onClick={download}>
          <Download className="size-4" />
        </IconButton>
        <IconButton label={consoleTheme === "dark" ? "Light console" : "Dark console"} onClick={() => setConsoleTheme(consoleTheme === "dark" ? "light" : "dark")}>
          {consoleTheme === "dark" ? <Sun className="size-4" /> : <Moon className="size-4" />}
        </IconButton>
      </div>
      <pre
        ref={box}
        tabIndex={0}
        aria-label={`Logs of ${spec.name} on ${spec.node}`}
        onScroll={(e) => {
          const el = e.currentTarget;
          setStick(el.scrollHeight - el.scrollTop - el.clientHeight < 40);
        }}
        className="log-console min-h-0 flex-1 overflow-auto bg-term p-3 font-mono text-[11.5px] leading-relaxed whitespace-pre-wrap break-all text-term-fg select-text"
      >
        {shown.length
          ? shown.map((l, i) => <LogLine key={i} line={l} />)
          : needle && f.lines.length
            ? "No line matches the filter."
            : f.running
              ? "Waiting for log lines… Following keeps running through restarts, so startup output is captured."
              : f.error
                ? "Following stopped after repeated errors. Press Resume to try again."
                : "Paused. Press Resume to continue from where it stopped."}
      </pre>
    </div>
  );
}
