// The Access dashboard: one page for users, groups and roles. Pick a principal
// on the left; on the right see every resource with the roles held there (or,
// for a role, its permissions and where it is assigned). Changes are staged as
// tags, reviewed as sentences and applied as a batch.

import * as DropdownMenu from "@radix-ui/react-dropdown-menu";
import { Link, useNavigate, useSearch } from "@tanstack/react-router";
import { Check, ChevronDown, KeyRound, KeySquare, ListChecks, Loader2, Pencil, Plus, ScanEye, Shield, ShieldCheck, Trash2, UserRound, Users, UsersRound } from "lucide-react";
import { useMemo, useState } from "react";

import { useAccessCatalog, useApplyAccessChanges, useAssignments, useDeleteGroup, useGroups, useProfiles, type AccessChange, type Group, type Profile } from "../api/accessControl";
import { useUsers } from "../api/hooks";
import type { User } from "../api/types";
import { loginOf } from "../api/users";
import { errorText, useToast } from "../app/toast";
import { AddPopover, Tag } from "../components/access/AddPopover";
import { CreateRoleDialog, DuplicateRoleDialog, GroupDialog } from "../components/access/ManageDialogs";
import { memberKey, publishKey, useAccessModel, useStaging, type AccessModel, type Principal, type Staging } from "../components/access/model";
import { ResourceTable } from "../components/access/ResourceTable";
import { ReviewDialog, type ChangeText } from "../components/access/ReviewDialog";
import { RolePane } from "../components/access/RolePane";
import { scopePath, useScopeIndex } from "../components/access/scopes";
import { WhoHasAccess } from "../components/access/WhoHasAccess";
import { Badge, Button, Confirm, Empty, ErrorNote, Skeleton } from "../components/ui";

export type AccessView = "users" | "groups" | "roles";
export interface AccessSearch {
  view?: AccessView;
  id?: string;
}

type Dialog =
  | { kind: "review" }
  | { kind: "who" }
  | { kind: "newRole" }
  | { kind: "dupRole"; role: Profile }
  | { kind: "group"; group?: Group }
  | { kind: "deleteGroup"; group: Group };

const menuItem = "flex cursor-pointer items-center gap-2 rounded px-2.5 py-1.5 text-[13px] outline-none select-none data-[highlighted]:bg-panel-2";

