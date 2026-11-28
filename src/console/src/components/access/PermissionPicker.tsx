// Permission picker in the style of fine-grained access tokens: one card per
// area with an access level (None / Read / Write / Admin) that selects every
// action at or below it, expandable to per-action checkboxes.

import { ChevronDown, ChevronRight, ShieldAlert, X } from "lucide-react";
import { useMemo, useState } from "react";

import { byArea, LEVEL_RANK, LEVELS, type AccessLevel, type CatalogAction } from "../../api/accessControl";
import { Badge, SearchInput } from "../ui";

const LEVEL_TONE: Record<AccessLevel, "muted" | "accent" | "bad"> = { read: "muted", write: "accent", admin: "bad" };
const LEVEL_NAME: Record<AccessLevel, string> = { read: "Read", write: "Write", admin: "Admin" };

/** The badges of one permission: its level (risk) and the MFA requirement. */
export function ActionBadges({ a }: { a: CatalogAction }) {
  return (
    <span className="inline-flex shrink-0 items-center gap-1">
      <Badge tone={LEVEL_TONE[a.level]}>{LEVEL_NAME[a.level]}</Badge>
      {a.mfa && (
        <span title="Needs an MFA-verified sign-in">
          <Badge tone="warn">MFA</Badge>
        </span>
      )}
    </span>
  );
}

/** The area's level when the selection is exactly "every action at or below L"; `none` when empty; `custom` otherwise. */
function areaLevel(actions: CatalogAction[], sel: ReadonlySet<string>): AccessLevel | "none" | "custom" {
  const picked = actions.filter((a) => sel.has(a.action));
  if (picked.length === 0) return "none";
  for (const l of LEVELS) {
    const want = actions.filter((a) => LEVEL_RANK[a.level] <= LEVEL_RANK[l]);
    if (want.length === picked.length && want.every((a) => sel.has(a.action))) {
      // A level that adds nothing over a lower one is reported as the lower one.
      return l;
    }
  }
  return "custom";
}

