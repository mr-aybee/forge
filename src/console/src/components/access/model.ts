// Derived access model for the Access dashboard: who holds which role where,
// directly or inherited from a group or a parent scope, and the staged
// (not yet applied) changes on top of it. Everything is computed during
// render from the queries; nothing here fetches.

import { useMemo, useState } from "react";

import { scopeKey, type AccessChange, type Assignment, type ChangeResult, type Group, type Profile } from "../../api/accessControl";
import { ancestorsOf, scopePath, type ScopeIndex } from "./scopes";

/** Who an assignment or a selection is about. */
export type Principal = { kind: "user"; id: string } | { kind: "group"; id: string };

/** One role held at a scope, and why. */
export interface Held {
  assignment: Assignment;
  /** Editable here: the principal's own assignment at exactly this scope. */
  direct: boolean;
  /** Why it applies when it is not direct: "via group ops", "from shop / prod". */
  via: string;
}

export function useAccessModel({
  assignments,
  groups,
  profiles,
  idx,
}: {
  assignments: Assignment[];
  groups: Group[];
  profiles: Profile[];
  idx: ScopeIndex;
}) {
  return useMemo(() => {
    const profileById = new Map(profiles.map((p) => [p.id, p]));
    const groupById = new Map(groups.map((g) => [g.id, g]));
    const groupsOf = (user: string) => groups.filter((g) => g.members.includes(user));
    const byScope = new Map<string, Assignment[]>();
    for (const a of assignments) {
      const k = scopeKey(a.scope);
      byScope.set(k, [...(byScope.get(k) ?? []), a]);
    }
    const holds = (p: Principal, a: Assignment, userGroups: Set<string>) =>
      p.kind === "user" ? a.user_id === p.id || (!!a.group_id && userGroups.has(a.group_id)) : a.group_id === p.id;

    /** Roles `p` holds at `key`: direct ones first, then inherited from groups or parents. */
    const heldAt = (p: Principal, key: string): Held[] => {
      const ug = new Set(p.kind === "user" ? groupsOf(p.id).map((g) => g.id) : []);
      const out: Held[] = [];
      for (const k of [key, ...ancestorsOf(idx, key)]) {
        for (const a of byScope.get(k) ?? []) {
          if (!holds(p, a, ug)) continue;
          const own = p.kind === "user" ? a.user_id === p.id : a.group_id === p.id;
          const viaGroup = !own && a.group_id ? `via group ${groupById.get(a.group_id)?.name ?? a.group_id.slice(0, 8)}` : "";
          const fromParent = k !== key ? `from ${scopePath(idx, a.scope)}` : "";
          out.push({ assignment: a, direct: own && k === key, via: [viaGroup, fromParent].filter(Boolean).join(", ") });
        }
      }
      return out;
    };

    /** Assignments a principal holds directly (user: own only; group: the group's). */
    const directOf = (p: Principal) => assignments.filter((a) => (p.kind === "user" ? a.user_id === p.id : a.group_id === p.id));

    /** Every assignment that gives `p` something somewhere (user: own and through groups). */
    const allOf = (p: Principal) => {
      if (p.kind === "group") return directOf(p);
      const ug = new Set(groupsOf(p.id).map((g) => g.id));
      return assignments.filter((a) => holds(p, a, ug));
    };

    /** Whether `p` has any role at `key` or above it. */
    const reaches = (p: Principal, key: string) => heldAt(p, key).length > 0;

    return { assignments, profileById, groupById, groupsOf, heldAt, directOf, allOf, reaches, byScope };
  }, [assignments, groups, profiles, idx]);
}

export type AccessModel = ReturnType<typeof useAccessModel>;

/** Staged changes with the outcome of the last apply. One state object. */
export function useStaging() {
  const [s, setS] = useState<{ changes: AccessChange[]; errors: Record<string, string> }>({ changes: [], errors: {} });
  const has = (key: string) => s.changes.some((c) => c.key === key);
  /** Adds or replaces a change by its key. */
  const stage = (c: AccessChange) =>
    setS((x) => {
      const errors = { ...x.errors };
      delete errors[c.key];
      return { changes: [...x.changes.filter((y) => y.key !== c.key), c], errors };
    });
  const unstage = (key: string) =>
    setS((x) => {
      const errors = { ...x.errors };
      delete errors[key];
      return { changes: x.changes.filter((y) => y.key !== key), errors };
    });
  /** Stages `c` unless its opposite (`cancels`) is staged, in which case that one is dropped instead. */
  const toggle = (c: AccessChange, cancels: string) => (has(cancels) ? unstage(cancels) : stage(c));
  /** Keeps only failed changes, with their reasons. */
  const settle = (results: ChangeResult[]) =>
    setS((x) => {
      const failed = new Map(results.filter((r) => !r.ok).map((r) => [r.key, r.error ?? "Failed"]));
      const applied = new Set(results.filter((r) => r.ok).map((r) => r.key));
      return { changes: x.changes.filter((c) => !applied.has(c.key)), errors: Object.fromEntries(failed) };
    });
  const clear = () => setS({ changes: [], errors: {} });
  return { changes: s.changes, errors: s.errors, has, stage, unstage, toggle, settle, clear, get: (key: string) => s.changes.find((c) => c.key === key) };
}

export type Staging = ReturnType<typeof useStaging>;

export const assignKey = (p: Principal, profile: string, scope: string) => `assign:${p.kind}:${p.id}:${profile}:${scope}`;
export const unassignKey = (assignmentId: string) => `unassign:${assignmentId}`;
export const memberKey = (group: string, user: string, add: boolean) => `member:${add ? "add" : "rm"}:${group}:${user}`;
export const publishKey = (profile: string) => `publish:${profile}`;

/** Staged assignments for a principal at a scope (not applied yet). */
export const stagedAssigns = (st: Staging, p: Principal, scope: string) =>
  st.changes.filter(
    (c): c is Extract<AccessChange, { type: "assign" }> => c.type === "assign" && c.scope === scope && (p.kind === "user" ? c.user_id === p.id : c.group_id === p.id),
  );