export function Access() {
  const search = useSearch({ from: "/access" });
  const view: AccessView = search.view ?? "users";
  const navigate = useNavigate();
  const select = (v: AccessView, id?: string) => void navigate({ to: "/access", search: { view: v, ...(id ? { id } : {}) }, replace: true });
  const open = (p: Principal) => {
    setDialog(null);
    select(p.kind === "user" ? "users" : "groups", p.id);
  };

  const users = useUsers();
  const groups = useGroups();
  const profiles = useProfiles();
  const assignments = useAssignments();
  const catalog = useAccessCatalog();
  const idx = useScopeIndex();
  const staging = useStaging();
  const apply = useApplyAccessChanges();
  const toast = useToast();
  const del = useDeleteGroup();
  const [dialog, setDialog] = useState<Dialog | null>(null);

  const userList = useMemo(() => (Array.isArray(users.data) ? users.data.filter((u): u is User => !!u && typeof u.id === "string") : []), [users.data]);
  const groupList = useMemo(() => groups.data ?? [], [groups.data]);
  const roleList = useMemo(() => (profiles.data ?? []).filter((p) => p.archived_at === null).sort((a, b) => Number(b.builtin) - Number(a.builtin) || a.name.localeCompare(b.name)), [profiles.data]);
  const model = useAccessModel({ assignments: assignments.data ?? [], groups: groupList, profiles: profiles.data ?? [], idx });
  const userById = useMemo(() => new Map(userList.map((u) => [u.id, u])), [userList]);
  const userName = (id: string) => {
    const u = userById.get(id);
    return u ? u.display_name || loginOf(u) : id.slice(0, 8);
  };
  const groupName = (id: string) => model.groupById.get(id)?.name ?? id.slice(0, 8);
  const roleName = (id: string) => model.profileById.get(id)?.name ?? id.slice(0, 8);

  const describe = (c: AccessChange): ChangeText => {
    const who = (u?: string | null, g?: string | null) => (u ? userName(u) : g ? `group ${groupName(g)}` : "—");
    const path = (k: string) => {
      const i = k.indexOf(":");
      return scopePath(idx, { kind: k.slice(0, i), id: k.slice(i + 1) });
    };
    switch (c.type) {
      case "assign": {
        const n = model.profileById.get(c.profile_id)?.actions.length ?? 0;
        return {
          title: `Grant ${roleName(c.profile_id)} to ${who(c.user_id, c.group_id)} at ${path(c.scope)}`,
          detail: `${who(c.user_id, c.group_id)} may run its ${n} action${n === 1 ? "" : "s"} there and on everything below it${c.expires_in_minutes ? `, for ${Math.round(c.expires_in_minutes / 60)} h` : ""}.`,
        };
      }
      case "unassign":
        return {
          title: `Remove ${roleName(c.assignment.profile_id)} from ${who(c.assignment.user_id, c.assignment.group_id)} at ${scopePath(idx, c.assignment.scope)}`,
          detail: "They lose what this role grants there, unless another role or group still grants it.",
        };
      case "member": {
        const n = model.directOf({ kind: "group", id: c.group }).length;
        return {
          title: `${c.add ? "Add" : "Remove"} ${userName(c.user)} ${c.add ? "to" : "from"} group ${groupName(c.group)}`,
          detail: `${c.add ? "Gains" : "Loses"} the group's ${n} role assignment${n === 1 ? "" : "s"}.`,
        };
      }
      case "publish":
        return { title: `Publish revision ${c.expected_revision + 1} of ${roleName(c.profile_id)}`, detail: `${c.actions.length} actions in the new revision; applies to every assignment of the role.` };
    }
  };

  const applyAll = () =>
    apply.mutate(staging.changes, {
      onSuccess: (results) => {
        staging.settle(results);
        const ok = results.filter((r) => r.ok).length;
        const bad = results.length - ok;
        if (bad === 0) toast(`${ok} change${ok === 1 ? "" : "s"} applied`);
        else toast(`${ok} applied, ${bad} refused`, "error", { label: "See why", onSelect: () => setDialog({ kind: "review" }) });
      },
      onError: (e) => toast(errorText(e), "error"),
    });

  const coreError = assignments.error ?? groups.error ?? profiles.error;
  const loading = assignments.isPending || groups.isPending || profiles.isPending;
  const n = staging.changes.length;
  const failed = Object.keys(staging.errors).length;
  const selRole = view === "roles" ? roleList.find((r) => r.id === search.id) : undefined;
  const selGroup = view === "groups" ? groupList.find((g) => g.id === search.id) : undefined;
  const selUser = view === "users" ? userById.get(search.id ?? "") : undefined;

  return (
    <div className="flex min-w-0 flex-col gap-3">
      <header className="flex flex-wrap items-center justify-between gap-3">
        <div className="flex min-w-0 items-center gap-3">
          <span className="grid size-7 shrink-0 place-items-center rounded-md bg-accent-soft text-accent" aria-hidden>
            <ShieldCheck className="size-[18px]" />
          </span>
          <div className="min-w-0">
            <h1 className="text-lg leading-6 font-semibold tracking-tight">Access</h1>
            <p className="mt-0.5 text-xs text-muted">Who can do what, where. Stage changes on the tags, review them, then apply them together.</p>
          </div>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <DropdownMenu.Root modal={false}>
            <DropdownMenu.Trigger asChild>
              <Button>
                Manage <ChevronDown className="size-3.5" />
              </Button>
            </DropdownMenu.Trigger>
            <DropdownMenu.Portal>
              <DropdownMenu.Content align="end" sideOffset={4} className="animate-in z-50 min-w-[200px] rounded-lg border border-line bg-panel p-1 shadow-[var(--shadow-pop)]">
                <DropdownMenu.Label className="px-2.5 pt-1 pb-0.5 text-[10px] tracking-wide text-faint uppercase">Roles</DropdownMenu.Label>
                <DropdownMenu.Item className={menuItem} onSelect={() => select("roles")}>
                  <KeySquare className="size-4 opacity-80" /> All roles
                </DropdownMenu.Item>
                <DropdownMenu.Item className={menuItem} onSelect={() => setDialog({ kind: "newRole" })}>
                  <Plus className="size-4 opacity-80" /> New role
                </DropdownMenu.Item>
                <DropdownMenu.Separator className="my-1 h-px bg-line" />
                <DropdownMenu.Label className="px-2.5 pt-1 pb-0.5 text-[10px] tracking-wide text-faint uppercase">Groups</DropdownMenu.Label>
                <DropdownMenu.Item className={menuItem} onSelect={() => select("groups")}>
                  <UsersRound className="size-4 opacity-80" /> All groups
                </DropdownMenu.Item>
                <DropdownMenu.Item className={menuItem} onSelect={() => setDialog({ kind: "group" })}>
                  <Plus className="size-4 opacity-80" /> New group
                </DropdownMenu.Item>
                <DropdownMenu.Separator className="my-1 h-px bg-line" />
                <DropdownMenu.Item className={menuItem} onSelect={() => setDialog({ kind: "who" })}>
                  <ScanEye className="size-4 opacity-80" /> Who has access…
                </DropdownMenu.Item>
                <DropdownMenu.Item className={menuItem} asChild>
                  <Link to="/members">
                    <Users className="size-4 opacity-80" /> Members (invite, disable)
                  </Link>
                </DropdownMenu.Item>
                <DropdownMenu.Item className={menuItem} asChild>
                  <Link to="/profile/$section" params={{ section: "tokens" }}>
                    <KeyRound className="size-4 opacity-80" /> Tokens (personal API keys)
                  </Link>
                </DropdownMenu.Item>
              </DropdownMenu.Content>
            </DropdownMenu.Portal>
          </DropdownMenu.Root>
          <Button onClick={() => setDialog({ kind: "review" })} disabled={n === 0} title={n === 0 ? "Nothing staged yet" : undefined}>
            <ListChecks className="size-4" /> Review
            <span className={`inline-grid h-5 min-w-5 place-items-center rounded-full px-1.5 font-mono text-[11px] font-bold tabular-nums ${n ? "bg-accent-soft text-accent" : "bg-panel-2 text-faint"}`}>{n}</span>
            {failed > 0 && <Badge tone="bad">{failed} refused</Badge>}
          </Button>
          <Button variant="primary" onClick={applyAll} disabled={n === 0 || apply.isPending} title={n === 0 ? "Nothing staged yet" : `Apply ${n} change(s) in order`}>
            {apply.isPending ? <Loader2 className="size-4 animate-spin" /> : <Check className="size-4" />} Apply changes
          </Button>
        </div>
      </header>

      <div className="flex flex-wrap items-center gap-x-3 gap-y-2 rounded-lg border border-line bg-panel px-3 py-2 shadow-[var(--shadow-card)]">
        <span className="text-[10px] font-semibold tracking-wide text-muted uppercase">Principal</span>
        <div className="inline-flex rounded-md border border-line bg-panel-2 p-0.5" role="tablist" aria-label="Principal kind">
          {(
            [
              ["users", "Users", UserRound],
              ["groups", "Groups", UsersRound],
              ["roles", "Roles", Shield],
            ] as const
          ).map(([v, label, Icon]) => (
            <button
              key={v}
              type="button"
              role="tab"
              aria-selected={view === v}
              onClick={() => select(v)}
              className={`inline-flex h-6 items-center gap-1.5 rounded px-2.5 text-xs font-semibold transition ${view === v ? "bg-panel text-fg shadow-[var(--shadow-card)]" : "text-muted hover:text-fg"}`}
            >
              <Icon className={`size-3.5 ${view === v ? KIND_TEXT[v] : ""}`} /> {label}
            </button>
          ))}
        </div>
        <span className="hidden h-6 w-px bg-line sm:block" aria-hidden />
        <Kpi label="roles" value={profiles.isError ? undefined : profiles.data && roleList.length} />
        <Kpi label="groups" value={groups.isError ? undefined : groups.data && groupList.length} />
        <Kpi label="users" value={users.isError ? undefined : users.data && userList.length} />
        <Kpi label="assignments" value={assignments.isError ? undefined : assignments.data?.length} />
        {n > 0 && (
          <span className="ml-auto inline-flex items-center gap-1.5 rounded-full bg-warn-soft px-2.5 py-0.5 text-[11px] font-semibold text-warn">
            <span className="size-1.5 rounded-full bg-current" aria-hidden />
            {n} pending change{n === 1 ? "" : "s"}
          </span>
        )}
      </div>

      {coreError ? (
        <ErrorNote error={coreError} />
      ) : loading ? (
        <Skeleton rows={6} />
      ) : (
        <div className="grid min-w-0 gap-3 md:h-[calc(100vh-15rem)] md:min-h-[480px] md:grid-cols-[17rem_minmax(0,1fr)]">
          <PrincipalPane
            view={view}
            selectedId={search.id}
            onSelect={(id) => select(view, id)}
            users={users}
            userList={userList}
            groupList={groupList}
            roleList={roleList}
            model={model}
            staging={staging}
            userName={userName}
            onNew={() => setDialog(view === "roles" ? { kind: "newRole" } : { kind: "group" })}
            onEditGroup={(g) => setDialog({ kind: "group", group: g })}
            onDeleteGroup={(g) => setDialog({ kind: "deleteGroup", group: g })}
          />
          <section className="flex min-h-[420px] min-w-0 flex-col overflow-hidden rounded-lg border border-line bg-panel shadow-[var(--shadow-card)]" aria-label="Access detail">
            {catalog.isError && <ErrorNote error={catalog.error} />}
            {selRole ? (
              <RolePane
                key={selRole.id}
                role={selRole}
                catalog={catalog.data?.actions ?? []}
                model={model}
                idx={idx}
                staging={staging}
                userName={userName}
                onOpen={open}
                onDuplicate={() => setDialog({ kind: "dupRole", role: selRole })}
                onArchived={() => select("roles")}
              />
            ) : selUser || selGroup ? (
              <>
                <div className="flex min-w-0 items-center gap-2 border-b border-line px-3 py-2">
                  <Avatar kind={view} name={selUser ? userName(selUser.id) : (selGroup?.name ?? "")} />
                  <h2 className="truncate font-semibold">{selUser ? userName(selUser.id) : selGroup?.name}</h2>
                  {selUser && <span className="truncate font-mono text-[11px] text-faint select-all">{loginOf(selUser)}</span>}
                  {selUser?.disabled && <Badge tone="warn">disabled</Badge>}
                  <span className="ml-auto shrink-0 font-mono text-[11px] text-faint tabular-nums">
                    {model.allOf(selUser ? { kind: "user", id: selUser.id } : { kind: "group", id: selGroup?.id ?? "" }).length} assignment(s)
                  </span>
                </div>
                <ResourceTable
                  key={`${view}:${search.id}`}
                  principal={selUser ? { kind: "user", id: selUser.id } : { kind: "group", id: selGroup?.id ?? "" }}
                  name={selUser ? userName(selUser.id) : (selGroup?.name ?? "")}
                  idx={idx}
                  model={model}
                  staging={staging}
                  roles={roleList}
                  catalog={catalog.data?.actions ?? []}
                />
              </>
            ) : (
              <Empty icon={view === "roles" ? <Shield className="size-5" /> : view === "groups" ? <UsersRound className="size-5" /> : <UserRound className="size-5" />} title={`Pick a ${view.slice(0, -1)}`}>
                {view === "roles"
                  ? "See and edit a role's permissions and where it is assigned."
                  : view === "groups"
                    ? "See the group's roles on every resource and manage its members."
                    : "See what a person can do on every resource, and why."}
                {search.id && <span className="mt-1 block text-xs text-warn">The {view.slice(0, -1)} in the link no longer exists or is hidden from you.</span>}
              </Empty>
            )}
          </section>
        </div>
      )}

      {dialog?.kind === "review" && <ReviewDialog staging={staging} describe={describe} onClose={() => setDialog(null)} />}
      {dialog?.kind === "who" && <WhoHasAccess idx={idx} model={model} userName={userName} onOpen={open} onClose={() => setDialog(null)} />}
      {dialog?.kind === "newRole" && (
        <CreateRoleDialog
          onClose={() => setDialog(null)}
          onCreated={(p) => {
            setDialog(null);
            select("roles", p.id);
          }}
        />
      )}
      {dialog?.kind === "dupRole" && (
        <DuplicateRoleDialog
          role={dialog.role}
          onClose={() => setDialog(null)}
          onCreated={(p) => {
            setDialog(null);
            select("roles", p.id);
          }}
        />
      )}
      {dialog?.kind === "group" && (
        <GroupDialog
          group={dialog.group}
          onClose={() => setDialog(null)}
          onSaved={(g) => {
            setDialog(null);
            select("groups", g.id);
          }}
        />
      )}
      {dialog?.kind === "deleteGroup" && (
        <Confirm
          open
          onOpenChange={(o) => !o && !del.isPending && setDialog(null)}
          title={`Delete group ${dialog.group.name}?`}
          body={(() => {
            const a = model.directOf({ kind: "group", id: dialog.group.id }).length;
            const m = dialog.group.members.length;
            return `Its ${m} member${m === 1 ? "" : "s"} lose the group's ${a} role assignment${a === 1 ? "" : "s"} right away, unless another group or a direct role grants the same. The users themselves stay.`;
          })()}
          confirmLabel="Delete group"
          busy={del.isPending}
          onConfirm={() =>
            del.mutate(dialog.group.id, {
              onSuccess: () => {
                toast(`Group ${dialog.group.name} deleted`);
                setDialog(null);
                select("groups");
              },
              onError: (e) => toast(errorText(e), "error"),
            })
          }
        />
      )}
    </div>
  );
}

