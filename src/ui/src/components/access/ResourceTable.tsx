// Right pane for a user or a group: every resource (the scope tree, flattened)
// with the roles held there as tags. × stages an unassign, "+ Add" stages an
// assignment; inherited roles are muted with where they come from. Expanding a
// row shows the effective actions there.

import { ChevronDown, ChevronRight } from "lucide-react";
import { useMemo, useState } from "react";

import { byArea, useEffectiveAccess, type CatalogAction, type Profile } from "../../api/accessControl";
import { errorText } from "../../app/toast";
import { Button, Empty, Skeleton, Switch, ago, time } from "../ui";
import { AddPopover, EXPIRY_OPTIONS, Tag, TagLegend } from "./AddPopover";
import { assignKey, stagedAssigns, unassignKey, type AccessModel, type Principal, type Staging } from "./model";
import { flatScopes, KIND_LABEL, LevelChip, scopePath, ScopeLink, type ScopeIndex, type ScopeNode } from "./scopes";

const PAGE_SIZES = [25, 50, 100];

interface Props {
  principal: Principal;
  name: string;
  idx: ScopeIndex;
  model: AccessModel;
  staging: Staging;
  roles: Profile[];
  catalog: CatalogAction[];
}

export function ResourceTable(p: Props) {
  const [f, setF] = useState({ q: "", level: "", only: false, page: 0, size: 25 });
  const [open, setOpen] = useState<string | null>(null);
  const all = useMemo(() => flatScopes(p.idx), [p.idx]);
  const needle = f.q.trim().toLowerCase();
  const filtered = !!needle || !!f.level;
  const rows = all.filter(({ node }) => {
    if (f.level && node.kind !== f.level) return false;
    if (needle && !`${scopePath(p.idx, node)} ${node.id}`.toLowerCase().includes(needle)) return false;
    if (f.only && !p.model.reaches(p.principal, node.key) && stagedAssigns(p.staging, p.principal, node.key).length === 0) return false;
    return true;
  });
  const pages = Math.max(1, Math.ceil(rows.length / f.size));
  const page = Math.min(f.page, pages - 1);
  const shown = rows.slice(page * f.size, page * f.size + f.size);
  const set = (x: Partial<typeof f>) => setF((o) => ({ ...o, page: 0, ...x }));

  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col">
      <div className="flex flex-wrap items-center gap-2 border-b border-line px-3 py-2">
        <input
          type="search"
          value={f.q}
          onChange={(e) => set({ q: e.target.value })}
          placeholder="Filter resources…"
          aria-label="Filter resources"
          className="h-7 w-48 min-w-0 flex-1 rounded-md border border-line bg-panel-2 px-2 text-xs outline-none sm:max-w-64"
        />
        <select value={f.level} onChange={(e) => set({ level: e.target.value })} aria-label="Level" className="h-7 rounded-md border border-line bg-panel px-1.5 text-xs">
          <option value="">All levels</option>
          {Object.entries(KIND_LABEL).map(([k, l]) => (
            <option key={k} value={k}>
              {l}
            </option>
          ))}
        </select>
        <Switch checked={f.only} onChange={(only) => set({ only })} label={<span className="text-xs">Only with access</span>} />
      </div>
      <div className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1 border-b border-line bg-panel-2 px-3 py-1.5 text-[11px] text-muted">
        <span className="min-w-0 truncate">
          <b className="font-semibold text-fg">{p.name}</b> roles by resource
        </span>
        <span className="ml-auto" />
        <TagLegend
          states={[
            { state: "held", label: "Granted" },
            { state: "narrow", label: "Expires" },
            { state: "inherited", label: "Inherited" },
            { state: "adding", label: "Staged add" },
            { state: "removing", label: "Staged removal" },
          ]}
        />
      </div>

      <div className="min-h-0 flex-1 overflow-auto">
        {p.idx.loading ? (
          <Skeleton rows={6} />
        ) : rows.length === 0 ? (
          f.only && !filtered ? (
            <Empty title={`${p.name} holds no role anywhere`} action={<Button onClick={() => set({ only: false })}>Browse every resource to add one</Button>}>
              Turn off “Only with access” and use <b>+ Add</b> on a row.
            </Empty>
          ) : (
            <Empty title="No resource matches these filters" action={<Button onClick={() => set({ q: "", level: "" })}>Clear filters</Button>} />
          )
        ) : (
          <table className="w-full table-fixed text-left text-[13px]">
            <thead className="sticky top-0 z-10 bg-panel-2 text-[10px] tracking-wide text-muted uppercase shadow-[inset_0_-1px_var(--color-line)]">
              <tr>
                <th className="w-[38%] px-3 py-2 font-semibold">Resource</th>
                <th className="px-3 py-2 font-semibold">Roles here</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-line">
              {shown.map(({ node, depth }) => (
                <ResourceRow key={node.key} {...p} node={node} depth={filtered ? 0 : depth} fullPath={filtered} open={open === node.key} onToggle={() => setOpen(open === node.key ? null : node.key)} />
              ))}
            </tbody>
          </table>
        )}
      </div>

      <div className="flex flex-wrap items-center gap-2 border-t border-line px-3 py-1.5 text-xs text-muted">
        <span>
          <span className="font-mono tabular-nums">{rows.length}</span> resource{rows.length === 1 ? "" : "s"}
        </span>
        <span className="ml-auto" />
        <label className="flex items-center gap-1">
          Per page
          <select value={f.size} onChange={(e) => set({ size: Number(e.target.value) })} className="h-6 rounded border border-line bg-panel px-1 text-xs">
            {PAGE_SIZES.map((s) => (
              <option key={s}>{s}</option>
            ))}
          </select>
        </label>
        <Button variant="ghost" className="!px-2 !py-0.5 text-xs" disabled={page === 0} onClick={() => setF({ ...f, page: page - 1 })}>
          Previous
        </Button>
        <span className="font-mono tabular-nums">
          {page + 1} / {pages}
        </span>
        <Button variant="ghost" className="!px-2 !py-0.5 text-xs" disabled={page >= pages - 1} onClick={() => setF({ ...f, page: page + 1 })}>
          Next
        </Button>
      </div>
    </div>
  );
}

