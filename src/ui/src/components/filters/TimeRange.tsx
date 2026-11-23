import * as Popover from "@radix-ui/react-popover";
import { CalendarRange } from "lucide-react";
import { useState } from "react";

import { Button, Input } from "../ui";

/** A relative window such as "24 h". */
export interface RangePreset {
  key: string;
  label: string;
}

/** The chosen window: a preset key, `"all"`, or a custom `since`/`until` (epoch ms, either may be open). */
export interface TimeRangeValue {
  range?: string;
  since?: number;
  until?: number;
}

/** `datetime-local` value ⇄ epoch ms, in local time. */
function toLocalInput(ms?: number): string {
  if (!ms) return "";
  const d = new Date(ms);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}`;
}
const fromLocalInput = (v: string) => (v ? new Date(v).getTime() || undefined : undefined);

const fmt = new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit", hour12: false });
const fmtYear = new Intl.DateTimeFormat(undefined, { year: "numeric", month: "short", day: "numeric", hour: "2-digit", minute: "2-digit", hour12: false });
/** The year only when it is not this year. */
const when = (ms: number) => (new Date(ms).getFullYear() === new Date().getFullYear() ? fmt : fmtYear).format(ms);

/** "Nov 22, 21:09 → now" for a custom window. */
export function rangeLabel(since?: number, until?: number): string {
  return `${since ? when(since) : "start"} → ${until ? when(until) : "now"}`;
}

const seg =
  "press inline-flex h-7 shrink-0 items-center gap-1.5 border-l border-line px-2.5 text-xs whitespace-nowrap transition first:border-l-0 focus-visible:relative focus-visible:outline-2 focus-visible:outline-accent";

/**
 * Presets, All and Custom as one joined control. Custom opens a popover
 * with From and To; once applied, the segment shows the chosen window.
 */
export function TimeRange({ presets, value, onChange }: { presets: readonly RangePreset[]; value: TimeRangeValue; onChange: (v: TimeRangeValue) => void }) {
  const [open, setOpen] = useState(false);
  const custom = !value.range && !!(value.since || value.until);
  const on = (key: string) => value.range === key;
  return (
    <div role="group" aria-label="Time range" className="inline-flex max-w-full min-w-0 overflow-hidden rounded-full border border-line bg-panel">
      {[...presets, { key: "all", label: "All" }].map((p) => (
        <button
          key={p.key}
          type="button"
          aria-pressed={on(p.key)}
          onClick={() => onChange({ range: p.key })}
          className={`${seg} ${on(p.key) ? "bg-accent-soft text-fg" : "text-muted hover:bg-panel-2 hover:text-fg"}`}
        >
          {p.label}
        </button>
      ))}
      <Popover.Root open={open} onOpenChange={setOpen}>
        <Popover.Trigger asChild>
          <button
            type="button"
            aria-pressed={custom}
            title={custom ? "Change the custom window" : "Pick a custom window"}
            className={`${seg} min-w-0 ${custom ? "bg-accent-soft text-fg" : "text-muted hover:bg-panel-2 hover:text-fg"}`}
          >
            <CalendarRange className="size-3.5 shrink-0" aria-hidden />
            <span className={`min-w-0 truncate ${custom ? "font-mono text-[11px] tabular-nums" : ""}`}>{custom ? rangeLabel(value.since, value.until) : "Custom"}</span>
          </button>
        </Popover.Trigger>
        <Popover.Portal>
          <Popover.Content align="end" sideOffset={4} collisionPadding={8} className="z-50 w-72 rounded-lg border border-line bg-panel p-3 shadow-pop">
            {open && <CustomForm value={value} onApply={(v) => (onChange(v), setOpen(false))} onCancel={() => setOpen(false)} />}
          </Popover.Content>
        </Popover.Portal>
      </Popover.Root>
    </div>
  );
}

/** Mounted while the popover is open, so it starts from the applied window every time. */
function CustomForm({ value, onApply, onCancel }: { value: TimeRangeValue; onApply: (v: TimeRangeValue) => void; onCancel: () => void }) {
  const [form, setForm] = useState({ from: toLocalInput(value.range ? undefined : value.since), to: toLocalInput(value.range ? undefined : value.until) });
  const since = fromLocalInput(form.from);
  const until = fromLocalInput(form.to);
  const invalid = since && until && since >= until ? "From must be before To." : !since && !until ? "Set From, To or both." : undefined;
  return (
    <form
      className="space-y-2 text-xs"
      onSubmit={(e) => {
        e.preventDefault();
        if (!invalid) onApply({ since, until });
      }}
    >
      <label className="grid grid-cols-[3rem_minmax(0,1fr)] items-center gap-2 text-muted">
        From
        <Input type="datetime-local" className="h-7 py-0 text-xs" value={form.from} onChange={(e) => setForm({ ...form, from: e.target.value })} autoFocus />
      </label>
      <label className="grid grid-cols-[3rem_minmax(0,1fr)] items-center gap-2 text-muted">
        To
        <Input type="datetime-local" className="h-7 py-0 text-xs" value={form.to} onChange={(e) => setForm({ ...form, to: e.target.value })} />
      </label>
      <p className="text-[11px] text-faint">Leave To empty for "now". To is exclusive.</p>
      {invalid && (form.from || form.to) && <p className="text-[11px] text-bad">{invalid}</p>}
      <div className="flex justify-end gap-2 pt-1">
        <Button type="button" variant="ghost" onClick={onCancel}>
          Cancel
        </Button>
        <Button type="submit" variant="primary" disabled={!!invalid}>
          Apply
        </Button>
      </div>
    </form>
  );
}