/** Hue per principal kind: users sky, groups teal, roles violet. */
const KIND_TEXT: Record<AccessView, string> = { users: "text-sky", groups: "text-teal", roles: "text-violet" };
const KIND_AVATAR: Record<AccessView, string> = { users: "bg-sky-soft text-sky", groups: "bg-teal-soft text-teal", roles: "bg-violet-soft text-violet" };

const initials = (name: string) => {
  const w = name.trim().split(/[\s._@-]+/).filter(Boolean);
  return ((w[0]?.[0] ?? "") + (w.length > 1 ? (w[1]?.[0] ?? "") : (w[0]?.[1] ?? ""))).toUpperCase() || "?";
};

/** Rounded-square initials, coloured by principal kind; solid accent when selected. */
function Avatar({ kind, name, active }: { kind: AccessView; name: string; active?: boolean }) {
  return (
    <span aria-hidden className={`grid size-7 shrink-0 place-items-center rounded-md text-[10px] font-bold ${active ? "bg-accent text-panel" : KIND_AVATAR[kind]}`}>
      {initials(name)}
    </span>
  );
}

function Kpi({ label, value }: { label: string; value: number | undefined }) {
  return (
    <span className="inline-flex items-baseline gap-1 text-xs text-muted">
      <span className="font-mono text-[15px] font-bold text-fg tabular-nums">{value ?? "—"}</span>
      {label}
    </span>
  );
}