function ResourceRow({ principal, idx, model, staging, roles, catalog, node, depth, fullPath, open, onToggle }: Props & { node: ScopeNode; depth: number; fullPath: boolean; open: boolean; onToggle: () => void }) {
  const [expiry, setExpiry] = useState("");
  const held = model.heldAt(principal, node.key);
  const direct = held.filter((h) => h.direct);
  const inherited = held.filter((h) => !h.direct);
  const adding = stagedAssigns(staging, principal, node.key);
  const roleName = (id: string) => model.profileById.get(id)?.name ?? id.slice(0, 8);
  const taken = new Set([...direct.map((h) => h.assignment.profile_id), ...adding.map((c) => c.profile_id)]);
  const scope = { kind: node.kind, id: node.id };

  return (
    <>
      <tr className={`transition-colors ${open ? "bg-accent-soft" : "hover:bg-panel-2"}`}>
        <td className="px-3 py-2 align-top">
          <span className="flex min-h-6 min-w-0 items-center gap-1.5" style={{ paddingLeft: depth * 14 }}>
            <button type="button" onClick={onToggle} aria-expanded={open} aria-label={`${open ? "Hide" : "Show"} effective access at ${node.name}`} className={`grid size-5 shrink-0 place-items-center rounded hover:bg-panel-2 hover:text-fg ${open ? "text-accent" : "text-faint"}`}>
              {open ? <ChevronDown className="size-3.5" /> : <ChevronRight className="size-3.5" />}
            </button>
            <LevelChip kind={node.kind} />
            <ScopeLink idx={idx} scope={scope} text={fullPath ? undefined : node.name} />
          </span>
        </td>
        <td className="px-3 py-2 align-top">
          <div className="flex min-h-6 flex-wrap items-center gap-1.5">
            {direct.map((h) => {
              const k = unassignKey(h.assignment.id);
              const removing = staging.has(k);
              const exp = h.assignment.expires_at ? ` · expires ${ago(h.assignment.expires_at)} (${time(h.assignment.expires_at)})` : "";
              return (
                <Tag
                  key={h.assignment.id}
                  state={removing ? "removing" : h.assignment.expires_at ? "narrow" : "held"}
                  title={`${roleName(h.assignment.profile_id)}${exp}`}
                  removeLabel={removing ? `Keep ${roleName(h.assignment.profile_id)}` : `Remove ${roleName(h.assignment.profile_id)} here`}
                  onRemove={() => (removing ? staging.unstage(k) : staging.stage({ type: "unassign", key: k, assignment: h.assignment }))}
                >
                  {roleName(h.assignment.profile_id)}
                  {h.assignment.expires_at ? " ⏱" : ""}
                </Tag>
              );
            })}
            {adding.map((c) => (
              <Tag key={c.key} state="adding" title="Staged: not applied yet" removeLabel={`Drop staged ${roleName(c.profile_id)}`} onRemove={() => staging.unstage(c.key)}>
                {roleName(c.profile_id)}
              </Tag>
            ))}
            {inherited.map((h) => (
              <Tag key={`i:${h.assignment.id}`} state="inherited" title={`${roleName(h.assignment.profile_id)} ${h.via}`}>
                {roleName(h.assignment.profile_id)}
              </Tag>
            ))}
            <AddPopover
              title={`Add a role at ${node.name}`}
              options={roles.map((r) => ({
                id: r.id,
                label: r.name,
                hint: `${r.actions.length}`,
                disabled: taken.has(r.id),
                why: "Already held here",
              }))}
              empty="No roles yet. Create one from Manage."
              onPick={(id) =>
                staging.stage({
                  type: "assign",
                  key: assignKey(principal, id, node.key),
                  ...(principal.kind === "user" ? { user_id: principal.id } : { group_id: principal.id }),
                  profile_id: id,
                  scope: node.key,
                  ...(expiry ? { expires_in_minutes: Number(expiry) } : {}),
                })
              }
              footer={
                <label className="flex items-center gap-1.5 text-[11px] text-muted">
                  Expiry
                  <select value={expiry} onChange={(e) => setExpiry(e.target.value)} className="h-6 flex-1 rounded border border-line bg-panel px-1 text-xs">
                    {EXPIRY_OPTIONS.map((o) => (
                      <option key={o.value} value={o.value}>
                        {o.label}
                      </option>
                    ))}
                  </select>
                </label>
              }
            />
          </div>
        </td>
      </tr>
      {open && (
        <tr className="bg-accent-soft/50">
          <td colSpan={2} className="border-l-2 border-accent px-3 py-2 pl-10">
            {principal.kind === "user" ? (
              <UserEffective user={principal.id} scope={node.key} catalog={catalog} />
            ) : (
              <ActionTags
                ids={[...new Set(held.flatMap((h) => model.profileById.get(h.assignment.profile_id)?.actions ?? []))]}
                catalog={catalog}
                note="Union of the group's roles here and above; members may hold more through other groups."
              />
            )}
          </td>
        </tr>
      )}
    </>
  );
}

