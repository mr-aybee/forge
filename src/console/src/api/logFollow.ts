import { useCallback, useEffect, useRef, useState } from "react";

import type { LogSpec } from "../app/terminals";
import { errorText } from "../app/toast";
import { get, post } from "./client";

interface Segment {
  job_id: string;
  status: string;
}
interface JobView {
  status: string;
  state?: { output?: { cursor?: string; lines?: string; state?: string } };
  last_error?: string | null;
}

export const MAX_LOG_LINES = 5000;
const DONE = ["succeeded", "failed", "cancelled"];

export interface LogFollow {
  lines: string[];
  running: boolean;
  error?: string;
  /** Unit state reported by the node (`active`, `failed`, …), when known. */
  unitState?: string;
  start(): void;
  stop(): void;
  clear(): void;
}

/**
 * Follows one log source until stopped or unmounted. For a service, each
 * segment is one `follow` action (about 50 s on the node); its lines stream
 * through the job's live output, then the next segment continues from the
 * returned cursor. The node reads by position (journal cursor, file offset,
 * Docker timestamp), so following survives stop / start / restart and a
 * pause resumes where it left off.
 *
 * This is a stream, not cacheable server state, so it lives in component
 * state rather than TanStack Query; the owner keys the component on the spec.
 */
export function useLogFollow(spec: LogSpec, autoStart = true): LogFollow {
  const [lines, setLines] = useState<string[]>([]);
  const [live, setLive] = useState<string[]>([]);
  const [running, setRunning] = useState(false);
  const [error, setError] = useState<string>();
  const [unitState, setUnitState] = useState<string>();
  // Each loop owns a generation; bumping it stops the loop (pause, unmount).
  const gen = useRef(0);
  const cursor = useRef("");
  const { appId, serverId } = spec;

  const start = useCallback(() => {
    gen.current += 1;
    const mine = gen.current;
    const alive = () => gen.current === mine;
    const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
    setRunning(true);
    setError(undefined);
    void (async () => {
      let failures = 0;
      while (alive()) {
        try {
          const seg = await post<Segment>(`/api/v1/services/${encodeURIComponent(appId)}/instances/${encodeURIComponent(serverId)}/actions`, { action: "follow", cursor: cursor.current });
          let streamed: string[] = [];
          for (;;) {
            await sleep(1000);
            if (!alive()) break;
            const l = await get<{ output?: string | null; status?: string }>(`/api/v1/jobs/${seg.job_id}/live`);
            if (!alive()) break;
            if (l.output) {
              streamed = l.output.split("\n").filter(Boolean);
              setLive(streamed);
            }
            if (DONE.includes(l.status ?? "")) break;
          }
          if (!alive()) {
            // Keep what the paused segment already showed.
            const keep = streamed;
            if (keep.length) setLines((prev) => [...prev, ...keep].slice(-MAX_LOG_LINES));
            setLive([]);
            break;
          }
          const j = await get<JobView>(`/api/v1/jobs/${seg.job_id}`);
          if (!alive()) break;
          const out = j.state?.output;
          if (j.status === "succeeded" && out) {
            // The final result is authoritative: it includes lines the live view missed.
            const segLines = (out.lines ?? "").split("\n").filter(Boolean);
            setLines((prev) => [...prev, ...segLines].slice(-MAX_LOG_LINES));
            setLive([]);
            cursor.current = out.cursor ?? cursor.current;
            if (out.state) setUnitState(out.state);
            failures = 0;
            setError(undefined);
          } else if (j.status === "failed") {
            throw new Error(j.last_error ?? "following failed");
          } else if (streamed.length) {
            const keep = streamed;
            setLines((prev) => [...prev, ...keep].slice(-MAX_LOG_LINES));
            setLive([]);
          }
        } catch (e) {
          if (!alive()) break;
          failures += 1;
          setError(errorText(e));
          if (failures >= 5) break;
          await sleep(Math.min(2000 * failures, 10_000));
        }
      }
      if (alive()) setRunning(false);
    })();
  }, [appId, serverId]);

  const stop = useCallback(() => {
    gen.current += 1;
    setRunning(false);
  }, []);

  const clear = useCallback(() => {
    setLines([]);
    setLive([]);
  }, []);

  // Start once on mount when asked; stop the loop on unmount (tab closed).
  useEffect(() => {
    if (autoStart) start();
    return () => {
      gen.current += 1;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps -- runs once; the owner keys the component on the spec
  }, []);

  return { lines: live.length ? [...lines, ...live] : lines, running, error, unitState, start, stop, clear };
}