// ------------------------------------------------------------------ left pane

function PrincipalPane({
  view,
  selectedId,
  onSelect,
  users,
  userList,
  groupList,
  roleList,
  model,
  staging,
  userName,
  onNew,
  onEditGroup,
  onDeleteGroup,
}: {
  view: AccessView;
  selectedId?: string;
  onSelect: (id: string) => void;
  users: ReturnType<typeof useUsers>;
  userList: User[];
  groupList: Group[];
  roleList: Profile[];
  model: AccessModel;
  staging: Staging;
  userName: (id: string) => string;
  onNew: () => void;
  onEditGroup: (g: Group) => void;
  onDeleteGroup: (g: Group) => void;
}) {
  const [q, setQ] = useState("");
  const needle = q.trim().toLowerCase();
  type Item = { id: string; name: string; sub: string; count: number; countTitle: string; badge?: React.ReactNode; dirty?: boolean };
  const items: Item[] =
    view === "users"
      ? userList.map((u) => ({
          id: u.id,
          name: u.display_name || loginOf(u),
          sub: `${model.groupsOf(u.id).length} group(s)${u.disabled ? " · disabled" : ""}`,
          count: model.directOf({ kind: "user", id: u.id }).length,
          countTitle: "Direct role assignments",
          badge: u.disabled ? <Badge tone="warn">off</Badge> : undefined,
          dirty: staging.changes.some((c) => (c.type === "assign" && c.user_id === u.id) || (c.type === "unassign" && c.assignment.user_id === u.id) || (c.type === "member" && c.user === u.id)),
        }))
      : view === "groups"
        ? groupList.map((g) => ({
            id: g.id,
            name: g.name,
            sub: g.description || "No description",
            count: g.members.length,
            countTitle: "Members",
            dirty: staging.changes.some((c) => (c.type === "assign" && c.group_id === g.id) || (c.type === "unassign" && c.assignment.group_id === g.id) || (c.type === "member" && c.group === g.id)),
          }))
        : roleList.map((r) => ({
            id: r.id,
            name: r.name,
            sub: r.description || `rev ${r.revision}`,
            count: r.actions.length,
            countTitle: "Actions",
            badge: r.builtin ? <Badge tone="accent">built-in</Badge> : undefined,
            dirty: staging.has(publishKey(r.id)),
          }));
  const shown = (needle ? items.filter((i) => `${i.name} ${i.sub} ${i.id}`.toLowerCase().includes(needle)) : items).sort((a, b) => (view === "roles" ? 0 : a.name.localeCompare(b.name)));
  const listError = view === "users" && users.isError;
  const selGroup = view === "groups" ? groupList.find((g) => g.id === selectedId) : undefined;
  const selUser = view === "users" ? userList.find((u) => u.id === selectedId) : undefined;
  const noun = view.slice(0, -1);

  return (
    <aside className="flex max-h-[70vh] min-h-0 min-w-0 flex-col overflow-hidden rounded-lg border border-line bg-panel shadow-[var(--shadow-card)] md:max-h-none" aria-label={`${view}`}>
      <div className="flex items-center gap-1.5 border-b border-line p-2">
        <input
          type="search"
          value={q}
          onChange={(e) => setQ(e.target.value)}
          placeholder={`Search ${view}…`}
          aria-label={`Search ${view}`}
          className="h-7 min-w-0 flex-1 rounded-md border border-line bg-panel-2 px-2 text-xs outline-none"
        />
        {view === "users" ? (
          <Link to="/members" className="grid size-7 shrink-0 place-items-center rounded-md border border-line text-muted hover:border-line-strong hover:text-fg" title="Invite or create users on Members" aria-label="Invite users on Members">
            <Plus className="size-4" />
          </Link>
        ) : (
          <button type="button" onClick={onNew} className="grid size-7 shrink-0 place-items-center rounded-md border border-line text-muted hover:border-line-strong hover:text-fg" title={`New ${noun}`} aria-label={`New ${noun}`}>
            <Plus className="size-4" />
          </button>
        )}
      </div>
      <div className="px-3 pt-2 pb-1 text-[10px] font-semibold tracking-wide text-faint uppercase">
        {view} <span className="font-mono tabular-nums">{shown.length}</span>
      </div>
      <ul className="flex min-h-0 flex-1 flex-col gap-0.5 overflow-y-auto px-1.5 pb-1.5" role="listbox" aria-label={view}>
        {listError ? (
          <li>
            <ErrorNote error={users.error} />
          </li>
        ) : view === "users" && users.isPending ? (
          <li>
            <Skeleton rows={4} />
          </li>
        ) : shown.length === 0 ? (
          <li className="px-3 py-4 text-center text-xs text-muted">{items.length ? "No match." : view === "groups" ? "No groups yet. Create one with +." : view === "roles" ? "No roles." : "No users."}</li>
        ) : (
          shown.map((i) => {
            const active = i.id === selectedId;
            return (
              <li key={i.id} role="option" aria-selected={active}>
                <button
                  type="button"
                  onClick={() => onSelect(i.id)}
                  className={`relative flex w-full min-w-0 items-center gap-2 rounded-md px-2 py-1.5 text-left transition-colors ${
                    active
                      ? "bg-[color-mix(in_srgb,var(--color-accent)_14%,var(--color-panel))] shadow-[inset_0_0_0_1px_color-mix(in_srgb,var(--color-accent)_35%,transparent)] before:absolute before:inset-y-1.5 before:left-0 before:w-[3px] before:rounded-full before:bg-accent"
                      : "hover:bg-panel-2"
                  }`}
                >
                  <Avatar kind={view} name={i.name} active={active} />
                  <span className="min-w-0 flex-1">
                    <span className={`flex min-w-0 items-center gap-1.5 text-[13px] ${active ? "font-semibold text-fg" : "font-medium"}`}>
                      <span className="truncate" title={i.name}>
                        {i.name}
                      </span>
                      {i.badge}
                      {i.dirty && <span className="size-1.5 shrink-0 rounded-full bg-accent" title="Has staged changes" />}
                    </span>
                    <span className="block truncate text-[11px] text-faint" title={i.sub}>
                      {i.sub}
                    </span>
                  </span>
                  <span className={`min-w-6 shrink-0 rounded-full px-1.5 text-center font-mono text-[10px] font-semibold tabular-nums ${active ? "bg-panel text-accent" : "bg-panel-2 text-muted"}`} title={i.countTitle}>
                    {i.count}
                  </span>
                </button>
              </li>
            );
          })
        )}
      </ul>
      {selUser && <UserGroupsFooter user={selUser} groupList={groupList} staging={staging} />}
      {selGroup && <GroupMembersFooter group={selGroup} userList={userList} staging={staging} userName={userName} onEdit={() => onEditGroup(selGroup)} onDelete={() => onDeleteGroup(selGroup)} />}
    </aside>
  );
}

