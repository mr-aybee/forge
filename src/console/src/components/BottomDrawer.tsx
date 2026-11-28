import { ChevronDown, Radio, Trash2 } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import type { LiveEvent } from "../api/types";

export interface StreamLine {
  at: number;
  text: string;
  tone: "info" | "ok" | "warn" | "bad";
}

/** Formats a live event as one console line. */
export function eventLine(ev: LiveEvent): StreamLine {
  const at = Date.now();
  if (ev.type === "job") {
    const tone = ev.status === "failed" ? "bad" : ev.status === "succeeded" ? "ok" : "info";
    return { at, tone, text: `job ${ev.job_id.slice(0, 8)} ${ev.kind} → ${ev.status}${ev.step ? ` · step ${ev.step}` : ""}` };
  }
  const tone = ev.status === "online" ? "ok" : ev.status === "offline" ? "warn" : ev.status === "revoked" ? "bad" : "info";
  return { at, tone, text: `node ${ev.server_id.slice(0, 8)} → ${ev.status}` };
}

const toneClass = { info: "text-accent", ok: "text-ok", warn: "text-warn", bad: "text-bad" };

/**
 * Bottom console drawer. Phase 1 streams live job and node events; browser
 * SSH/RDP sessions (Phase 2) will open as additional tabs here.
 */
export function BottomDrawer({ open, onClose, lines, onClear }: { open: boolean; onClose: () => void; lines: StreamLine[]; onClear: () => void }) {
  const end = useRef<HTMLDivElement>(null);
  const [follow, setFollow] = useState(true);
  useEffect(() => {
    if (follow) end.current?.scrollIntoView({ block: "end" });
  }, [lines, follow]);
  if (!open) return null;
  return (
    <section aria-label="Console" className="flex h-64 shrink-0 flex-col border-t border-line bg-panel">
      <div className="flex items-center gap-1 border-b border-line px-3">
        <button className="flex items-center gap-1.5 border-b-2 border-accent px-2 py-2 text-xs font-medium text-accent">
          <Radio className="size-3.5" /> Unified event stream
        </button>
        <span
          className="ml-1 cursor-help px-2 py-2 text-xs text-faint"
          title="Browser SSH, RDP and database consoles arrive in Phase 2 and will open as tabs here."
        >
          SSH · RDP · SQL (Phase 2)
        </span>
        <div className="ml-auto flex items-center gap-2 text-xs text-muted">
          <label className="flex items-center gap-1">
            <input type="checkbox" checked={follow} onChange={(e) => setFollow(e.target.checked)} /> Follow
          </label>
          <button onClick={onClear} className="flex items-center gap-1 rounded px-1.5 py-1 hover:bg-panel-2" aria-label="Clear">
            <Trash2 className="size-3.5" /> Clear
          </button>
          <button onClick={onClose} className="rounded p-1 hover:bg-panel-2" aria-label="Close console">
            <ChevronDown className="size-4" />
          </button>
        </div>
      </div>
      <div className="flex-1 overflow-y-auto bg-panel-2/60 p-3 font-mono text-xs leading-relaxed">
        {lines.length === 0 && <div className="text-faint">Waiting for events… jobs and node status changes appear here live.</div>}
        {lines.map((l, i) => (
          <div key={i}>
            <span className="text-faint">{new Date(l.at).toLocaleTimeString()} </span>
            <span className={toneClass[l.tone]}>{l.text}</span>
          </div>
        ))}
        <div ref={end} />
      </div>
    </section>
  );
}
