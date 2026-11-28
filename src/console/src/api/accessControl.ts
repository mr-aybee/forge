// Access control: the permission catalog, roles (access profiles), user
// groups, role assignments and effective access (`/api/v1/access/*`).
// Every hook returns normalized data: an older server without an endpoint
// yields an error (shown by the page), never a crash on a missing field.

import { useQuery, type QueryKey } from "@tanstack/react-query";

import { ApiError, api, get, post } from "./client";
import { useAction } from "./hooks";

const enc = encodeURIComponent;
const BASE = "/api/v1/access";

// ------------------------------------------------------------------ types

export type AccessLevel = "read" | "write" | "admin";
export const LEVELS: AccessLevel[] = ["read", "write", "admin"];

/** One permission in the catalog. */
export interface CatalogAction {
  action: string;
  area: string;
  label: string;
  description: string;
  level: AccessLevel;
  mfa: boolean;
  /** Built-in roles that include it. */
  roles: string[];
}

export interface AccessCatalog {
  actions: CatalogAction[];
  scopes: string[];
}

/** Scope kinds an assignment may target. */
export type ScopeKind = "workspace" | "cluster" | "environment" | "server" | "service" | "project";
export const SCOPE_KINDS: ScopeKind[] = ["workspace", "project", "environment", "service", "cluster", "server"];

export interface ScopeRef {
  kind: string;
  id: string;
}

/** A role (API: access profile): a named, revisioned list of actions. */
export interface Profile {
  id: string;
  name: string;
  description: string;
  builtin: boolean;
  revision: number;
  archived_at: number | null;
  created_by: string;
  created_at: number;
  updated_at: number;
  actions: string[];
}

export interface ProfileRevision {
  revision: number;
  actions: string[];
  created_by: string;
  created_at: number;
}

export interface ProfileDetail extends Profile {
  revisions: ProfileRevision[];
}

export interface PublishImpact {
  from_revision: number;
  added: string[];
  removed: string[];
  assignments: number;
  users: number;
  blocked: { assignment: string; scope: string; missing: string[] }[];
}

export interface Assignment {
  id: string;
  user_id: string | null;
  group_id: string | null;
  profile_id: string;
  scope: ScopeRef;
  expires_at: number | null;
  created_by: string;
  created_at: number;
}

export interface Group {
  id: string;
  name: string;
  description: string;
  members: string[];
  created_by: string;
  created_at: number;
}

export interface EffectiveSource {
  assignment_id: string;
  profile_id: string;
  profile_name: string;
  scope: ScopeRef;
  via: "direct" | "group";
  group_id: string | null;
  group_name: string | null;
}

export interface EffectiveAccess {
  user_id: string;
  scope: string;
  actions: { action: string; allowed: boolean }[];
  sources: EffectiveSource[];
}

// ------------------------------------------------------------------ normalizers

type Obj = Record<string, unknown>;
const obj = (x: unknown): Obj | null => (x && typeof x === "object" && !Array.isArray(x) ? (x as Obj) : null);
const str = (v: unknown, d = ""): string => (typeof v === "string" ? v : d);
const optStr = (v: unknown): string | null => (typeof v === "string" && v ? v : null);
const num = (v: unknown, d = 0): number => (typeof v === "number" && Number.isFinite(v) ? v : d);
const optNum = (v: unknown): number | null => (typeof v === "number" && Number.isFinite(v) ? v : null);
const strs = (v: unknown): string[] => (Array.isArray(v) ? v.filter((x): x is string => typeof x === "string") : []);
const arr = (v: unknown): unknown[] => (Array.isArray(v) ? v : []);
const level = (v: unknown): AccessLevel => (v === "write" || v === "admin" ? v : "read");

function toScope(v: unknown): ScopeRef {
  if (typeof v === "string") {
    const i = v.indexOf(":");
    return i > 0 ? { kind: v.slice(0, i), id: v.slice(i + 1) } : { kind: v, id: "" };
  }
  const o = obj(v);
  return { kind: str(o?.kind), id: str(o?.id) };
}

/** `kind:id`, the form the API takes. */
export const scopeKey = (s: ScopeRef) => `${s.kind}:${s.id}`;