/** Member tags for one membership pair: applied (× stages removal), staged add or staged removal (↺ undoes). */
function memberTags(staging: Staging, pairs: { group: string; user: string; label: string; member: boolean }[]) {
  return pairs.map(({ group, user, label, member }) => {
    const rm = memberKey(group, user, false);
    const add = memberKey(group, user, true);
    if (member) {
      const removing = staging.has(rm);
      return (
        <Tag
          key={`${group}:${user}`}
          state={removing ? "removing" : "held"}
          removeLabel={removing ? `Keep ${label}` : `Remove ${label}`}
          onRemove={() => (removing ? staging.unstage(rm) : staging.stage({ type: "member", key: rm, group, user, add: false }))}
        >
          {label}
        </Tag>
      );
    }
    return (
      <Tag key={`${group}:${user}`} state="adding" title="Staged: not applied yet" removeLabel={`Drop staged ${label}`} onRemove={() => staging.unstage(add)}>
        {label}
      </Tag>
    );
  });
}

function UserGroupsFooter({ user, groupList, staging }: { user: User; groupList: Group[]; staging: Staging }) {
  const pairs = groupList.flatMap((g) => {
    const member = g.members.includes(user.id);
    if (member || staging.has(memberKey(g.id, user.id, true))) return [{ group: g.id, user: user.id, label: g.name, member }];
    return [];
  });
  const options = groupList
    .filter((g) => !pairs.some((p) => p.group === g.id))
    .map((g) => ({ id: g.id, label: g.name, hint: `${g.members.length}` }));
  return (
    <div className="border-t border-line bg-panel-2 px-2.5 py-2">
      <div className="mb-1 text-[10px] font-semibold tracking-wide text-faint uppercase">Groups</div>
      <div className="flex flex-wrap items-center gap-1">
        {pairs.length === 0 && <span className="text-[11px] text-muted">In no group.</span>}
        {memberTags(staging, pairs)}
        <AddPopover
          label="Add to group"
          title="Add to group"
          options={options}
          empty="No other groups. Create one from Manage."
          onPick={(g) => staging.stage({ type: "member", key: memberKey(g, user.id, true), group: g, user: user.id, add: true })}
        />
      </div>
    </div>
  );
}

