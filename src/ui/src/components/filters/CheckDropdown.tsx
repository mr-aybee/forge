import * as Popover from "@radix-ui/react-popover";
import { Check, ChevronDown, Search, X } from "lucide-react";
import { type ReactNode, useMemo, useState } from "react";

export interface CheckOption {
  value: string;
  label: string;
  /** Options with the same group are listed under one heading. */
  group?: string;
  count?: number;
  /** Shown before the label, such as an avatar initial. */
  icon?: ReactNode;
  /** Mono label (ids, action names). */
  mono?: boolean;
}

/**
 * A pill dropdown with a search box and checkboxes, grouped under headings
 * ("Select group" ticks a whole group). `single` keeps at most one choice,
 * for filters the server takes once.
 */
export function CheckDropdown({
  label,
  placeholder,
  noun,
  options,
  value,
  onChange,
  single,
  groupClass,
  width = "w-48",
}: {
  /** Accessible name, such as "Action types". */
  label: string;
  /** Shown when nothing is chosen, such as "All types". */
  placeholder: string;
  /** Plural used in "3 types" and the search box. */
  noun: string;
  options: CheckOption[];
  value: string[];
  onChange: (v: string[]) => void;
  single?: boolean;
  /** Classes of a group heading chip. */
  groupClass?: (group: string) => string;
  width?: string;
}) {
  const [open, setOpen] = useState(false);
  const [q, setQ] = useState("");
  const needle = q.trim().toLowerCase();
  const groups = useMemo(() => {
    const m = new Map<string, CheckOption[]>();
    for (const o of options) {
      if (needle && !`${o.label} ${o.value}`.toLowerCase().includes(needle)) continue;
      const g = o.group ?? "";
      m.set(g, [...(m.get(g) ?? []), o]);
    }
    return [...m.entries()];
  }, [options, needle]);
  const chosen = new Set(value);
  const toggle = (v: string) => {
    if (single) {
      onChange(chosen.has(v) ? [] : [v]);
      setOpen(false);
    } else onChange(chosen.has(v) ? value.filter((x) => x !== v) : [...value, v]);
  };
  const toggleGroup = (items: CheckOption[]) => {
    const all = items.every((i) => chosen.has(i.value));
    const names = items.map((i) => i.value);
    onChange(all ? value.filter((x) => !names.includes(x)) : [...new Set([...value, ...names])]);
  };
  const first = options.find((o) => o.value === value[0]);
  const text = value.length === 0 ? placeholder : value.length === 1 ? (first?.label ?? value[0]) : `${value.length} ${noun}`;

  return (
    <Popover.Root open={open} onOpenChange={(o) => (setOpen(o), o && setQ(""))}>
      <Popover.Trigger asChild>
        <button
          type="button"
          aria-label={`${label}: ${text}`}
          className={`press flex h-7 ${width} max-w-full min-w-0 items-center gap-1.5 rounded-full border bg-panel px-2.5 text-left text-xs hover:border-faint/60 focus-visible:outline-2 focus-visible:outline-accent aria-expanded:border-accent ${value.length ? "border-accent/60" : "border-line"}`}
        >
          <span className={`min-w-0 flex-1 truncate ${value.length ? "" : "text-muted"} ${value.length === 1 && first?.mono ? "font-mono text-[11px]" : ""}`} title={text}>
            {text}
          </span>
          {value.length > 0 && (
            <span
              role="button"
              tabIndex={-1}
              aria-label={`Clear ${label.toLowerCase()}`}
              className="rounded p-0.5 text-faint hover:bg-panel-2 hover:text-fg"
              onPointerDown={(e) => (e.preventDefault(), e.stopPropagation(), onChange([]))}
            >
              <X className="size-3" />
            </span>
          )}
          <ChevronDown className={`size-3.5 shrink-0 text-faint transition ${open ? "rotate-180" : ""}`} />
        </button>
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content align="end" sideOffset={4} collisionPadding={8} className="z-50 w-80 max-w-[calc(100vw-1rem)] rounded-lg border border-line bg-panel shadow-pop">
          <div className="flex items-center gap-2 border-b border-line px-2.5 py-2">
            <Search className="size-3.5 text-faint" />
            <input
              autoFocus
              value={q}
              onChange={(e) => setQ(e.target.value)}
              placeholder={`Search ${noun}`}
              aria-label={`Search ${noun}`}
              data-bare
              className="min-w-0 flex-1 bg-transparent text-[13px] outline-none placeholder:text-faint"
            />
          </div>
          <div role="listbox" aria-multiselectable={!single} aria-label={label} className="max-h-80 overflow-auto py-1">
            {groups.length === 0 && <div className="px-3 py-2 text-xs text-faint">No matching {noun}</div>}
            {groups.map(([g, items]) => {
              const all = items.every((i) => chosen.has(i.value));
              return (
                <div key={g || "-"}>
                  {g && (
                    <div className="flex w-full items-center gap-2 px-2.5 pt-2 pb-1">
                      <span className={`rounded px-1.5 py-0.5 text-[10px] font-semibold tracking-wide uppercase ${groupClass?.(g) ?? "bg-panel-2 text-muted"}`}>{g}</span>
                      {!single && items.length > 1 && (
                        <button type="button" onClick={() => toggleGroup(items)} className="ml-auto rounded px-1 text-[11px] text-faint hover:text-fg focus-visible:outline-2 focus-visible:outline-accent">
                          {all ? "Clear group" : "Select group"}
                        </button>
                      )}
                    </div>
                  )}
                  {items.map((o) => (
                    <button
                      key={o.value}
                      type="button"
                      role="option"
                      aria-selected={chosen.has(o.value)}
                      onClick={() => toggle(o.value)}
                      className="flex w-full items-center gap-2 px-2.5 py-1 text-left text-[13px] hover:bg-panel-2 focus-visible:bg-panel-2 focus-visible:outline-none"
                    >
                      <span className={`grid size-3.5 shrink-0 place-items-center border ${single ? "rounded-full" : "rounded-sm"} ${chosen.has(o.value) ? "border-accent bg-accent text-bg" : "border-line-strong"}`}>
                        {chosen.has(o.value) && <Check className="size-3" />}
                      </span>
                      {o.icon}
                      <span className={`min-w-0 flex-1 truncate ${o.mono ? "font-mono text-xs" : ""}`} title={o.label}>
                        {o.label}
                      </span>
                      {o.count !== undefined && <span className="font-mono text-[11px] text-faint tabular-nums">{o.count.toLocaleString()}</span>}
                    </button>
                  ))}
                </div>
              );
            })}
          </div>
          <div className="flex items-center justify-between border-t border-line px-2.5 py-1.5 text-xs text-muted">
            <span>{single ? (value.length ? "1 selected" : "Any") : `${value.length} selected`}</span>
            <button type="button" className="hover:text-fg disabled:opacity-50" onClick={() => onChange([])} disabled={!value.length}>
              Clear
            </button>
          </div>
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}