function toCatalog(x: unknown): AccessCatalog {
  const o = obj(x);
  return {
    actions: arr(o?.actions).flatMap((a) => {
      const r = obj(a);
      const action = str(r?.action);
      if (!r || !action) return [];
      return [{ action, area: str(r.area, "Other") || "Other", label: str(r.label, action) || action, description: str(r.description), level: level(r.level), mfa: r.mfa === true, roles: strs(r.roles) }];
    }),
    scopes: strs(o?.scopes),
  };
}

function toProfile(x: unknown): Profile | null {
  const o = obj(x);
  const id = str(o?.id);
  if (!o || !id) return null;
  return {
    id,
    name: str(o.name, id) || id,
    description: str(o.description),
    builtin: o.builtin === true,
    revision: num(o.revision, 1),
    archived_at: optNum(o.archived_at),
    created_by: str(o.created_by),
    created_at: num(o.created_at),
    updated_at: num(o.updated_at),
    actions: strs(o.actions),
  };
}

function toProfileDetail(x: unknown): ProfileDetail | null {
  const p = toProfile(x);
  if (!p) return null;
  const o = obj(x);
  const revisions = arr(o?.revisions).flatMap((r) => {
    const v = obj(r);
    if (!v) return [];
    return [{ revision: num(v.revision), actions: strs(v.actions), created_by: str(v.created_by), created_at: num(v.created_at) }];
  });
  return { ...p, revisions: revisions.sort((a, b) => b.revision - a.revision) };
}

function toImpact(x: unknown): PublishImpact {
  const o = obj(x);
  return {
    from_revision: num(o?.from_revision),
    added: strs(o?.added),
    removed: strs(o?.removed),
    assignments: num(o?.assignments),
    users: num(o?.users),
    blocked: arr(o?.blocked).flatMap((b) => {
      const v = obj(b);
      return v ? [{ assignment: str(v.assignment), scope: typeof v.scope === "string" ? v.scope : scopeKey(toScope(v.scope)), missing: strs(v.missing) }] : [];
    }),
  };
}

function toAssignment(x: unknown): Assignment | null {
  const o = obj(x);
  const id = str(o?.id);
  if (!o || !id) return null;
  return {
    id,
    user_id: optStr(o.user_id),
    group_id: optStr(o.group_id),
    profile_id: str(o.profile_id),
    scope: toScope(o.scope),
    expires_at: optNum(o.expires_at),
    created_by: str(o.created_by),
    created_at: num(o.created_at),
  };
}

function toGroup(x: unknown): Group | null {
  const o = obj(x);
  const id = str(o?.id);
  if (!o || !id) return null;
  return { id, name: str(o.name, id) || id, description: str(o.description), members: strs(o.members), created_by: str(o.created_by), created_at: num(o.created_at) };
}

function toEffective(x: unknown): EffectiveAccess {
  const o = obj(x);
  return {
    user_id: str(o?.user_id),
    scope: str(o?.scope),
    actions: arr(o?.actions).flatMap((a) => {
      const v = obj(a);
      return v && typeof v.action === "string" ? [{ action: v.action, allowed: v.allowed === true }] : [];
    }),
    sources: arr(o?.sources).flatMap((s) => {
      const v = obj(s);
      if (!v) return [];
      return [
        {
          assignment_id: str(v.assignment_id),
          profile_id: str(v.profile_id),
          profile_name: str(v.profile_name),
          scope: toScope(v.scope),
          via: v.via === "group" ? ("group" as const) : ("direct" as const),
          group_id: optStr(v.group_id),
          group_name: optStr(v.group_name),
        },
      ];
    }),
  };
}

const list = <T,>(x: unknown, f: (v: unknown) => T | null): T[] => arr(x).flatMap((v) => {
  const r = f(v);
  return r ? [r] : [];
});

function must<T>(v: T | null, what: string): T {
  if (!v) throw new Error(`The server returned an unreadable ${what}.`);
  return v;
}

// ------------------------------------------------------------------ keys

