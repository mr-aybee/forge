import * as Popover from "@radix-ui/react-popover";
import { Check, ChevronDown, ChevronRight, Minus, Search, X } from "lucide-react";
import { type KeyboardEvent, useId, useMemo, useRef, useState } from "react";

/** A child row (e.g. a node a service runs on). */
export interface TreeLeaf {
  id: string;
  label: string;
  /** Small muted chip after the label (e.g. "standby"). */
  badge?: string;
}

/** A top-level row (e.g. a service). Children are shown only when there are two or more. */
export interface TreeGroup {
  id: string;
  label: string;
  /** Muted count after the label; defaults to the number of children. */
  count?: number;
  children: TreeLeaf[];
}

/**
 * What is selected: whole groups, plus single leaves of groups that are not
 * whole. A group never appears in both.
 */
export interface TreeSelection {
  groups: string[];
  leaves: { group: string; leaf: string }[];
}

export const emptyTreeSelection: TreeSelection = { groups: [], leaves: [] };

type State = "on" | "off" | "mixed";

/** Pure selection rules, exported for reuse and tests. */
export const treeRules = {
  groupState(sel: TreeSelection, g: TreeGroup): State {
    if (sel.groups.includes(g.id)) return "on";
    return sel.leaves.some((l) => l.group === g.id) ? "mixed" : "off";
  },
  /** Off → whole group; mixed → whole group; on → nothing of it. */
  toggleGroup(sel: TreeSelection, g: TreeGroup): TreeSelection {
    const leaves = sel.leaves.filter((l) => l.group !== g.id);
    const groups = sel.groups.filter((x) => x !== g.id);
    return treeRules.groupState(sel, g) === "on" ? { groups, leaves } : { groups: [...groups, g.id], leaves };
  },
  /** Unchecking a leaf of a whole group leaves its siblings; checking every leaf collapses to the group. */
  toggleLeaf(sel: TreeSelection, g: TreeGroup, leaf: string): TreeSelection {
    const others = sel.leaves.filter((l) => l.group !== g.id);
    const groups = sel.groups.filter((x) => x !== g.id);
    let mine: string[];
    if (sel.groups.includes(g.id)) mine = g.children.map((c) => c.id).filter((c) => c !== leaf);
    else {
      const cur = new Set(sel.leaves.filter((l) => l.group === g.id).map((l) => l.leaf));
      if (cur.has(leaf)) cur.delete(leaf);
      else cur.add(leaf);
      mine = [...cur];
    }
    if (g.children.length > 0 && g.children.every((c) => mine.includes(c.id))) return { groups: [...groups, g.id], leaves: others };
    return { groups, leaves: [...others, ...mine.map((m) => ({ group: g.id, leaf: m }))] };
  },
  count: (sel: TreeSelection) => sel.groups.length + sel.leaves.length,
};

type Row = { kind: "group"; g: TreeGroup; key: string } | { kind: "leaf"; g: TreeGroup; leaf: TreeLeaf; key: string };

const short = (id: string) => (id.length > 12 ? id.slice(0, 8) : id);

/**
 * Multi-select over a two-level tree (groups → leaves), e.g. service → node.
 * Search filters groups and leaves (a matching leaf keeps its group visible
 * and expanded). Keyboard: ↓ from the search box enters the tree; ↑/↓ move,
 * Home/End jump, →/← expand/collapse (← on a leaf goes to its group),
 * Space/Enter toggle, typing returns to the search box, Esc closes.
 */
