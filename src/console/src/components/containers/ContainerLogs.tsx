import { useEffect, useRef, useState } from "react";

import { containerCall } from "../../api/hooks";
import { errorText } from "../../app/toast";
import { CopyButton, Input, Select } from "../ui";

/** Docker's RFC 3339 timestamp at the start of a line, as decimal Unix seconds (nanosecond precision). */
function lineTime(line: string): string | undefined {
  const m = /^(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2})(?:\.(\d{1,9}))?Z\s/.exec(line);
  if (!m) return undefined;
  const secs = Math.floor(Date.parse(`${m[1]}Z`) / 1000);
  return `${secs}.${(m[2] ?? "").padEnd(9, "0")}`;
}

/**
 * Container logs with filter, tail and follow. Follow keeps polling for new
 * lines only (after the last timestamp seen), so it keeps going across stop,
 * start and restart. `height` is the Tailwind height class of the output box.
 */
export function ContainerLogs({ server, containerId, height = "h-[55vh]" }: { server: string; containerId: string; height?: string }) {
  const [tail, setTail] = useState(500);
  const [follow, setFollow] = useState(true);
  const [filter, setFilter] = useState("");
  const [lines, setLines] = useState<string[]>([]);
  const [error, setError] = useState<string>();
  const [loading, setLoading] = useState(true);
  const box = useRef<HTMLPreElement>(null);

  useEffect(() => {
    let stop = false;
    let cursor: string | undefined;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const MAX = 20_000;
    setLoading(true);
    const tick = async (first: boolean) => {
      try {
        const r = await containerCall<{ logs?: string | null }>(server, { op: "logs", id: containerId, tail: first ? tail : 5000, since: first ? undefined : cursor });
        if (stop) return;
        if (r.result) {
          let got = (r.result.logs ?? "").split("\n").filter((l) => l.length > 0);
          // `since` is inclusive: drop lines at or before the cursor.
          if (!first && cursor) got = got.filter((l) => (lineTime(l) ?? "") > (cursor as string));
          const last = [...got].reverse().map(lineTime).find(Boolean);
          if (last) cursor = last;
          setLines((prev) => (first ? got : [...prev, ...got]).slice(-MAX));
          setError(undefined);
        } else setError(r.error ?? "The node returned no logs");
      } catch (e) {
        if (!stop) setError(errorText(e));
      }
      if (!stop) setLoading(false);
      if (!stop && follow) timer = setTimeout(() => void tick(false), 2000);
    };
    void tick(true);
    return () => {
      stop = true;
      if (timer) clearTimeout(timer);
    };
  }, [server, containerId, tail, follow]);

  useEffect(() => {
    if (follow && box.current) box.current.scrollTop = box.current.scrollHeight;
  }, [lines, follow]);

  const shown = filter ? lines.filter((l) => l.toLowerCase().includes(filter.toLowerCase())) : lines;
  const text = shown.join("\n");
  return (
    <div className="min-w-0">
      <div className="mb-2 flex flex-wrap items-center gap-2">
        <Input className="max-w-xs" placeholder="Filter lines" aria-label="Filter lines" value={filter} onChange={(e) => setFilter(e.target.value)} />
        <Select className="w-auto" value={tail} onChange={(e) => setTail(Number(e.target.value))} aria-label="Lines">
          {[100, 500, 2000, 5000].map((n) => (
            <option key={n} value={n}>
              last {n} lines
            </option>
          ))}
        </Select>
        <label className="flex items-center gap-1.5 text-xs">
          <input type="checkbox" checked={follow} onChange={(e) => setFollow(e.target.checked)} /> Follow live
        </label>
        {follow && (
          <span className="inline-flex items-center gap-1.5 text-[11px] text-ok">
            <span className="size-1.5 animate-pulse rounded-full bg-current" /> following (keeps going across restarts)
          </span>
        )}
        <span className="flex-1" />
        {filter && (
          <span className="font-mono text-[11px] text-faint tabular-nums">
            {shown.length}/{lines.length}
          </span>
        )}
        {text && <CopyButton text={text} label="Copy" />}
      </div>
      {error && (
        <p role="alert" className="mb-2 rounded-md bg-bad-soft px-3 py-1.5 text-xs text-bad">
          {error}
        </p>
      )}
      <pre ref={box} className={`log-console ${height} overflow-auto rounded-lg bg-term p-3 font-mono text-[11.5px] leading-relaxed whitespace-pre text-term-fg select-text`}>
        {text || (loading ? "Reading logs from the node…" : "No output.")}
      </pre>
    </div>
  );
}