/** Query keys. Everything sits under `["access"]`, so one invalidation refreshes the console. */
export const acKeys = {
  all: ["access"] as const,
  catalog: ["access", "catalog"] as const,
  profiles: ["access", "profiles"] as const,
  profile: (id: string) => ["access", "profiles", id] as const,
  assignments: ["access", "assignments"] as const,
  groups: ["access", "groups"] as const,
  effective: (user: string, scope: string) => ["access", "effective", user, scope] as const,
  impact: (id: string, revision: number, actions: string[]) => ["access", "impact", id, revision, [...actions].sort().join(",")] as const,
  apiKeys: ["api-keys"] as const,
};

/** Every access change can change what the signed-in user may do too. */
const AFTER_CHANGE: QueryKey[] = [acKeys.all, ["me"]];

// ------------------------------------------------------------------ queries

export const useAccessCatalog = () =>
  useQuery({ queryKey: acKeys.catalog, queryFn: async () => toCatalog(await get<unknown>(`${BASE}/catalog`)), staleTime: 5 * 60_000 });

export const useProfiles = () =>
  useQuery({ queryKey: acKeys.profiles, queryFn: async () => list(await get<unknown>(`${BASE}/profiles`), toProfile), staleTime: 15_000 });

export const useProfile = (id: string | undefined) =>
  useQuery({
    queryKey: acKeys.profile(id ?? ""),
    enabled: !!id,
    queryFn: async () => must(toProfileDetail(await get<unknown>(`${BASE}/profiles/${enc(id ?? "")}`)), "role"),
  });

export const useAssignments = () =>
  useQuery({ queryKey: acKeys.assignments, queryFn: async () => list(await get<unknown>(`${BASE}/assignments`), toAssignment), staleTime: 15_000 });

export const useGroups = () =>
  useQuery({ queryKey: acKeys.groups, queryFn: async () => list(await get<unknown>(`${BASE}/groups`), toGroup), staleTime: 15_000 });

export const useEffectiveAccess = (userId: string, scope: string) =>
  useQuery({
    queryKey: acKeys.effective(userId, scope),
    enabled: !!userId && !!scope,
    queryFn: async () => toEffective(await get<unknown>(`${BASE}/effective?user_id=${enc(userId)}&scope=${enc(scope)}`)),
  });

// ------------------------------------------------------------------ mutations

export const useCreateProfile = () =>
  useAction(
    async (b: { name: string; description: string; actions: string[] }) => must(toProfile(await post<unknown>(`${BASE}/profiles`, b)), "role"),
    AFTER_CHANGE,
  );

export const useDuplicateProfile = () =>
  useAction(
    async ({ id, name }: { id: string; name: string }) => must(toProfile(await post<unknown>(`${BASE}/profiles/${enc(id)}/duplicate`, { name })), "role"),
    AFTER_CHANGE,
  );

/** Dry run of a new revision: what changes and for whom. Changes nothing, so it invalidates nothing. */
export const usePublishPreview = () =>
  useAction(
    async ({ id, actions, expected_revision }: { id: string; actions: string[]; expected_revision: number }) =>
      toImpact(await api<unknown>("PUT", `${BASE}/profiles/${enc(id)}?dry_run=true`, { actions, expected_revision })),
    [],
  );

export const usePublishProfile = () =>
  useAction(
    async ({ id, actions, expected_revision }: { id: string; actions: string[]; expected_revision: number }) =>
      must(toProfile(await api<unknown>("PUT", `${BASE}/profiles/${enc(id)}?dry_run=false`, { actions, expected_revision })), "role"),
    AFTER_CHANGE,
  );

export const useArchiveProfile = () =>
  useAction(async (id: string) => {
    const o = obj(await api<unknown>("DELETE", `${BASE}/profiles/${enc(id)}`));
    return { archived: o?.archived !== false, assignments_unlinked: num(o?.assignments_unlinked) };
  }, AFTER_CHANGE);

export interface AssignInput {
  user_id?: string;
  group_id?: string;
  profile_id: string;
  /** `kind:id`. */
  scope: string;
  expires_in_minutes?: number;
}