export function TreeMultiSelect({
  groups,
  value,
  onChange,
  placeholder = "All",
  label,
  noun = "items",
  className = "",
}: {
  groups: TreeGroup[];
  value: TreeSelection;
  onChange: (next: TreeSelection) => void;
  /** Trigger text when nothing is selected. */
  placeholder?: string;
  /** Accessible name of the trigger and tree. */
  label: string;
  /** Plural used in the footer ("3 services"). */
  noun?: string;
  className?: string;
}) {
  const [open, setOpen] = useState(false);
  const btn = useRef<HTMLButtonElement>(null);
  const n = treeRules.count(value);

  // Labels for the trigger; ids no longer in `groups` (stale links) fall back to a short id.
  const byId = useMemo(() => new Map(groups.map((g) => [g.id, g])), [groups]);
  const names = [
    ...value.groups.map((id) => byId.get(id)?.label ?? short(id)),
    ...value.leaves.map((l) => {
      const g = byId.get(l.group);
      return `${g?.label ?? short(l.group)} · ${g?.children.find((c) => c.id === l.leaf)?.label ?? short(l.leaf)}`;
    }),
  ];
  const summary = n === 0 ? placeholder : n === 1 ? names[0]! : `${names[0]} +${n - 1}`;

  return (
    <div className={`relative ${className.includes("w-") ? "" : "w-full"} ${className}`}>
      <Popover.Root open={open} onOpenChange={setOpen}>
        <Popover.Trigger asChild>
          <button
            ref={btn}
            type="button"
            aria-haspopup="tree"
            aria-label={`${label}: ${summary}`}
            title={n > 1 ? names.join("\n") : summary}
            onKeyDown={(e) => {
              if (e.key === "ArrowDown" || e.key === "ArrowUp") {
                e.preventDefault();
                setOpen(true);
              }
            }}
            className={`flex h-8 w-full items-center gap-2 rounded-md border bg-panel pr-1.5 pl-2.5 text-left text-[13px] transition-[border-color,box-shadow] hover:border-faint/60 focus-visible:border-accent focus-visible:shadow-[0_0_0_3px_color-mix(in_srgb,var(--color-accent)_18%,transparent)] focus-visible:outline-none aria-expanded:border-accent ${n ? "border-accent/60" : "border-line"} ${n ? "pr-7" : ""}`}
          >
            <span className={`min-w-0 flex-1 truncate ${n ? "" : "text-faint"}`}>{summary}</span>
            {n > 1 && <span className="shrink-0 rounded bg-accent-soft px-1 font-mono text-[10.5px] text-accent tabular-nums">{n}</span>}
            {!n && <ChevronDown className={`size-3.5 shrink-0 text-faint transition ${open ? "rotate-180" : ""}`} />}
          </button>
        </Popover.Trigger>
        {n > 0 && (
          <button
            type="button"
            aria-label={`Clear ${label.toLowerCase()} filter`}
            title="Clear"
            onClick={() => {
              onChange(emptyTreeSelection);
              btn.current?.focus();
            }}
            className="absolute top-1/2 right-1.5 grid size-5 -translate-y-1/2 place-items-center rounded text-faint hover:bg-panel-2 hover:text-fg focus-visible:outline-2 focus-visible:outline-accent"
          >
            <X className="size-3.5" />
          </button>
        )}
        <Popover.Portal>
          <Popover.Content
            align="start"
            sideOffset={4}
            collisionPadding={8}
            onOpenAutoFocus={(e) => e.preventDefault()}
            onCloseAutoFocus={(e) => {
              e.preventDefault();
              btn.current?.focus();
            }}
            style={{ width: "max(var(--radix-popover-trigger-width), 260px)" }}
            className="z-[100] overflow-hidden rounded-md border border-line bg-panel text-[13px] shadow-[0_8px_28px_rgba(0,0,0,0.18)]"
          >
            <Panel groups={groups} value={value} onChange={onChange} label={label} noun={noun} />
          </Popover.Content>
        </Popover.Portal>
      </Popover.Root>
    </div>
  );
}

