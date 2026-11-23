// "+ Add" chip that opens a small searchable list (roles, groups, members).
// Picking an option stages a change; nothing is written until Apply.

import * as Popover from "@radix-ui/react-popover";
import { Plus } from "lucide-react";
import { type ReactNode, useState } from "react";

export interface AddOption {
  id: string;
  label: string;
  hint?: string;
  badge?: ReactNode;
  disabled?: boolean;
  /** Why a disabled option can't be picked. */
  why?: string;
}

export function AddPopover({
  label = "Add",
  title,
  options,
  onPick,
  footer,
  empty = "Nothing to add.",
}: {
  label?: string;
  title: string;
  options: AddOption[];
  onPick: (id: string) => void;
  /** Extra controls under the list (e.g. an expiry). */
  footer?: ReactNode;
  empty?: string;
}) {
  const [open, setOpen] = useState(false);
  const [q, setQ] = useState("");
  const needle = q.trim().toLowerCase();
  const shown = needle ? options.filter((o) => `${o.label} ${o.hint ?? ""}`.toLowerCase().includes(needle)) : options;
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
          aria-label={title}
          className="inline-flex h-6 shrink-0 items-center gap-0.5 rounded-full border border-dashed border-line-strong bg-panel px-2 text-[11px] font-semibold text-muted transition hover:border-accent hover:bg-accent-soft hover:text-accent focus-visible:outline-2 focus-visible:outline-accent"
        >
          <Plus className="size-3" /> {label}
        </button>
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content align="start" sideOffset={4} collisionPadding={8} className="animate-in z-50 w-64 rounded-lg border border-line bg-panel p-1 shadow-[var(--shadow-pop)]">
          <div className="px-1.5 pt-1 pb-1.5 text-[10px] font-semibold tracking-wide text-faint uppercase">{title}</div>
          <input
            autoFocus
            value={q}
            onChange={(e) => setQ(e.target.value)}
            placeholder="Search…"
            aria-label={`Search: ${title}`}
            className="mb-1 w-full rounded border border-line bg-panel px-2 py-1 text-xs outline-none focus:border-accent"
          />
          <ul className="max-h-60 overflow-y-auto" role="listbox" aria-label={title}>
            {shown.length === 0 && <li className="px-2 py-2 text-xs text-muted">{options.length ? "No match." : empty}</li>}
            {shown.map((o) => (
              <li key={o.id}>
                <button
                  type="button"
                  role="option"
                  aria-selected={false}
                  disabled={o.disabled}
                  title={o.disabled ? o.why : o.hint}
                  onClick={() => {
                    onPick(o.id);
                    setOpen(false);
                    setQ("");
                  }}
                  className="flex w-full min-w-0 items-center gap-1.5 rounded px-2 py-1 text-left text-[13px] hover:bg-panel-2 focus-visible:bg-panel-2 focus-visible:outline-none disabled:cursor-not-allowed disabled:opacity-50"
                >
                  <span className="min-w-0 flex-1 truncate">{o.label}</span>
                  {o.badge}
                  {o.hint && <span className="shrink-0 font-mono text-[10px] text-faint tabular-nums">{o.hint}</span>}
                </button>
              </li>
            ))}
          </ul>
          {footer && <div className="mt-1 border-t border-line px-1.5 pt-1.5 pb-1">{footer}</div>}
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}

/** How a tag reads: applied, inherited, staged add or removal, or narrowed (time-limited). */
export type TagState = "held" | "inherited" | "adding" | "removing" | "narrow";

/** Pill colours per state, from the grant tokens in styles.css. Shared with the legend. */
export const TAG_CLASS: Record<TagState, string> = {
  held: "border-grant-line bg-grant-soft text-grant",
  narrow: "border-grant-narrow/50 bg-grant-narrow-soft text-grant-narrow",
  inherited: "border-line bg-panel-2/60 text-faint",
  adding: "border-dashed border-grant-add bg-grant-add-soft text-grant-add",
  removing: "border-dashed border-grant-remove bg-grant-remove-soft text-grant-remove",
};

/** A capability tag: a role or member chip with an optional × (stage removal) and its state. */
export function Tag({
  children,
  state = "held",
  title,
  onRemove,
  removeLabel,
}: {
  children: ReactNode;
  /** held: applied and editable; narrow: applied with an expiry; inherited: comes from elsewhere (read-only); adding / removing: staged. */
  state?: TagState;
  title?: string;
  onRemove?: () => void;
  removeLabel?: string;
}) {
  const editable = state === "held" || state === "narrow";
  return (
    <span
      className={`inline-flex h-6 max-w-[14rem] min-w-0 items-stretch overflow-hidden rounded-full border text-[11px] font-semibold transition hover:-translate-y-px hover:shadow-[var(--shadow-card)] motion-reduce:hover:translate-y-0 ${TAG_CLASS[state]} ${state === "inherited" ? "font-medium" : ""}`}
      title={title}
    >
      <span className={`flex min-w-0 items-center truncate pl-2.5 ${onRemove ? "pr-1.5" : "pr-2.5"} ${state === "removing" ? "line-through" : ""}`}>
        <span className="truncate">{children}</span>
      </span>
      {onRemove && (
        <button
          type="button"
          aria-label={removeLabel}
          title={removeLabel}
          onClick={onRemove}
          className="grid w-6 shrink-0 place-items-center border-l border-current/25 text-[13px] leading-none text-current opacity-75 hover:bg-current/10 hover:opacity-100 focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-accent"
        >
          {editable ? "×" : "↺"}
        </button>
      )}
    </span>
  );
}

/** Colour key for the tag states, shown above tables of tags. */
export function TagLegend({ states }: { states: { state: TagState; label: string }[] }) {
  return (
    <span className="flex min-w-0 items-center gap-3 overflow-x-auto text-[11px] whitespace-nowrap text-muted" aria-label="Tag states">
      {states.map(({ state, label }) => (
        <span key={state} className="inline-flex items-center gap-1">
          <span className={`size-2 rounded-full border ${TAG_CLASS[state]}`} />
          {label}
        </span>
      ))}
    </span>
  );
}

export const EXPIRY_OPTIONS: { value: string; label: string }[] = [
  { value: "", label: "Never expires" },
  { value: "60", label: "1 hour" },
  { value: "480", label: "8 hours" },
  { value: "1440", label: "1 day" },
  { value: "10080", label: "7 days" },
  { value: "43200", label: "30 days" },
  { value: "129600", label: "90 days" },
];
