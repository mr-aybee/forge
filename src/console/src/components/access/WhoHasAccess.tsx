// Reverse lookup: pick a resource, see every user and group with a role there
// and how they got it (direct, through a group, from a parent scope).

import { UserRound, UsersRound } from "lucide-react";
import { useState } from "react";

import { Badge, Modal, ago, time } from "../ui";
import type { AccessModel, Principal } from "./model";
import { ancestorsOf, ScopeLabel, ScopePicker, type ScopeIndex } from "./scopes";

interface Row {
  key: string;
  principal: Principal;
  name: string;
  role: string;
  via: string;
  expires: number | null;
}

export function WhoHasAccess({
  idx,
  model,
  userName,
  onOpen,
  onClose,
  initialScope = "",
}: {
  idx: ScopeIndex;
  model: AccessModel;
  userName: (id: string) => string;
  /** Opens a principal on the dashboard. */
  onOpen: (p: Principal) => void;
  onClose: () => void;
  initialScope?: string;
}) {
  const [scope, setScope] = useState(initialScope);
  const rows: Row[] = [];
  if (scope) {
    for (const k of [scope, ...ancestorsOf(idx, scope)]) {
      for (const a of model.byScope.get(k) ?? []) {
        const role = model.profileById.get(a.profile_id)?.name ?? a.profile_id.slice(0, 8);
        const from = k === scope ? "here" : `from ${idx.byKey.get(k)?.name ?? k}`;
        if (a.user_id) rows.push({ key: `${a.id}:u`, principal: { kind: "user", id: a.user_id }, name: userName(a.user_id), role, via: from, expires: a.expires_at });
        if (a.group_id) {
          const g = model.groupById.get(a.group_id);
          const gname = g?.name ?? a.group_id.slice(0, 8);
          rows.push({ key: `${a.id}:g`, principal: { kind: "group", id: a.group_id }, name: gname, role, via: from, expires: a.expires_at });
          for (const m of g?.members ?? []) rows.push({ key: `${a.id}:${m}`, principal: { kind: "user", id: m }, name: userName(m), role, via: `via group ${gname}, ${from}`, expires: a.expires_at });
        }
      }
    }
  }
  const users = new Set(rows.filter((r) => r.principal.kind === "user").map((r) => r.principal.id)).size;
  return (
    <Modal open onOpenChange={(o) => !o && onClose()} title="Who has access" description="Pick a resource to see every user and group holding a role there, and why." width={860}>
      <div className="grid gap-3 md:grid-cols-[280px_minmax(0,1fr)]">
        <ScopePicker idx={idx} value={scope} onChange={setScope} height={380} />
        <div className="min-w-0">
          {!scope ? (
            <p className="py-10 text-center text-xs text-muted">Pick a resource on the left.</p>
          ) : (
            <>
              <div className="mb-2 flex min-w-0 items-center gap-2">
                <ScopeLabel idx={idx} scope={{ kind: scope.slice(0, scope.indexOf(":")), id: scope.slice(scope.indexOf(":") + 1) }} />
                <span className="ml-auto shrink-0 font-mono text-[11px] text-faint tabular-nums">
                  {users} user{users === 1 ? "" : "s"} · {rows.length} grant{rows.length === 1 ? "" : "s"}
                </span>
              </div>
              {rows.length === 0 ? (
                <p className="rounded-md border border-line py-8 text-center text-xs text-muted">Nobody holds a role here or above it. Only workspace administrators can act on it.</p>
              ) : (
                <ul className="max-h-[380px] divide-y divide-line overflow-y-auto rounded-md border border-line">
                  {rows.map((r) => (
                    <li key={r.key} className="flex min-w-0 items-center gap-2 px-2.5 py-1.5 text-[13px]">
                      {r.principal.kind === "user" ? <UserRound className="size-3.5 shrink-0 text-faint" /> : <UsersRound className="size-3.5 shrink-0 text-faint" />}
                      <button type="button" onClick={() => onOpen(r.principal)} className="min-w-0 truncate text-left hover:text-accent" title={`Open ${r.name} on the dashboard`}>
                        {r.name}
                      </button>
                      <span className="shrink-0 text-faint">→</span>
                      <Badge tone="accent">{r.role}</Badge>
                      <span className="min-w-0 flex-1 truncate text-xs text-muted" title={r.via}>
                        {r.via}
                      </span>
                      {r.expires && (
                        <span className="shrink-0 text-[11px] text-warn" title={time(r.expires)}>
                          expires {ago(r.expires)}
                        </span>
                      )}
                    </li>
                  ))}
                </ul>
              )}
            </>
          )}
        </div>
      </div>
    </Modal>
  );
}