/** The popover body; mounts on open, so search and expansion reset each time. */
function Panel({ groups, value, onChange, label, noun }: { groups: TreeGroup[]; value: TreeSelection; onChange: (s: TreeSelection) => void; label: string; noun: string }) {
  const uid = useId();
  const [q, setQ] = useState("");
  // Start with partially selected groups expanded, so the chosen nodes are visible.
  const [expanded, setExpanded] = useState<Set<string>>(() => new Set(value.leaves.map((l) => l.group)));
  const [active, setActive] = useState(-1);
  const input = useRef<HTMLInputElement>(null);
  const tree = useRef<HTMLDivElement>(null);

  const needle = q.trim().toLowerCase();
  const rows: Row[] = useMemo(() => {
    const out: Row[] = [];
    for (const g of groups) {
      const nested = g.children.length >= 2;
      const gHit = !needle || g.label.toLowerCase().includes(needle);
      const kids = nested ? g.children.filter((c) => gHit || c.label.toLowerCase().includes(needle)) : [];
      if (!gHit && !kids.length) continue;
      out.push({ kind: "group", g, key: `g:${g.id}` });
      const show = needle && !gHit ? true : expanded.has(g.id);
      if (show) for (const c of kids) out.push({ kind: "leaf", g, leaf: c, key: `l:${g.id}:${c.id}` });
    }
    return out;
  }, [groups, needle, expanded]);
  const cur = active >= 0 && active < rows.length ? active : -1;
  const rowId = (i: number) => `${uid}-r${i}`;

  const isOpen = (g: TreeGroup) => g.children.length >= 2 && (expanded.has(g.id) || rows.some((r) => r.kind === "leaf" && r.g.id === g.id));
  const setExp = (g: TreeGroup, on: boolean) =>
    setExpanded((s) => {
      const next = new Set(s);
      if (on) next.add(g.id);
      else next.delete(g.id);
      return next;
    });

  const focusRow = (i: number) => {
    setActive(i);
    tree.current?.focus();
    tree.current?.querySelector(`#${CSS.escape(rowId(i))}`)?.scrollIntoView({ block: "nearest" });
  };
  const toggle = (r: Row) => onChange(r.kind === "group" ? treeRules.toggleGroup(value, r.g) : treeRules.toggleLeaf(value, r.g, r.leaf.id));

  const onTreeKey = (e: KeyboardEvent) => {
    const r = rows[cur];
    switch (e.key) {
      case "ArrowDown":
        e.preventDefault();
        if (rows.length) focusRow(Math.min(rows.length - 1, cur + 1));
        return;
      case "ArrowUp":
        e.preventDefault();
        if (cur <= 0) (setActive(-1), input.current?.focus());
        else focusRow(cur - 1);
        return;
      case "Home":
        e.preventDefault();
        if (rows.length) focusRow(0);
        return;
      case "End":
        e.preventDefault();
        if (rows.length) focusRow(rows.length - 1);
        return;
      case "ArrowRight":
        e.preventDefault();
        if (r?.kind === "group" && r.g.children.length >= 2) {
          if (!isOpen(r.g)) setExp(r.g, true);
          else if (rows[cur + 1]?.kind === "leaf") focusRow(cur + 1);
        }
        return;
      case "ArrowLeft":
        e.preventDefault();
        if (r?.kind === "group" && isOpen(r.g) && !needle) setExp(r.g, false);
        else if (r?.kind === "leaf") focusRow(rows.findIndex((x) => x.kind === "group" && x.g.id === r.g.id));
        return;
      case " ":
      case "Enter":
        e.preventDefault();
        if (r) toggle(r);
        return;
    }
    // Type-to-search: printable keys go to the search box (focus moves before the key lands).
    if (e.key.length === 1 && !e.ctrlKey && !e.metaKey && !e.altKey) input.current?.focus();
  };

  const selected = treeRules.count(value);
  return (
    <>
      <div className="flex items-center gap-1.5 border-b border-line px-2.5">
        <Search className="size-3.5 shrink-0 text-faint" />
        <input
          ref={input}
          autoFocus
          data-bare
          value={q}
          onChange={(e) => (setQ(e.target.value), setActive(-1))}
          onKeyDown={(e) => {
            if (e.key === "ArrowDown" && rows.length) (e.preventDefault(), focusRow(0));
            else if (e.key === "Enter" && rows.length) (e.preventDefault(), focusRow(0));
          }}
          placeholder="Search services and nodes…"
          aria-label={`Search ${label.toLowerCase()}`}
          aria-controls={`${uid}-tree`}
          className="h-8 w-full min-w-0 bg-transparent text-[13px] outline-none placeholder:text-faint"
        />
      </div>
      <div
        ref={tree}
        id={`${uid}-tree`}
        role="tree"
        aria-label={label}
        aria-multiselectable
        tabIndex={rows.length ? 0 : -1}
        aria-activedescendant={cur >= 0 ? rowId(cur) : undefined}
        onKeyDown={onTreeKey}
        onFocus={() => cur < 0 && rows.length && setActive(0)}
        className="group/tree max-h-72 overflow-y-auto py-1 outline-none"
      >
        {rows.map((r, i) => {
          const isActive = i === cur;
          const state: State =
            r.kind === "group" ? treeRules.groupState(value, r.g) : value.groups.includes(r.g.id) || value.leaves.some((l) => l.group === r.g.id && l.leaf === r.leaf.id) ? "on" : "off";
          const nested = r.kind === "group" && r.g.children.length >= 2;
          const opened = nested && isOpen(r.g);
          const count = r.kind === "group" ? (r.g.count ?? r.g.children.length) : 0;
          // The tree line stops at the last node of its service.
          const lastLeaf = r.kind === "leaf" && !(rows[i + 1]?.kind === "leaf" && rows[i + 1]?.g.id === r.g.id);
          return (
            <div
              key={r.key}
              id={rowId(i)}
              role="treeitem"
              aria-level={r.kind === "group" ? 1 : 2}
              aria-checked={state === "mixed" ? "mixed" : state === "on"}
              aria-selected={state === "on"}
              aria-expanded={nested ? opened : undefined}
              onMouseDown={(e) => e.preventDefault()}
              onMouseMove={() => !isActive && setActive(i)}
              onClick={() => (setActive(i), toggle(r))}
              className={`relative flex cursor-pointer items-center gap-2 py-1 pr-2.5 ${r.kind === "leaf" ? "pl-[60px]" : "pl-1.5"} ${isActive ? "bg-accent-soft text-fg" : ""} ${isActive ? "group-focus-visible/tree:shadow-[inset_2px_0_0_var(--color-accent)]" : ""}`}
            >
              {r.kind === "leaf" && (
                // Elbow connector (|_) from the service's checkbox to this node's.
                <span aria-hidden className="pointer-events-none absolute inset-y-0 left-[40px] w-[14px]">
                  <span className={`absolute left-0 top-0 w-px bg-line ${lastLeaf ? "h-1/2" : "h-full"}`} />
                  <span className="absolute left-0 top-1/2 h-px w-full bg-line" />
                </span>
              )}
              {r.kind === "group" &&
                (nested ? (
                  <button
                    type="button"
                    tabIndex={-1}
                    aria-hidden
                    onClick={(e) => (e.stopPropagation(), setActive(i), setExp(r.g, !opened))}
                    className="grid size-5 shrink-0 place-items-center rounded text-faint hover:bg-panel-2 hover:text-fg"
                  >
                    {opened ? <ChevronDown className="size-3.5" /> : <ChevronRight className="size-3.5" />}
                  </button>
                ) : (
                  <span aria-hidden className="size-5 shrink-0" />
                ))}
              <Box state={state} />
              <span className={`min-w-0 flex-1 truncate ${r.kind === "leaf" ? "text-muted" : ""} ${state !== "off" ? "text-fg" : ""}`} title={r.kind === "group" ? r.g.label : r.leaf.label}>
                {r.kind === "group" ? r.g.label : r.leaf.label}
              </span>
              {r.kind === "leaf" && r.leaf.badge && <span className="shrink-0 rounded bg-panel-2 px-1 text-[10.5px] text-faint">{r.leaf.badge}</span>}
              {r.kind === "group" && (
                <span className="shrink-0 font-mono text-[11px] text-faint tabular-nums" title={`${count} node${count === 1 ? "" : "s"}`}>
                  {count}
                </span>
              )}
            </div>
          );
        })}
        {!rows.length && <div className="px-2.5 py-2 text-faint">{groups.length ? "No matches" : `No ${noun}`}</div>}
      </div>
      <div className="flex items-center gap-3 border-t border-line px-2.5 py-1.5 text-xs">
        <button
          type="button"
          className="text-accent hover:underline disabled:cursor-not-allowed disabled:text-faint disabled:no-underline"
          disabled={!groups.length || (value.groups.length === groups.length && !value.leaves.length)}
          onClick={() => onChange({ groups: groups.map((g) => g.id), leaves: [] })}
        >
          Select all
        </button>
        <button type="button" className="text-accent hover:underline disabled:cursor-not-allowed disabled:text-faint disabled:no-underline" disabled={!selected} onClick={() => onChange(emptyTreeSelection)}>
          Clear
        </button>
        <span className="ml-auto font-mono text-[11px] text-faint tabular-nums">{selected ? `${selected} selected` : `all ${noun}`}</span>
      </div>
    </>
  );
}

function Box({ state }: { state: State }) {
  return (
    <span
      aria-hidden
      className={`grid size-3.5 shrink-0 place-items-center rounded-[3px] border transition-colors ${state === "off" ? "border-line-strong bg-panel" : "border-accent bg-accent text-white"}`}
    >
      {state === "on" && <Check className="size-3" strokeWidth={3} />}
      {state === "mixed" && <Minus className="size-3" strokeWidth={3} />}
    </span>
  );
}
