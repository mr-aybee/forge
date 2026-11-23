import { ChevronDown, ChevronRight } from "lucide-react";
import { type ReactNode, useId, useState } from "react";

import { ErrorNote, time } from "../ui";

const nf = new Intl.NumberFormat();

/** One stacked series; `bar` is a background token class such as `bg-bad`. */
export interface HistogramSeries {
  key: string;
  label: string;
  bar: string;
}

export interface HistogramBucket {
  t: number;
  counts: Partial<Record<string, number>>;
}

/** Whether a chart is shown; remembered per browser (a convenience, so storage errors fall back to shown). */
function useChartShown(storageKey: string): [boolean, (v: boolean) => void] {
  const [shown, setShown] = useState(() => {
    try {
      return localStorage.getItem(storageKey) !== "hidden";
    } catch {
      return true;
    }
  });
  const set = (v: boolean) => {
    setShown(v);
    try {
      localStorage.setItem(storageKey, v ? "shown" : "hidden");
    } catch {
      /* storage blocked: keep the choice for this visit only */
    }
  };
  return [shown, set];
}

/**
 * Counts per time bucket as stacked bars, under a "Chart" toggle that hides
 * it (remembered in `storageKey`). Series stack bottom-up in the given order.
 */
export function StackedHistogram({
  storageKey,
  series,
  buckets,
  from,
  to,
  loading,
  error,
  noun = "events",
  empty,
  total: totalOverride,
}: {
  storageKey: string;
  series: readonly HistogramSeries[];
  buckets: HistogramBucket[];
  from?: number;
  to?: number;
  loading: boolean;
  error?: unknown;
  noun?: string;
  /** Replaces the bars with a note, for example when the server has no histogram. */
  empty?: ReactNode;
  /** The count in the header; defaults to the sum of the buckets. */
  total?: number;
}) {
  const [shown, setShown] = useChartShown(storageKey);
  const id = useId();
  if (error) return <ErrorNote error={error} />;
  const totals = buckets.map((b) => series.reduce((s, x) => s + (b.counts[x.key] ?? 0), 0));
  const max = Math.max(1, ...totals);
  const total = totalOverride ?? totals.reduce((a, b) => a + b, 0);
  return (
    <div className={`mb-3 rounded-lg border border-line bg-panel px-3 ${shown ? "pt-2 pb-1.5" : "py-1.5"}`}>
      <div className={`flex min-w-0 items-center gap-2 text-[11px] text-faint ${shown ? "mb-1" : ""}`}>
        <button
          type="button"
          aria-expanded={shown}
          aria-controls={id}
          title={shown ? "Hide the chart" : "Show the chart"}
          onClick={() => setShown(!shown)}
          className="press -ml-1 inline-flex shrink-0 items-center gap-1 rounded px-1 hover:text-fg focus-visible:outline-2 focus-visible:outline-accent"
        >
          {shown ? <ChevronDown className="size-3.5" aria-hidden /> : <ChevronRight className="size-3.5" aria-hidden />}
          Chart
        </button>
        <span className="min-w-0 truncate">
          {from ? time(from) : "—"} → {to ? time(to) : "now"}
        </span>
        <span className="ml-auto shrink-0 font-mono tabular-nums">{loading ? "…" : `${nf.format(total)} ${noun}`}</span>
      </div>
      {shown && (
        <div id={id} className="flex h-14 items-end gap-px" aria-label={`${noun} per interval by ${series.map((s) => s.label).join(", ")}`} role="img">
          {empty ? (
            <div className="w-full self-center text-center text-xs text-faint">{empty}</div>
          ) : loading ? (
            Array.from({ length: 48 }, (_, i) => <div key={i} className="h-1/3 flex-1 animate-pulse rounded-sm bg-panel-2" />)
          ) : buckets.length === 0 ? (
            <div className="w-full self-center text-center text-xs text-faint">No {noun} in this window</div>
          ) : (
            buckets.map((b, i) => (
              <div
                key={b.t}
                className="flex h-full min-w-0 flex-1 flex-col-reverse"
                title={`${time(b.t)}\n${series.filter((s) => b.counts[s.key]).map((s) => `${s.label}: ${nf.format(b.counts[s.key] ?? 0)}`).join("\n") || `No ${noun}`}`}
              >
                {series.map((s) => {
                  const n = b.counts[s.key] ?? 0;
                  return n ? <div key={s.key} className={`${s.bar} w-full first:rounded-b-sm last:rounded-t-sm`} style={{ height: `${(n / max) * 100}%` }} /> : null;
                })}
                {totals[i] === 0 && <div className="h-px w-full bg-line" />}
              </div>
            ))
          )}
        </div>
      )}
    </div>
  );
}