export const useCreateAssignment = () =>
  useAction(async (b: AssignInput) => must(toAssignment(await post<unknown>(`${BASE}/assignments`, b)), "assignment"), AFTER_CHANGE);

export const useDeleteAssignment = () => useAction((id: string) => api<void>("DELETE", `${BASE}/assignments/${enc(id)}`), AFTER_CHANGE);

export const useCreateGroup = () =>
  useAction(async (b: { name: string; description: string }) => must(toGroup(await post<unknown>(`${BASE}/groups`, b)), "group"), AFTER_CHANGE);

export const useUpdateGroup = () =>
  useAction(
    async ({ id, ...b }: { id: string; name?: string; description?: string }) => must(toGroup(await api<unknown>("PATCH", `${BASE}/groups/${enc(id)}`, b)), "group"),
    AFTER_CHANGE,
  );

export const useDeleteGroup = () => useAction((id: string) => api<void>("DELETE", `${BASE}/groups/${enc(id)}`), AFTER_CHANGE);

export const useGroupMember = () =>
  useAction(
    ({ group, user, add }: { group: string; user: string; add: boolean }) => api<void>(add ? "PUT" : "DELETE", `${BASE}/groups/${enc(group)}/members/${enc(user)}`),
    AFTER_CHANGE,
  );

/** Impact of publishing `actions` as the next revision of a role (a dry run; changes nothing). */
export const usePublishImpact = (id: string, actions: string[], expected_revision: number) =>
  useQuery({
    queryKey: acKeys.impact(id, expected_revision, actions),
    enabled: !!id,
    retry: false,
    staleTime: 30_000,
    queryFn: async () => toImpact(await api<unknown>("PUT", `${BASE}/profiles/${enc(id)}?dry_run=true`, { actions, expected_revision })),
  });

// ------------------------------------------------------------------ staged changes

/**
 * One change composed on the Access dashboard and applied later as part of a
 * batch. `key` identifies the change, so staging the same thing twice replaces
 * it and an opposite change can cancel it.
 */
export type AccessChange =
  | { type: "assign"; key: string; user_id?: string; group_id?: string; profile_id: string; scope: string; expires_in_minutes?: number }
  | { type: "unassign"; key: string; assignment: Assignment }
  | { type: "member"; key: string; group: string; user: string; add: boolean }
  | { type: "publish"; key: string; profile_id: string; actions: string[]; expected_revision: number };

/** Result of one applied change: `error` holds the server's reason when it was refused. */
export interface ChangeResult {
  key: string;
  ok: boolean;
  error?: string;
}

/** The server's refusals in plain words. */
export function accessErrorText(e: unknown): string {
  if (e instanceof ApiError) {
    if (e.code === "escalation") return `Refused: you can only grant permissions you hold at that scope. ${e.message}`;
    if (e.code === "mfa_required" || e.code === "step_up_required") return `Needs an MFA-verified sign-in: ${e.message}`;
    if (e.status === 409) return `Changed by someone else meanwhile; reload and try again. ${e.message}`;
    return e.message || `${e.status} ${e.code}`;
  }
  return e instanceof Error ? e.message : String(e);
}

async function runChange(c: AccessChange): Promise<void> {
  switch (c.type) {
    case "assign": {
      const body: AssignInput = { profile_id: c.profile_id, scope: c.scope };
      if (c.user_id) body.user_id = c.user_id;
      if (c.group_id) body.group_id = c.group_id;
      if (c.expires_in_minutes) body.expires_in_minutes = c.expires_in_minutes;
      await post<unknown>(`${BASE}/assignments`, body);
      return;
    }
    case "unassign":
      await api<void>("DELETE", `${BASE}/assignments/${enc(c.assignment.id)}`);
      return;
    case "member":
      await api<void>(c.add ? "PUT" : "DELETE", `${BASE}/groups/${enc(c.group)}/members/${enc(c.user)}`);
      return;
    case "publish":
      await api<unknown>("PUT", `${BASE}/profiles/${enc(c.profile_id)}?dry_run=false`, { actions: c.actions, expected_revision: c.expected_revision });
      return;
  }
}

