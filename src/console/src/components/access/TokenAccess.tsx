// "Only selected" access for a personal API key: scopes from the resource tree
// and, per scope, actions from the catalog. Only actions the signed-in user
// holds at that scope are offered (the server refuses anything more).

import { ChevronDown, ChevronRight, Plus, X } from "lucide-react";
import { useState } from "react";

import { useAccessCatalog, useEffectiveAccess, type CatalogAction } from "../../api/accessControl";
import { useMe } from "../../api/hooks";
import { errorText } from "../../app/toast";
import { Button, ErrorNote, Skeleton } from "../ui";
import { PermissionPicker } from "./PermissionPicker";
import { ScopeLabel, ScopePicker, useScopeIndex } from "./scopes";

export interface ScopeGrant {
  /** `kind:id`. */
  scope: string;
  actions: Set<string>;
}

const toRef = (k: string) => {
  const i = k.indexOf(":");
  return { kind: k.slice(0, i), id: k.slice(i + 1) };
};

export function TokenAccess({ value, onChange }: { value: ScopeGrant[]; onChange: (v: ScopeGrant[]) => void }) {
  const me = useMe();
  const catalog = useAccessCatalog();
  const idx = useScopeIndex();
  const [picking, setPicking] = useState(value.length === 0);
  const [open, setOpen] = useState<string | null>(value[0]?.scope ?? null);
  const myId = me.data?.user?.id ?? "";

  if (me.isError) return <ErrorNote error={me.error} />;
  if (catalog.isError) return <ErrorNote error={catalog.error} />;
  if (me.isPending || catalog.isPending) return <Skeleton rows={2} />;

  const add = (scope: string) => {
    if (!value.some((g) => g.scope === scope)) onChange([...value, { scope, actions: new Set() }]);
    setOpen(scope);
    setPicking(false);
  };
  const total = value.reduce((n, g) => n + g.actions.size, 0);

  return (
    <div className="space-y-2">
      <div className="flex items-center gap-2 text-xs text-muted">
        <span>
          <span className="font-mono tabular-nums">{value.length}</span> scope{value.length === 1 ? "" : "s"} · <span className="font-mono tabular-nums">{total}</span> action{total === 1 ? "" : "s"}
        </span>
        {value.length >= 50 && <span className="text-warn">At most 50 scopes.</span>}
        <Button type="button" className="ml-auto !py-0.5 text-xs" onClick={() => setPicking(!picking)} disabled={value.length >= 50}>
          <Plus className="size-3.5" /> Add scope
        </Button>
      </div>
      {picking && <ScopePicker idx={idx} value="" onChange={add} height={200} />}
      {value.length > 0 && (
        <ul className="divide-y divide-line rounded-md border border-line">
          {value.map((g) => (
            <li key={g.scope}>
              <div className="flex min-w-0 items-center gap-1.5 px-2 py-1.5">
                <button type="button" onClick={() => setOpen(open === g.scope ? null : g.scope)} aria-expanded={open === g.scope} aria-label="Show actions" className="grid size-5 shrink-0 place-items-center text-faint hover:text-fg">
                  {open === g.scope ? <ChevronDown className="size-3.5" /> : <ChevronRight className="size-3.5" />}
                </button>
                <span className="min-w-0 flex-1">
                  <ScopeLabel idx={idx} scope={toRef(g.scope)} />
                </span>
                <span className={`shrink-0 font-mono text-[11px] tabular-nums ${g.actions.size ? "text-muted" : "text-warn"}`} title={g.actions.size ? undefined : "Pick at least one action"}>
                  {g.actions.size} action{g.actions.size === 1 ? "" : "s"}
                </span>
                <button type="button" onClick={() => onChange(value.filter((x) => x.scope !== g.scope))} aria-label="Remove scope" title="Remove scope" className="grid size-5 shrink-0 place-items-center rounded text-faint hover:text-bad">
                  <X className="size-3.5" />
                </button>
              </div>
              {open === g.scope && (
                <div className="border-t border-line bg-panel-2/30 p-2">
                  <HeldActions
                    user={myId}
                    scope={g.scope}
                    catalog={catalog.data.actions}
                    selected={g.actions}
                    onChange={(actions) => onChange(value.map((x) => (x.scope === g.scope ? { ...x, actions } : x)))}
                  />
                </div>
              )}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

function HeldActions({ user, scope, catalog, selected, onChange }: { user: string; scope: string; catalog: CatalogAction[]; selected: Set<string>; onChange: (s: Set<string>) => void }) {
  const eff = useEffectiveAccess(user, scope);
  if (!user) return <p className="text-xs text-muted">Your account could not be read.</p>;
  if (eff.isPending) return <Skeleton rows={2} />;
  if (eff.isError) return <p className="text-xs text-bad">Can't read what you hold here: {errorText(eff.error)}</p>;
  const held = new Set(eff.data.actions.filter((a) => a.allowed).map((a) => a.action));
  const offered = catalog.filter((a) => held.has(a.action));
  if (offered.length === 0) return <p className="text-xs text-muted">You hold no actions here, so a token can't get any either. Remove this scope.</p>;
  return (
    <>
      <p className="mb-2 text-[11px] text-faint">
        Only the {offered.length} action{offered.length === 1 ? "" : "s"} you hold here are offered.
      </p>
      <PermissionPicker catalog={offered} selected={selected} onChange={onChange} />
    </>
  );
}
