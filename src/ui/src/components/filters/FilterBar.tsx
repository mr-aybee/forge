import * as Popover from "@radix-ui/react-popover";
import { Check, ChevronDown, X } from "lucide-react";
import { useRef, useState, type KeyboardEvent, type ReactNode } from "react";

import { SearchInput, Segmented, type SegmentOption } from "../ui";

/** One choice of a filter pill. */
export interface FilterOption {
  value: string;
  label: string;
  count?: number;
}

/** A filter pill: single or multi select. Its value lives in the page's URL search. */
export interface FilterDef {
  id: string;
  label: string;
  options: FilterOption[];
  value: string[];
  onChange: (v: string[]) => void;
  multi?: boolean;
  /** Unavailable; say why in `title`. */
  disabled?: boolean;
  title?: string;
  /** Left out entirely (e.g. a cluster filter on a cluster's own page). */
  hidden?: boolean;
}

/** Optional status segmented control on the left of the bar. */
export interface StatusFilter<S extends string> {
  label: string;
  value: S;
  options: readonly SegmentOption<S>[];
  onChange: (v: S) => void;
}

/**
 * Filter toolbar: status segments with counts, a fixed-width search, compact
 * pill dropdowns, "Clear filters" while anything is set, and a `view` slot
 * (the `ViewToggle`) on the right. Wraps instead of overflowing.
 */
export function FilterBar<S extends string>({
  status,
  search,
  filters = [],
  onClear,
  view,
  children,
}: {
  status?: StatusFilter<S>;
  search?: { value: string; onChange: (v: string) => void; placeholder?: string };
  filters?: FilterDef[];
  /** Resets search and every pill (not the status). Shown only while something is set. */
  onClear?: () => void;
  view?: ReactNode;
  /** Extra controls after the pills. */
  children?: ReactNode;
}) {
  const shown = filters.filter((f) => !f.hidden);
  const active = !!search?.value || shown.some((f) => f.value.length > 0);
  return (
    <div className="mb-3 flex min-w-0 flex-wrap items-center gap-2">
      {status && <Segmented label={status.label} value={status.value} options={status.options} onChange={status.onChange} />}
      {search && (
        <div className="w-56 shrink-0 [&_input]:h-7 [&_input]:w-full [&_input]:py-0 [&_input]:text-xs">
          <SearchInput value={search.value} onChange={search.onChange} placeholder={search.placeholder} />
        </div>
      )}
      {shown.map((f) => (
        <FilterPill key={f.id} f={f} />
      ))}
      {children}
      {active && onClear && (
        <button type="button" onClick={onClear} className="press inline-flex h-7 items-center gap-1 rounded px-1.5 text-xs text-muted hover:text-fg focus-visible:outline-2 focus-visible:outline-accent">
          <X className="size-3.5" aria-hidden /> Clear filters
        </button>
      )}
      {view && <div className="ml-auto">{view}</div>}
    </div>
  );
}

function pillLabel(f: FilterDef): string {
  if (!f.value.length) return f.label;
  if (f.value.length > 1) return `${f.label} · ${f.value.length}`;
  const v = f.value[0] ?? "";
  return `${f.label}: ${f.options.find((o) => o.value === v)?.label ?? v}`;
}

function FilterPill({ f }: { f: FilterDef }) {
  const [open, setOpen] = useState(false);
  const [q, setQ] = useState("");
  const list = useRef<HTMLDivElement>(null);
  const set = f.value.length > 0;
  const label = pillLabel(f);
  const needle = q.trim().toLowerCase();
  const options = needle ? f.options.filter((o) => o.label.toLowerCase().includes(needle)) : f.options;
  const pick = (v: string) => {
    if (f.multi) f.onChange(f.value.includes(v) ? f.value.filter((x) => x !== v) : [...f.value, v]);
    else {
      f.onChange(f.value[0] === v ? [] : [v]);
      setOpen(false);
    }
  };
  // Arrow keys move between options; Tab still leaves the list.
  const onKey = (e: KeyboardEvent) => {
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    const items = [...(list.current?.querySelectorAll<HTMLButtonElement>("button[role=option]") ?? [])];
    if (!items.length) return;
    e.preventDefault();
    const i = items.indexOf(document.activeElement as HTMLButtonElement);
    const next = e.key === "ArrowDown" ? (i + 1) % items.length : (i - 1 + items.length) % items.length;
    items[next]?.focus();
  };
  return (
    <Popover.Root
      open={open}
      onOpenChange={(o) => {
        setOpen(o);
        if (!o) setQ("");
      }}
    >
      <Popover.Trigger asChild>
        <button
          type="button"
          disabled={f.disabled}
          title={f.title ?? (label.length > 28 ? label : undefined)}
          className={`press inline-flex h-7 max-w-[220px] min-w-0 items-center gap-1 rounded-full border px-2.5 text-xs focus-visible:outline-2 focus-visible:outline-accent disabled:cursor-not-allowed disabled:opacity-50 ${
            set ? "border-accent/50 bg-accent-soft text-accent" : "border-line bg-panel text-muted hover:border-line-strong hover:text-fg"
          }`}
        >
          <span className="min-w-0 truncate">{label}</span>
          <ChevronDown className="size-3.5 shrink-0 opacity-70" aria-hidden />
        </button>
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content align="start" sideOffset={4} collisionPadding={8} onKeyDown={onKey} className="pop-in z-50 w-60 rounded-lg border border-line bg-panel p-1 shadow-[var(--shadow-pop)]">
          {f.options.length > 8 && (
            <input
              autoFocus
              value={q}
              onChange={(e) => setQ(e.target.value)}
              placeholder={`Filter ${f.label.toLowerCase()}…`}
              aria-label={`Filter ${f.label.toLowerCase()} options`}
              className="mb-1 w-full rounded-md border border-line bg-panel px-2 py-1 text-xs outline-none focus:border-accent"
            />
          )}
          <div ref={list} role="listbox" aria-label={f.label} aria-multiselectable={f.multi || undefined} className="max-h-64 overflow-y-auto">
            {options.length === 0 && <p className="px-2 py-1.5 text-xs text-faint">No matches</p>}
            {options.map((o) => {
              const on = f.value.includes(o.value);
              return (
                <button
                  key={o.value}
                  type="button"
                  role="option"
                  aria-selected={on}
                  onClick={() => pick(o.value)}
                  className="flex w-full min-w-0 items-center gap-2 rounded px-2 py-1 text-left text-xs outline-none hover:bg-panel-2 focus-visible:bg-panel-2"
                >
                  <Check className={`size-3.5 shrink-0 ${on ? "text-accent" : "invisible"}`} aria-hidden />
                  <span className="min-w-0 flex-1 truncate" title={o.label}>
                    {o.label}
                  </span>
                  {o.count !== undefined && <span className="shrink-0 font-mono text-[10px] text-faint tabular-nums">{o.count}</span>}
                </button>
              );
            })}
          </div>
          {set && (
            <button
              type="button"
              onClick={() => {
                f.onChange([]);
                setOpen(false);
              }}
              className="mt-1 w-full rounded border-t border-line px-2 pt-1.5 pb-1 text-left text-xs text-muted hover:text-fg"
            >
              Clear {f.label.toLowerCase()}
            </button>
          )}
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}
