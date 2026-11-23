// Right pane for a role: the permission editor (areas with None / Read /
// Write / Admin, actions as checkboxes) and where the role is assigned.
// Edits stage a "publish revision" change; built-in roles are read-only.

import { Archive, Copy, UserRound, UsersRound } from "lucide-react";
import { useState } from "react";

import { useArchiveProfile, type AccessChange, type CatalogAction, type Profile } from "../../api/accessControl";
import { errorText, useToast } from "../../app/toast";
import { Badge, Button, Confirm, ago, time } from "../ui";
import { Tag } from "./AddPopover";
import { publishKey, unassignKey, type AccessModel, type Principal, type Staging } from "./model";
import { PermissionPicker } from "./PermissionPicker";
import { ScopeLabel, type ScopeIndex } from "./scopes";

const same = (a: Set<string>, b: string[]) => a.size === b.length && b.every((x) => a.has(x));

export function RolePane({
  role,
  catalog,
  model,
  idx,
  staging,
  userName,
  onOpen,
  onDuplicate,
  onArchived,
}: {
  role: Profile;
  catalog: CatalogAction[];
  model: AccessModel;
  idx: ScopeIndex;
  staging: Staging;
  userName: (id: string) => string;
  onOpen: (p: Principal) => void;
  onDuplicate: () => void;
  onArchived: () => void;
}) {
  const toast = useToast();
  const archive = useArchiveProfile();
  const [confirming, setConfirming] = useState(false);
  const key = publishKey(role.id);
  const staged = staging.get(key) as Extract<AccessChange, { type: "publish" }> | undefined;
  const selected = new Set(staged?.actions ?? role.actions);
  const added = [...selected].filter((a) => !role.actions.includes(a));
  const removed = role.actions.filter((a) => !selected.has(a));
  const uses = model.assignments.filter((a) => a.profile_id === role.id);

  const change = (next: Set<string>) =>
    same(next, role.actions) ? staging.unstage(key) : staging.stage({ type: "publish", key, profile_id: role.id, actions: [...next].sort(), expected_revision: role.revision });

  return (
    <div className="min-h-0 min-w-0 flex-1 overflow-y-auto">
      <div className="flex flex-wrap items-start gap-2 border-b border-line px-3 py-2">
        <div className="min-w-0 flex-1">
          <div className="flex min-w-0 items-center gap-2">
            <h2 className="truncate font-semibold" title={role.name}>
              {role.name}
            </h2>
            {role.builtin ? <Badge tone="accent">built-in</Badge> : <Badge>custom</Badge>}
            <span className="font-mono text-[11px] text-faint tabular-nums" title={`Updated ${time(role.updated_at)}`}>
              rev {role.revision} · {ago(role.updated_at)}
            </span>
          </div>
          {role.description && <p className="truncate text-xs text-muted" title={role.description}>{role.description}</p>}
        </div>
        <Button onClick={onDuplicate} className="!py-1 text-xs">
          <Copy className="size-3.5" /> Duplicate
        </Button>
        {!role.builtin && (
          <Button variant="ghost" onClick={() => setConfirming(true)} className="!py-1 text-xs hover:!text-bad">
            <Archive className="size-3.5" /> Archive
          </Button>
        )}
      </div>

      {role.builtin && <p className="mx-3 mt-2 rounded bg-panel-2 px-2.5 py-1.5 text-xs text-muted">Built-in roles are read-only. Duplicate it to make an editable copy.</p>}
      {staged && (
        <p className="mx-3 mt-2 flex flex-wrap items-center gap-2 rounded border border-dashed border-accent bg-accent-soft px-2.5 py-1.5 text-xs text-accent">
          Staged: publish revision {role.revision + 1}
          <span className="font-mono font-semibold text-grant-add tabular-nums">+{added.length}</span>
          <span className="font-mono font-semibold text-grant-remove tabular-nums">−{removed.length}</span>
          <span className="text-muted">Review shows who it affects.</span>
          <button type="button" className="ml-auto underline hover:no-underline" onClick={() => staging.unstage(key)}>
            Undo edits
          </button>
        </p>
      )}

      <div className="p-3">
        <PermissionPicker catalog={catalog} selected={selected} onChange={change} readOnly={role.builtin} />
      </div>

      <section className="border-t border-line px-3 py-2">
        <h3 className="mb-1.5 text-[10px] font-semibold tracking-wide text-muted uppercase">
          Assigned <span className="font-mono tabular-nums">{uses.length}</span>
        </h3>
        {uses.length === 0 ? (
          <p className="text-xs text-muted">Not assigned anywhere. Pick a user or group and use + Add on a resource.</p>
        ) : (
          <ul className="divide-y divide-line rounded-md border border-line">
            {uses.map((a) => {
              const p: Principal = a.user_id ? { kind: "user", id: a.user_id } : { kind: "group", id: a.group_id ?? "" };
              const name = p.kind === "user" ? userName(p.id) : (model.groupById.get(p.id)?.name ?? p.id.slice(0, 8));
              const k = unassignKey(a.id);
              const removing = staging.has(k);
              return (
                <li key={a.id} className="flex min-w-0 items-center gap-2 px-2.5 py-1 text-[13px] transition-colors hover:bg-panel-2">
                  {p.kind === "user" ? <UserRound className="size-3.5 shrink-0 text-sky" /> : <UsersRound className="size-3.5 shrink-0 text-teal" />}
                  <button type="button" onClick={() => onOpen(p)} className="max-w-[12rem] shrink-0 truncate text-left hover:text-accent" title={`Open ${name}`}>
                    {name}
                  </button>
                  <span className="shrink-0 text-faint">×</span>
                  <span className="min-w-0 flex-1">
                    <ScopeLabel idx={idx} scope={a.scope} />
                  </span>
                  {a.expires_at && (
                    <span className="shrink-0 rounded-full bg-grant-narrow-soft px-1.5 text-[11px] font-medium text-grant-narrow" title={time(a.expires_at)}>
                      expires {ago(a.expires_at)}
                    </span>
                  )}
                  <Tag state={removing ? "removing" : a.expires_at ? "narrow" : "held"} removeLabel={removing ? "Keep this assignment" : "Stage removing this assignment"} onRemove={() => (removing ? staging.unstage(k) : staging.stage({ type: "unassign", key: k, assignment: a }))}>
                    {removing ? "removing" : "assigned"}
                  </Tag>
                </li>
              );
            })}
          </ul>
        )}
      </section>

      {confirming && (
        <Confirm
          open
          onOpenChange={(o) => !o && !archive.isPending && setConfirming(false)}
          title={`Archive ${role.name}?`}
          body={
            <>
              The role is hidden and can't be assigned again.{" "}
              {uses.length > 0 ? (
                <b className="text-fg">
                  Its {uses.length} assignment{uses.length === 1 ? " is" : "s are"} removed: those users and groups lose what it grants right away.
                </b>
              ) : (
                "It is not assigned anywhere, so nobody loses access."
              )}
            </>
          }
          confirmLabel="Archive role"
          busy={archive.isPending}
          onConfirm={() =>
            archive.mutate(role.id, {
              onSuccess: (r) => {
                toast(`${role.name} archived${r.assignments_unlinked ? `, ${r.assignments_unlinked} assignment(s) removed` : ""}`);
                setConfirming(false);
                onArchived();
              },
              onError: (e) => toast(errorText(e), "error"),
            })
          }
        />
      )}
    </div>
  );
}