/** Applies staged changes one after another; never throws, reports each change's outcome. */
export const useApplyAccessChanges = () =>
  useAction(async (changes: AccessChange[]) => {
    const out: ChangeResult[] = [];
    for (const c of changes) {
      try {
        await runChange(c);
        out.push({ key: c.key, ok: true });
      } catch (e) {
        out.push({ key: c.key, ok: false, error: accessErrorText(e) });
      }
    }
    return out;
  }, AFTER_CHANGE);

// ------------------------------------------------------------------ personal API keys

/** A token restriction: the token may only run `actions` at `scope`. */
export interface TokenRestriction {
  scope: ScopeRef;
  actions: string[];
}

export interface ApiKey {
  id: string;
  name: string;
  prefix: string;
  expires_at: number | null;
  last_used_at: number | null;
  created_at: number;
  /** May run MFA-guarded actions without a prompt. */
  elevated: boolean;
  /** `null`: the key has all its owner's access. */
  restrictions: TokenRestriction[] | null;
}

function toApiKey(x: unknown): ApiKey | null {
  const o = obj(x);
  const id = str(o?.id);
  if (!o || !id) return null;
  return {
    id,
    name: str(o.name, id) || id,
    prefix: str(o.prefix),
    expires_at: optNum(o.expires_at),
    last_used_at: optNum(o.last_used_at),
    created_at: num(o.created_at),
    elevated: o.elevated === true,
    restrictions: Array.isArray(o.restrictions)
      ? o.restrictions.flatMap((r) => {
          const v = obj(r);
          return v ? [{ scope: toScope(v.scope), actions: strs(v.actions) }] : [];
        })
      : null,
  };
}

export interface CreateApiKeyInput {
  name: string;
  /** Days until it expires; `null` never expires. */
  expires_days: number | null;
  elevated: boolean;
  /** `kind:id` per entry; omitted for all of the owner's access. */
  restrictions?: { scope: string; actions: string[] }[];
}

/** Personal keys and service-account tokens share this shape. */
export const apiKeyList = (x: unknown): ApiKey[] => list(x, toApiKey);

export const useApiKeys = () => useQuery({ queryKey: acKeys.apiKeys, queryFn: async () => apiKeyList(await get<unknown>("/api/v1/api-keys")) });

/** A service account's tokens (the same kind of key as a personal one). */
export const serviceAccountTokensKey = (accountId: string) => ["service-account-tokens", accountId] as const;

/**
 * Creates a personal access token, or one for service account `accountId`:
 * the same kind of key and body either way. Optional fields are sent only
 * when set, so older servers (strict bodies) keep working.
 */
export const useCreateApiKey = (accountId?: string) =>
  useAction(async (b: CreateApiKeyInput) => {
    const path = accountId ? `/api/v1/service-accounts/${enc(accountId)}/tokens` : "/api/v1/api-keys";
    const o = obj(
      await post<unknown>(path, {
        name: b.name,
        ...(b.expires_days ? { expires_days: b.expires_days } : { never_expires: true }),
        ...(b.elevated ? { elevated: true } : {}),
        ...(b.restrictions ? { restrictions: b.restrictions } : {}),
      }),
    );
    return { secret: optStr(o?.secret) };
  }, accountId ? [serviceAccountTokensKey(accountId), ["service-accounts"]] : [acKeys.apiKeys]);

export const useRevokeApiKey = () => useAction((id: string) => api<void>("DELETE", `/api/v1/api-keys/${enc(id)}`), [acKeys.apiKeys]);

// ------------------------------------------------------------------ helpers

/** Order of access levels: an area set to a level selects every action at or below it. */
export const LEVEL_RANK: Record<AccessLevel, number> = { read: 1, write: 2, admin: 3 };

/** Areas in catalog order with their actions. */
export function byArea<T extends Pick<CatalogAction, "area">>(actions: T[]): { area: string; actions: T[] }[] {
  const out: { area: string; actions: T[] }[] = [];
  for (const a of actions) {
    const g = out.find((x) => x.area === a.area);
    if (g) g.actions.push(a);
    else out.push({ area: a.area, actions: [a] });
  }
  return out;
}