function GroupMembersFooter({ group, userList, staging, userName, onEdit, onDelete }: { group: Group; userList: User[]; staging: Staging; userName: (id: string) => string; onEdit: () => void; onDelete: () => void }) {
  const staged = staging.changes.flatMap((c) => (c.type === "member" && c.add && c.group === group.id && !group.members.includes(c.user) ? [c.user] : []));
  const pairs = [...group.members.map((u) => ({ group: group.id, user: u, label: userName(u), member: true })), ...staged.map((u) => ({ group: group.id, user: u, label: userName(u), member: false }))];
  const options = userList.filter((u) => !group.members.includes(u.id) && !staged.includes(u.id)).map((u) => ({ id: u.id, label: u.display_name || loginOf(u), hint: u.display_name ? loginOf(u) : undefined }));
  return (
    <div className="max-h-48 overflow-y-auto border-t border-line bg-panel-2 px-2.5 py-2">
      <div className="mb-1 flex items-center gap-1 text-[10px] font-semibold tracking-wide text-faint uppercase">
        Members <span className="font-mono tabular-nums">{group.members.length}</span>
        <span className="ml-auto" />
        <button type="button" onClick={onEdit} className="rounded p-0.5 text-faint hover:text-fg" aria-label={`Rename ${group.name}`} title="Rename">
          <Pencil className="size-3.5" />
        </button>
        <button type="button" onClick={onDelete} className="rounded p-0.5 text-faint hover:text-bad" aria-label={`Delete ${group.name}`} title="Delete group">
          <Trash2 className="size-3.5" />
        </button>
      </div>
      <div className="flex flex-wrap items-center gap-1">
        {pairs.length === 0 && <span className="text-[11px] text-muted">No members.</span>}
        {memberTags(staging, pairs)}
        <AddPopover label="Add member" title={`Add a member to ${group.name}`} options={options} empty="Everyone is already a member." onPick={(u) => staging.stage({ type: "member", key: memberKey(group.id, u, true), group: group.id, user: u, add: true })} />
      </div>
    </div>
  );
}
