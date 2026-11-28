import { Loader2, Play, Search, X } from "lucide-react";
import { useState } from "react";

/**
 * The full-width search of a feed page with a joined Run | Live control.
 * Enter or Run applies the draft (and ends Live); Live keeps polling. Key it
 * on the applied query so a URL change resets the draft.
 */
export function QueryInput({
  value,
  onRun,
  loading,
  live,
  liveDisabled,
  onLive,
  label,
  placeholder,
  noun = "events",
  liveEvery = "2 seconds",
}: {
  value: string;
  onRun: (q: string) => void;
  loading: boolean;
  live: boolean;
  /** Why Live cannot start; shown as its tooltip. */
  liveDisabled?: string;
  onLive: (on: boolean) => void;
  /** Accessible name of the input, such as "Search events". */
  label: string;
  placeholder: string;
  noun?: string;
  liveEvery?: string;
}) {
  const [draft, setDraft] = useState(value);
  const dirty = draft.trim() !== value.trim();
  return (
    <form
      className="flex w-full items-center gap-2"
      onSubmit={(ev) => {
        ev.preventDefault();
        onRun(draft);
      }}
    >
      <div className="relative flex min-w-0 flex-1 items-center">
        <Search className="pointer-events-none absolute left-2.5 size-3.5 text-faint" />
        <input
          aria-label={label}
          className={`h-7 w-full rounded-full border bg-panel py-0 pr-16 pl-8 font-mono text-xs outline-none focus:border-accent ${dirty ? "border-accent/60" : "border-line"}`}
          placeholder={placeholder}
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
        />
        <div className="absolute right-1.5 flex items-center gap-1">
          {dirty && <kbd className="rounded border border-line px-1 font-mono text-[10px] text-faint">↵</kbd>}
          {(draft || value) && (
            <button
              type="button"
              aria-label="Clear search"
              title="Clear search"
              className="grid size-5 place-items-center rounded text-faint hover:bg-panel-2 hover:text-fg"
              onClick={() => {
                setDraft("");
                onRun("");
              }}
            >
              <X className="size-3.5" />
            </button>
          )}
        </div>
      </div>
      {/* Run and Live are one control: Run loads once (and ends Live), Live keeps polling. */}
      <div role="group" aria-label={`Load ${noun}`} className="inline-flex h-7 shrink-0 overflow-hidden rounded-full border border-line bg-panel">
        <button
          type="submit"
          title={dirty ? "Run this query (stops Live)" : `Load the ${noun} again (stops Live)`}
          className="press inline-flex items-center gap-1.5 px-3 text-xs font-medium text-fg hover:bg-panel-2 focus-visible:relative focus-visible:outline-2 focus-visible:outline-accent disabled:opacity-60"
          disabled={loading && !live}
        >
          {loading && !live ? <Loader2 className="size-3.5 animate-spin" aria-hidden /> : <Play className="size-3.5 text-accent" aria-hidden />}
          Run
        </button>
        <button
          type="button"
          aria-pressed={live}
          title={liveDisabled ?? (live ? "Stop polling" : `Poll for new ${noun} every ${liveEvery}`)}
          disabled={!!liveDisabled}
          onClick={() => onLive(!live)}
          className={`press inline-flex items-center gap-1.5 border-l border-line px-3 text-xs font-medium focus-visible:relative focus-visible:outline-2 focus-visible:outline-accent disabled:cursor-not-allowed disabled:opacity-50 ${
            live ? "bg-ok/15 text-ok" : "text-muted hover:bg-panel-2 hover:text-fg"
          }`}
        >
          <span aria-hidden className={`size-1.5 rounded-full ${live ? "animate-pulse bg-ok" : "bg-faint"}`} />
          Live
        </button>
      </div>
    </form>
  );
}