function UserEffective({ user, scope, catalog }: { user: string; scope: string; catalog: CatalogAction[] }) {
  const q = useEffectiveAccess(user, scope);
  if (q.isPending) return <span className="text-xs text-faint">Loading effective access…</span>;
  if (q.isError) return <span className="text-xs text-bad">{errorText(q.error)}</span>;
  return <ActionTags ids={q.data.actions.filter((a) => a.allowed).map((a) => a.action)} catalog={catalog} note="Effective here, from every role, group and parent scope (applied changes only)." />;
}

/** Actions grouped by area as small tags, with label and description in the tooltip. */
export function ActionTags({ ids, catalog, note }: { ids: string[]; catalog: CatalogAction[]; note?: string }) {
  const set = new Set(ids);
  const known = catalog.filter((a) => set.has(a.action));
  const unknown = ids.filter((i) => !catalog.some((a) => a.action === i));
  if (ids.length === 0) return <span className="text-xs text-muted">No actions allowed here.</span>;
  return (
    <div className="space-y-1">
      {note && <p className="text-[11px] text-faint">{note}</p>}
      {byArea(known).map(({ area, actions }) => (
        <div key={area} className="flex min-w-0 flex-wrap items-center gap-1">
          <span className="w-28 shrink-0 truncate text-[11px] text-muted" title={area}>
            {area}
          </span>
          {actions.map((a) => (
            <span key={a.action} className="inline-flex h-5 items-center gap-1 rounded-full border border-grant-line bg-grant-soft px-2 font-mono text-[10px] text-grant" title={[a.label, a.description, a.mfa ? "Needs MFA" : ""].filter(Boolean).join("\n")}>
              {a.action}
              {a.mfa && <span className="font-sans font-semibold text-grant-narrow">MFA</span>}
            </span>
          ))}
        </div>
      ))}
      {unknown.length > 0 && <p className="font-mono text-[10px] text-faint">{unknown.join(" ")}</p>}
    </div>
  );
}