export function PermissionPicker({
  catalog,
  selected,
  onChange,
  readOnly,
}: {
  catalog: CatalogAction[];
  selected: ReadonlySet<string>;
  onChange: (next: Set<string>) => void;
  readOnly?: boolean;
}) {
  const [q, setQ] = useState("");
  const [open, setOpen] = useState<Set<string>>(() => new Set());
  const areas = useMemo(() => byArea(catalog), [catalog]);
  const byId = useMemo(() => new Map(catalog.map((a) => [a.action, a])), [catalog]);
  const needle = q.trim().toLowerCase();
  // Selected actions the catalog doesn't know (removed from the server): shown so they can be dropped.
  const unknown = [...selected].filter((s) => !byId.has(s));

  const set = (ids: string[], on: boolean) => {
    const n = new Set(selected);
    for (const id of ids) {
      if (on) n.add(id);
      else n.delete(id);
    }
    onChange(n);
  };
  const setLevel = (actions: CatalogAction[], l: AccessLevel | "none") => {
    const n = new Set(selected);
    for (const a of actions) {
      if (l !== "none" && LEVEL_RANK[a.level] <= LEVEL_RANK[l]) n.add(a.action);
      else n.delete(a.action);
    }
    onChange(n);
  };
  const toggleOpen = (area: string) =>
    setOpen((s) => {
      const n = new Set(s);
      if (n.has(area)) n.delete(area);
      else n.add(area);
      return n;
    });

  return (
    <div className="space-y-3">
      <div className="flex flex-wrap items-center gap-3">
        <SearchInput value={q} onChange={setQ} placeholder="Search permissions…" />
        <span className="font-mono text-[11px] text-faint tabular-nums">
          {selected.size} of {catalog.length} selected
        </span>
        {!readOnly && selected.size > 0 && (
          <button type="button" onClick={() => onChange(new Set())} className="text-xs text-muted hover:text-bad">
            Clear all
          </button>
        )}
      </div>

      {(selected.size > 0 || unknown.length > 0) && (
        <div className="flex max-h-28 flex-wrap gap-1 overflow-y-auto">
          {[...selected].sort().map((id) => {
            const a = byId.get(id);
            return (
              <span key={id} className={`inline-flex h-5 items-center gap-1 rounded-full border px-2 font-mono text-[11px] ${a ? "border-grant-line bg-grant-soft text-grant" : "border-grant-narrow/50 bg-grant-narrow-soft text-grant-narrow"}`} title={a ? [`${a.area} · ${a.label}`, a.description, a.mfa ? "Needs an MFA-verified sign-in" : ""].filter(Boolean).join("\n") : "Not in the catalog of this server"}>
                {id}
                {!readOnly && (
                  <button type="button" aria-label={`Remove ${id}`} onClick={() => set([id], false)} className="opacity-60 hover:text-grant-remove hover:opacity-100">
                    <X className="size-3" />
                  </button>
                )}
              </span>
            );
          })}
        </div>
      )}

      <div className="divide-y divide-line rounded-md border border-line">
        {areas.map(({ area, actions }) => {
          const shown = needle ? actions.filter((a) => `${a.action} ${a.label} ${a.description} ${a.area}`.toLowerCase().includes(needle)) : actions;
          if (needle && shown.length === 0) return null;
          const count = actions.filter((a) => selected.has(a.action)).length;
          const all = count === actions.length;
          const lvl = areaLevel(actions, selected);
          const expanded = !!needle || open.has(area);
          const has = (l: AccessLevel) => actions.some((a) => a.level === l);
          return (
            <section key={area}>
              <header className="flex flex-wrap items-center gap-2 px-3 py-2">
                <input
                  type="checkbox"
                  aria-label={`All of ${area}`}
                  className="size-3.5 accent-[var(--color-accent)]"
                  checked={all}
                  ref={(el) => {
                    if (el) el.indeterminate = count > 0 && !all;
                  }}
                  disabled={readOnly}
                  onChange={() => set(actions.map((a) => a.action), !all)}
                />
                <button type="button" onClick={() => toggleOpen(area)} aria-expanded={expanded} className="flex min-w-0 flex-1 items-center gap-1.5 text-left text-[13px] font-medium">
                  {expanded ? <ChevronDown className="size-3.5 shrink-0 text-faint" /> : <ChevronRight className="size-3.5 shrink-0 text-faint" />}
                  <span className="truncate" title={area}>{area}</span>
                  <span className="font-mono text-[11px] font-normal text-faint tabular-nums">
                    {count}/{actions.length}
                  </span>
                  {actions.some((a) => a.mfa && selected.has(a.action)) && <ShieldAlert className="size-3.5 shrink-0 text-warn" aria-label="Includes MFA-protected permissions" />}
                </button>
                <div role="radiogroup" aria-label={`Access level for ${area}`} className="inline-flex shrink-0 rounded-md border border-line bg-panel-2 p-0.5" title={lvl === "custom" ? "Custom selection: pick a level to reset it" : undefined}>
                  {(["none", ...LEVELS.filter(has)] as const).map((l) => (
                    <button
                      key={l}
                      type="button"
                      role="radio"
                      aria-checked={lvl === l}
                      disabled={readOnly}
                      onClick={() => setLevel(actions, l)}
                      className={`h-5 rounded px-1.5 text-[11px] transition disabled:cursor-not-allowed font-semibold ${lvl === l ? (l === "none" ? "bg-panel text-fg shadow-[var(--shadow-card)]" : "bg-panel text-accent shadow-[var(--shadow-card)]") : "text-muted enabled:hover:text-fg"}`}
                    >
                      {l === "none" ? "None" : LEVEL_NAME[l]}
                    </button>
                  ))}
                  {lvl === "custom" && <span className="rounded bg-grant-narrow-soft px-1.5 text-[11px] leading-5 font-semibold text-grant-narrow">Custom</span>}
                </div>
              </header>
              {expanded && (
                <ul className="border-t border-line bg-panel-2/40 py-1">
                  {shown.map((a) => (
                    <li key={a.action}>
                      <label className={`flex items-start gap-2 px-3 py-1.5 pl-8 ${readOnly ? "" : "cursor-pointer hover:bg-panel-2"}`}>
                        <input type="checkbox" className="mt-0.5 size-3.5 shrink-0 accent-[var(--color-accent)]" checked={selected.has(a.action)} disabled={readOnly} onChange={(e) => set([a.action], e.target.checked)} />
                        <span className="min-w-0 flex-1">
                          <span className="flex flex-wrap items-center gap-x-2 gap-y-0.5">
                            <span className="text-[13px]">{a.label}</span>
                            <span className="font-mono text-[11px] text-faint select-all">{a.action}</span>
                            <ActionBadges a={a} />
                          </span>
                          {a.description && <span className="block text-xs text-muted">{a.description}</span>}
                        </span>
                      </label>
                    </li>
                  ))}
                </ul>
              )}
            </section>
          );
        })}
        {needle && !areas.some(({ actions }) => actions.some((a) => `${a.action} ${a.label} ${a.description} ${a.area}`.toLowerCase().includes(needle))) && (
          <p className="px-3 py-3 text-xs text-muted">No permission matches “{q}”.</p>
        )}
      </div>
    </div>
  );
}
